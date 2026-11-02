use chrono::NaiveDate;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

use crate::{
    camera::{self, Camera, How},
    config::{Notify, Settings},
    device,
};

#[derive(Clone)]
pub struct Item {
    /// Path on the card, or the shell name of a file on a device.
    pub src: PathBuf,
    pub name: String,
    /// Shell id of a file on a device plugged in by cable; None for a file on a card.
    pub pidl: Option<Arc<Vec<u8>>>,
    /// Subfolder at the destination, from the camera's rules.
    pub ty: String,
    pub date: NaiveDate,
    pub mtime: SystemTime,
    pub size: u64,
}

#[derive(Clone)]
pub struct Card {
    /// Drive root (`E:\`) or the shell name of a device; the key for its job.
    pub root: String,
    /// What the user sees: the drive or the device name.
    pub title: String,
    pub device: bool,
    /// Shown instead of the counts: a locked phone, or a device still being read.
    pub hint: Option<String>,
    /// The device is locked or has not allowed access yet.
    pub locked: bool,
    /// Files of a device as listed, so it can be sorted again without asking the device.
    pub listed: Option<Arc<Vec<Listed>>>,
    pub serial: Option<String>,
    pub camera: Camera,
    pub how: How,
    /// Model name read from the files, if any.
    pub model: Option<String>,
    pub items: Arc<Vec<Item>>,
    /// Files per destination subfolder.
    pub counts: BTreeMap<String, usize>,
    /// Extensions the camera has no rule for, with their file count; these are not imported.
    pub unsorted: BTreeMap<String, usize>,
    pub bytes: u64,
    /// Library camera that fits an unknown card.
    pub suggest: Option<Camera>,
}

impl Card {
    pub fn summary(&self) -> String {
        if self.counts.is_empty() {
            return "no files to import".into();
        }
        self.counts.iter().map(|(t, n)| format!("{t} {n}")).collect::<Vec<_>>().join("   ")
    }

    pub fn unsorted_line(&self) -> String {
        self.unsorted
            .iter()
            .map(|(e, n)| format!("{} {n}", e.to_uppercase()))
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// No camera matched, or files showed up that the camera has no rule for.
    pub fn needs_setup(&self) -> bool {
        self.how == How::Unknown || !self.unsorted.is_empty()
    }
}

#[derive(Clone, Default, PartialEq)]
pub enum State {
    #[default]
    Idle,
    Running,
    Done,
}

#[derive(Clone, Default)]
pub struct Progress {
    pub state: State,
    pub files_total: usize,
    pub files_done: usize,
    pub bytes_total: u64,
    pub bytes_done: u64,
    pub current: String,
    pub copied: usize,
    pub skipped: usize,
    pub failed: usize,
    pub dest: PathBuf,
    pub cancel: Arc<AtomicBool>,
    pub cancelled: bool,
    pub started: Option<Instant>,
    pub ended: Option<Instant>,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        if self.bytes_total == 0 {
            return if self.state == State::Done { 1.0 } else { 0.0 };
        }
        (self.bytes_done as f32 / self.bytes_total as f32).min(1.0)
    }

    /// Average bytes per second since the job started.
    pub fn speed(&self) -> f64 {
        let Some(start) = self.started else { return 0.0 };
        let secs = self.ended.unwrap_or_else(Instant::now).duration_since(start).as_secs_f64();
        if secs < 0.5 { 0.0 } else { self.bytes_done as f64 / secs }
    }
}

pub struct Shared {
    pub cards: Vec<Card>,
    /// One job per drive root, kept until the user dismisses it.
    pub jobs: HashMap<String, Progress>,
    pub settings: Settings,
    pub cameras: Vec<Camera>,
    pub library: crate::library::Library,
    /// Camera picked by hand per drive root, for cards without a volume serial.
    pub picked: HashMap<String, String>,
    /// Devices being read right now.
    pub reading: HashSet<String>,
    pub update: crate::update::Update,
    /// Set by background threads, picked up by the UI thread.
    pub popup_request: Option<String>,
    pub want_full: bool,
}

pub type Handle = Arc<Mutex<Shared>>;

pub fn any_running(g: &Shared) -> bool {
    g.jobs.values().any(|p| p.state == State::Running)
}

fn scan(root: &str, cameras: &[Camera], library: &[Camera], pick: Option<&str>) -> Card {
    let base = Path::new(root);
    let serial = camera::volume_serial(root);
    let found = camera::detect(base, cameras, pick);
    let cam = found.camera.clone();
    let suggest = if found.how == How::Unknown && !library.is_empty() {
        let f = camera::detect(base, library, None);
        (f.how != How::Unknown).then(|| f.camera.clone())
    } else {
        None
    };

    let mut items = Vec::new();
    let mut unsorted = BTreeMap::new();
    camera::walk_media(base, &cam.sources, &mut |p, md, ext, src| match camera::type_for(&cam, src, ext) {
        None => *unsorted.entry(ext.to_string()).or_insert(0) += 1,
        Some("") => {}
        Some(ty) => {
            let Ok(mtime) = md.modified() else { return };
            items.push(Item {
                src: p.to_path_buf(),
                name: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                pidl: None,
                ty: ty.to_string(),
                date: camera::shoot_date(&cam, p, mtime),
                mtime,
                size: md.len(),
            });
        }
    });

    let mut card = card_from(root, cam, found.how, found.model, items, unsorted, suggest);
    card.serial = serial;
    card
}

fn card_from(
    root: &str,
    cam: Camera,
    how: How,
    model: Option<String>,
    mut items: Vec<Item>,
    unsorted: BTreeMap<String, usize>,
    suggest: Option<Camera>,
) -> Card {
    items.sort_by(|a, b| a.src.cmp(&b.src));
    let mut counts = BTreeMap::new();
    for it in &items {
        *counts.entry(it.ty.clone()).or_insert(0) += 1;
    }
    Card {
        root: root.to_string(),
        title: root.to_string(),
        device: false,
        hint: None,
        locked: false,
        listed: None,
        serial: None,
        camera: cam,
        how,
        model,
        counts,
        unsorted,
        bytes: items.iter().map(|i| i.size).sum(),
        items: Arc::new(items),
        suggest,
    }
}

/// One file of a device as it was listed.
#[derive(Clone)]
pub struct Listed {
    pub name: String,
    pub ext: String,
    /// Camera folder it was found under.
    pub src: String,
    pub size: u64,
    pub mtime: SystemTime,
    pub pidl: Arc<Vec<u8>>,
}

fn sort_listed(cam: &Camera, listed: &[Listed]) -> (Vec<Item>, BTreeMap<String, usize>) {
    let mut items = Vec::new();
    let mut unsorted = BTreeMap::new();
    for f in listed {
        match camera::type_for(cam, &f.src, &f.ext) {
            None => *unsorted.entry(f.ext.clone()).or_insert(0) += 1,
            Some("") => {}
            Some(ty) => items.push(Item {
                src: PathBuf::from(&f.name),
                name: f.name.clone(),
                pidl: Some(f.pidl.clone()),
                ty: ty.to_string(),
                date: camera::shoot_date(cam, Path::new(&f.name), f.mtime),
                mtime: f.mtime,
                size: f.size,
            }),
        }
    }
    (items, unsorted)
}

fn put(h: &Handle, ctx: &eframe::egui::Context, card: Card) {
    if let Some(slot) = h.lock().unwrap().cards.iter_mut().find(|c| c.root == card.root) {
        *slot = card;
    }
    ctx.request_repaint();
}

/// Reads a phone or camera plugged in by cable in the background. The card shows up right
/// away and fills in once the files are listed, which can take minutes on a full phone.
fn read_device(h: &Handle, ctx: &eframe::egui::Context, key: String, name: String, fresh: bool) {
    if !h.lock().unwrap().reading.insert(key.clone()) {
        return;
    }
    let (h, ctx) = (h.clone(), ctx.clone());
    thread::spawn(move || {
        device::init();
        let (cams, lib, pick, old) = {
            let g = h.lock().unwrap();
            let (cams, lib) = camera_lists(&g);
            let old = g.cards.iter().find(|c| c.root == key).map(|c| (c.locked, c.camera.sources.clone(), c.listed.clone()));
            (cams, lib, picked_for(&g, &key, Some(&key)), old)
        };
        let opened = device::Opened::new(&key);
        let has = |p: &str| opened.as_ref().is_some_and(|o| o.has_path(p));
        let found = camera::detect_device(&name, &has, &cams, pick.as_deref());
        let cam = found.camera.clone();
        let suggest = if found.how == How::Unknown && !lib.is_empty() {
            let f = camera::detect_device(&name, &has, &lib, None);
            (f.how != How::Unknown).then(|| f.camera.clone())
        } else {
            None
        };
        let locked = opened.as_ref().is_none_or(|o| o.is_empty());
        // Players, e-readers and the like are no camera and stay out of the list.
        if fresh && found.how == How::Unknown && suggest.is_none() && !locked && !has("DCIM") {
            h.lock().unwrap().reading.remove(&key);
            return;
        }
        let was_locked = old.as_ref().is_some_and(|(l, _, _)| *l);
        // The same folders as last time: sort what was listed instead of asking the device again.
        let cached = old.and_then(|(_, src, listed)| listed.filter(|_| src == cam.sources));

        let make = |listed: Option<Arc<Vec<Listed>>>, hint: Option<String>| {
            let (items, unsorted) = listed.as_deref().map(|l| sort_listed(&cam, l)).unwrap_or_default();
            let mut c = card_from(&key, cam.clone(), found.how, found.model.clone(), items, unsorted, suggest.clone());
            c.title = name.clone();
            c.device = true;
            c.serial = Some(key.clone());
            c.locked = locked;
            c.listed = listed;
            c.hint = hint;
            c
        };
        let first = if locked {
            make(None, Some("Unlock the device and allow access to its photos.".into()))
        } else if cached.is_some() {
            make(cached.clone(), None)
        } else {
            make(None, Some(format!("Reading {name} …")))
        };
        if fresh {
            arrived(&h, &ctx, first);
        } else {
            put(&h, &ctx, first);
        }

        if !locked && cached.is_none() {
            let mut listed = Vec::new();
            let mut last = Instant::now();
            if let Some(o) = &opened {
                o.walk(&cam.sources, &mut |f, ext, src| {
                    listed.push(Listed {
                        name: f.name,
                        ext: ext.to_string(),
                        src: src.to_string(),
                        size: f.size,
                        mtime: f.mtime,
                        pidl: Arc::new(f.pidl),
                    });
                    if last.elapsed() > Duration::from_millis(700) {
                        last = Instant::now();
                        if let Some(c) = h.lock().unwrap().cards.iter_mut().find(|c| c.root == key) {
                            c.hint = Some(format!("Reading {name} … {} files", listed.len()));
                        }
                        ctx.request_repaint();
                    }
                });
            }
            let card = make(Some(Arc::new(listed)), None);
            put(&h, &ctx, card.clone());
            if fresh || was_locked {
                {
                    let mut g = h.lock().unwrap();
                    if g.settings.notify == Notify::Popup {
                        g.popup_request = Some(key.clone());
                    }
                }
                auto_import(&h, &ctx, &card);
            }
        }
        h.lock().unwrap().reading.remove(&key);
    });
}

/// Counts what is on a card or a listed device, for the camera editor.
pub fn survey(card: &Card, cam: &Camera) -> camera::Survey {
    if !card.device {
        return camera::survey(Path::new(&card.root), cam);
    }
    let mut out = camera::Survey::default();
    for f in card.listed.iter().flat_map(|l| l.iter()) {
        *out.folders.entry(f.src.clone()).or_insert(0) += 1;
        if !cam.folders.contains_key(&f.src) {
            *out.exts.entry(f.ext.clone()).or_insert(0) += 1;
        }
    }
    out
}

/// The user's cameras and the library cameras they do not have yet.
fn camera_lists(g: &Shared) -> (Vec<Camera>, Vec<Camera>) {
    (g.cameras.clone(), g.library.missing(&g.cameras))
}

/// Camera the user chose for this card earlier, by volume serial or for this session.
fn picked_for(g: &Shared, root: &str, serial: Option<&String>) -> Option<String> {
    g.picked
        .get(root)
        .or_else(|| serial.and_then(|s| g.settings.cards.get(s)))
        .cloned()
}

/// Remembers the camera for a card (until it is formatted) and sorts it again.
pub fn assign(h: &Handle, ctx: &eframe::egui::Context, root: &str, camera_id: &str) {
    {
        let mut g = h.lock().unwrap();
        g.picked.insert(root.to_string(), camera_id.to_string());
        let serial = g.cards.iter().find(|c| c.root == root).and_then(|c| c.serial.clone());
        if let Some(s) = serial {
            g.settings.cards.insert(s, camera_id.to_string());
            crate::config::save(&g.settings);
        }
    }
    rescan_all(h, ctx);
}

/// Sorts every inserted card again, after cameras were changed. Running imports keep their list.
pub fn rescan_all(h: &Handle, ctx: &eframe::egui::Context) {
    let (h, ctx) = (h.clone(), ctx.clone());
    thread::spawn(move || {
        let cards: Vec<(String, String, bool)> =
            h.lock().unwrap().cards.iter().map(|c| (c.root.clone(), c.title.clone(), c.device)).collect();
        for (root, title, is_device) in cards {
            if h.lock().unwrap().jobs.get(&root).is_some_and(|p| p.state == State::Running) {
                continue;
            }
            if is_device {
                read_device(&h, &ctx, root, title, false);
                continue;
            }
            let (cams, lib, pick) = {
                let g = h.lock().unwrap();
                let (cams, lib) = camera_lists(&g);
                (cams, lib, picked_for(&g, &root, camera::volume_serial(&root).as_ref()))
            };
            put(&h, &ctx, scan(&root, &cams, &lib, pick.as_deref()));
        }
    });
}

/// Asks the UI thread to open the full window.
pub fn show_window(h: &Handle, ctx: &eframe::egui::Context) {
    h.lock().unwrap().want_full = true;
    ctx.request_repaint();
}

pub fn toast(title: &str, body: &str, button: Option<&str>, on_click: impl Fn(bool) + Send + 'static) {
    use tauri_winrt_notification::Toast;
    let mut t = Toast::new(Toast::POWERSHELL_APP_ID).title(title).text1(body);
    if let Some(b) = button {
        t = t.add_button(b, "import");
    }
    let _ = t
        .on_activated(move |action| {
            on_click(action.is_some_and(|a| a.contains("import")));
            Ok(())
        })
        .show();
}

/// A card or device showed up: list it, tell the user, maybe import right away.
fn arrived(h: &Handle, ctx: &eframe::egui::Context, card: Card) {
    let root = card.root.clone();
    let (mode, auto) = {
        let mut g = h.lock().unwrap();
        g.cards.push(card.clone());
        // A fresh card replaces the finished job of the same letter.
        if g.jobs.get(&root).is_some_and(|p| p.state == State::Done) {
            g.jobs.remove(&root);
        }
        if g.settings.notify == Notify::Popup {
            g.popup_request = Some(root.clone());
        }
        (g.settings.notify, g.settings.auto_import)
    };
    if mode == Notify::Toast {
        let body = format!("{}  ·  {}  ·  {}", card.title, card.camera.name, card.summary());
        let (h2, ctx2, root2) = (h.clone(), ctx.clone(), root.clone());
        toast("SWORTA – Card detected", &body, Some("Import"), move |import| {
            show_window(&h2, &ctx2);
            if import {
                begin_import(&h2, &ctx2, &root2);
            }
        });
    }
    if auto {
        auto_import(h, ctx, &card);
    }
    ctx.request_repaint();
}

fn auto_import(h: &Handle, ctx: &eframe::egui::Context, card: &Card) {
    let (auto, dest) = {
        let g = h.lock().unwrap();
        (g.settings.auto_import, card.camera.dest.clone().unwrap_or_else(|| g.settings.dest.clone()))
    };
    // An unknown camera waits for the user instead of being sorted by guesswork.
    if auto && card.how != How::Unknown && card.hint.is_none() {
        start_import(h, ctx, &card.root, dest);
    }
}

fn gone(h: &Handle, ctx: &eframe::egui::Context, root: &str) {
    let mut g = h.lock().unwrap();
    g.cards.retain(|c| c.root != root);
    g.picked.remove(root);
    ctx.request_repaint();
}

pub fn spawn_watcher(h: Handle, ctx: eframe::egui::Context) {
    thread::spawn(move || {
        device::init();
        let sys = std::env::var("SystemDrive")
            .ok()
            .and_then(|s| s.chars().next())
            .map_or('C', |c| c.to_ascii_uppercase());
        let mut known: HashSet<char> = HashSet::new();
        let mut known_dev: HashSet<String> = HashSet::new();
        loop {
            let (cams, lib) = camera_lists(&h.lock().unwrap());
            let every: Vec<Camera> = cams.iter().chain(&lib).cloned().collect();
            for l in 'D'..='Z' {
                if l == sys {
                    continue;
                }
                let root = format!("{l}:\\");
                let present = Path::new(&root).is_dir() && camera::looks_like_card(Path::new(&root), &every);
                if present && known.insert(l) {
                    let pick = {
                        let g = h.lock().unwrap();
                        picked_for(&g, &root, camera::volume_serial(&root).as_ref())
                    };
                    arrived(&h, &ctx, scan(&root, &cams, &lib, pick.as_deref()));
                } else if !present && known.remove(&l) {
                    gone(&h, &ctx, &root);
                }
            }

            let devices = device::list();
            for d in &devices {
                if known_dev.insert(d.key.clone()) {
                    read_device(&h, &ctx, d.key.clone(), d.name.clone(), true);
                } else if h.lock().unwrap().cards.iter().any(|c| c.root == d.key && c.locked) {
                    // Looked at again until the phone is unlocked.
                    read_device(&h, &ctx, d.key.clone(), d.name.clone(), false);
                }
            }
            let left: Vec<String> = known_dev.iter().filter(|k| !devices.iter().any(|d| &d.key == *k)).cloned().collect();
            for k in left {
                known_dev.remove(&k);
                gone(&h, &ctx, &k);
            }
            thread::sleep(Duration::from_secs(2));
        }
    });
}

static RESERVED: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(Default::default);
static NEXT_SLOT: LazyLock<Mutex<Instant>> = LazyLock::new(|| Mutex::new(Instant::now()));

/// Shared speed cap across every running copy thread.
fn throttle(limit_mb: u32, bytes: u64) {
    if limit_mb == 0 {
        return;
    }
    let cost = Duration::from_secs_f64(bytes as f64 / (limit_mb as f64 * 1e6));
    let wait = {
        let mut next = NEXT_SLOT.lock().unwrap();
        let now = Instant::now();
        let start = (*next).max(now);
        *next = start + cost;
        start.saturating_duration_since(now)
    };
    if !wait.is_zero() {
        thread::sleep(wait);
    }
}

pub fn start_import(h: &Handle, ctx: &eframe::egui::Context, root: &str, dest: PathBuf) {
    let (card, long_date, workers, cancel) = {
        let mut g = h.lock().unwrap();
        if g.jobs.get(root).is_some_and(|p| p.state == State::Running) {
            return;
        }
        let Some(card) = g.cards.iter().find(|c| c.root == root).cloned() else {
            return;
        };
        if card.items.is_empty() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        g.jobs.insert(
            root.to_string(),
            Progress {
                state: State::Running,
                files_total: card.items.len(),
                bytes_total: card.bytes,
                dest: dest.clone(),
                cancel: cancel.clone(),
                started: Some(Instant::now()),
                ..Default::default()
            },
        );
        let s = &g.settings;
        (card, s.long_date, s.parallel.clamp(1, 4) as usize, cancel)
    };

    let next = Arc::new(AtomicUsize::new(0));
    let alive = Arc::new(AtomicUsize::new(workers));
    for _ in 0..workers {
        let (h, ctx, card, dest) = (h.clone(), ctx.clone(), card.clone(), dest.clone());
        let (next, alive, root, cancel) = (next.clone(), alive.clone(), root.to_string(), cancel.clone());
        thread::spawn(move || {
            if card.device {
                device::init();
            }
            loop {
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(it) = card.items.get(i) else { break };
                if let Some(p) = h.lock().unwrap().jobs.get_mut(&root) {
                    p.current = it.name.clone();
                }
                let date = it.date.format(if long_date { "%Y-%m-%d" } else { "%y%m%d" }).to_string();
                let dir = camera::target_dir(&dest, &card.camera.layout, &date, &it.ty, &card.camera.name);

                let res = copy_one(it, &dir, &mut |n| {
                    let limit = {
                        let mut g = h.lock().unwrap();
                        if let Some(p) = g.jobs.get_mut(&root) {
                            p.bytes_done += n;
                        }
                        g.settings.speed_limit
                    };
                    throttle(limit, n);
                    ctx.request_repaint();
                    !cancel.load(Ordering::SeqCst)
                });

                let mut g = h.lock().unwrap();
                if let Some(p) = g.jobs.get_mut(&root) {
                    p.files_done += 1;
                    match res {
                        Ok(true) => p.copied += 1,
                        Ok(false) => {
                            p.skipped += 1;
                            p.bytes_done += it.size;
                        }
                        Err(_) if cancel.load(Ordering::SeqCst) => {}
                        Err(_) => p.failed += 1,
                    }
                }
                drop(g);
                ctx.request_repaint();
            }
            if alive.fetch_sub(1, Ordering::SeqCst) == 1 {
                finish(&h, &ctx, &root, &dest);
            }
        });
    }
}

fn finish(h: &Handle, ctx: &eframe::egui::Context, root: &str, dest: &Path) {
    let (p, mode, open) = {
        let mut g = h.lock().unwrap();
        let mode = g.settings.notify;
        let open = g.settings.open_when_done;
        let Some(p) = g.jobs.get_mut(root) else { return };
        p.state = State::Done;
        p.cancelled = p.cancel.load(Ordering::SeqCst);
        p.ended = Some(Instant::now());
        p.current.clear();
        (p.clone(), mode, open)
    };
    ctx.request_repaint();
    if open && !p.cancelled {
        let _ = std::process::Command::new("explorer").arg(dest).spawn();
    }
    if mode == Notify::Toast {
        let body = format!("{} copied  ·  {} skipped  ·  {} failed", p.copied, p.skipped, p.failed);
        let title = if p.cancelled { "SWORTA – Cancelled" } else { "SWORTA – Done" };
        let (h2, ctx2) = (h.clone(), ctx.clone());
        toast(title, &body, None, move |_| show_window(&h2, &ctx2));
    }
}

/// Stops a running import; files already copied stay, the half-written one is removed.
pub fn cancel_import(h: &Handle, root: &str) {
    if let Some(p) = h.lock().unwrap().jobs.get(root) {
        p.cancel.store(true, Ordering::SeqCst);
    }
}

fn same_mtime(md: &fs::Metadata, t: SystemTime) -> bool {
    let Ok(m) = md.modified() else { return false };
    let diff = m.duration_since(t).or_else(|_| t.duration_since(m)).unwrap_or_default();
    diff <= Duration::from_secs(2)
}

/// Ok(true) = copied, Ok(false) = already present (skipped).
fn copy_one(it: &Item, dir: &Path, on_bytes: &mut dyn FnMut(u64) -> bool) -> std::io::Result<bool> {
    fs::create_dir_all(dir)?;
    let name = it.name.clone();
    let (stem, extn) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (name.clone(), String::new()),
    };

    // Same name, size and time counts as already imported. Anything else
    // gets a suffix, and parallel copies reserve their target name.
    let mut n = 0;
    let target = loop {
        let cand = if n == 0 {
            dir.join(&name)
        } else {
            dir.join(format!("{stem}_{n}{extn}"))
        };
        let mut reserved = RESERVED.lock().unwrap();
        match fs::metadata(&cand) {
            Ok(md) if md.len() == it.size && same_mtime(&md, it.mtime) => return Ok(false),
            Ok(_) => {}
            Err(_) => {
                if reserved.insert(cand.clone()) {
                    break cand;
                }
            }
        }
        n += 1;
    };

    let res = write_file(it, &target, on_bytes);
    RESERVED.lock().unwrap().remove(&target);
    res.map(|_| true)
}

fn write_file(it: &Item, target: &Path, on_bytes: &mut dyn FnMut(u64) -> bool) -> std::io::Result<()> {
    let part = target.with_file_name(format!("{}.part", target.file_name().unwrap().to_string_lossy()));
    let mut src: Box<dyn Read> = match &it.pidl {
        Some(p) => Box::new(device::read(p)?),
        None => Box::new(fs::File::open(&it.src)?),
    };
    let mtime = Some(it.mtime);
    let mut out = fs::File::create(&part)?;
    let mut buf = vec![0u8; 1 << 20];
    let copied = (|| -> std::io::Result<()> {
        loop {
            let r = src.read(&mut buf)?;
            if r == 0 {
                break;
            }
            out.write_all(&buf[..r])?;
            if !on_bytes(r as u64) {
                return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"));
            }
        }
        out.flush()
    })();
    if let Err(e) = copied {
        drop(out);
        let _ = fs::remove_file(&part);
        return Err(e);
    }
    if let Some(t) = mtime {
        let _ = out.set_modified(t);
    }
    drop(out);
    fs::rename(&part, target)
}

/// Starts an import, asking for the destination first unless the camera or the settings fix it.
pub fn begin_import(h: &Handle, ctx: &eframe::egui::Context, root: &str) {
    let (h, ctx, root) = (h.clone(), ctx.clone(), root.to_string());
    thread::spawn(move || {
        let (ask, last, fixed, title) = {
            let g = h.lock().unwrap();
            let card = g.cards.iter().find(|c| c.root == root);
            let fixed = card.and_then(|c| c.camera.dest.clone());
            let title = card.map_or(root.clone(), |c| c.title.clone());
            (g.settings.ask_dest, g.settings.dest.clone(), fixed, title)
        };
        let dest = if let Some(d) = fixed {
            d
        } else if ask {
            let start = if last.exists() { last.clone() } else { last.parent().map(Path::to_path_buf).unwrap_or(last.clone()) };
            let picked = rfd::FileDialog::new()
                .set_title(format!("Destination folder for {title}"))
                .set_directory(start)
                .pick_folder();
            let Some(p) = picked else { return };
            let mut g = h.lock().unwrap();
            g.settings.dest = p.clone();
            crate::config::save(&g.settings);
            p
        } else {
            last
        };
        start_import(&h, &ctx, &root, dest);
    });
}
