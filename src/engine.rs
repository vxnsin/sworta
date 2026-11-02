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
};

#[derive(Clone)]
pub struct Item {
    pub src: PathBuf,
    /// Subfolder at the destination, from the camera's rules.
    pub ty: String,
    pub date: NaiveDate,
    pub mtime: SystemTime,
    pub size: u64,
}

#[derive(Clone)]
pub struct Card {
    pub root: String,
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
    /// Camera picked by hand per drive root, for cards without a volume serial.
    pub picked: HashMap<String, String>,
    pub update: crate::update::Update,
    /// Set by background threads, picked up by the UI thread.
    pub popup_request: Option<String>,
    pub want_full: bool,
}

pub type Handle = Arc<Mutex<Shared>>;

pub fn any_running(g: &Shared) -> bool {
    g.jobs.values().any(|p| p.state == State::Running)
}

fn scan(root: &str, cameras: &[Camera], pick: Option<&str>) -> Card {
    let base = Path::new(root);
    let serial = camera::volume_serial(root);
    let found = camera::detect(base, cameras, pick);
    let cam = found.camera.clone();

    let mut items = Vec::new();
    let mut unsorted = BTreeMap::new();
    camera::walk_media(base, &cam.sources, &mut |p, md, ext| match cam.types.get(ext) {
        None => *unsorted.entry(ext.to_string()).or_insert(0) += 1,
        Some(ty) if ty.is_empty() => {}
        Some(ty) => {
            let Ok(mtime) = md.modified() else { return };
            items.push(Item {
                src: p.to_path_buf(),
                ty: ty.clone(),
                date: camera::shoot_date(&cam, p, mtime),
                mtime,
                size: md.len(),
            });
        }
    });

    items.sort_by(|a, b| a.src.cmp(&b.src));
    let mut counts = BTreeMap::new();
    for it in &items {
        *counts.entry(it.ty.clone()).or_insert(0) += 1;
    }
    Card {
        root: root.to_string(),
        serial,
        camera: cam,
        how: found.how,
        model: found.model,
        counts,
        unsorted,
        bytes: items.iter().map(|i| i.size).sum(),
        items: Arc::new(items),
    }
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
        let roots: Vec<String> = h.lock().unwrap().cards.iter().map(|c| c.root.clone()).collect();
        for root in roots {
            let (cams, pick) = {
                let g = h.lock().unwrap();
                if g.jobs.get(&root).is_some_and(|p| p.state == State::Running) {
                    continue;
                }
                let serial = camera::volume_serial(&root);
                (g.cameras.clone(), picked_for(&g, &root, serial.as_ref()))
            };
            let card = scan(&root, &cams, pick.as_deref());
            if let Some(slot) = h.lock().unwrap().cards.iter_mut().find(|c| c.root == root) {
                *slot = card;
            }
            ctx.request_repaint();
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

pub fn spawn_watcher(h: Handle, ctx: eframe::egui::Context) {
    thread::spawn(move || {
        let sys = std::env::var("SystemDrive")
            .ok()
            .and_then(|s| s.chars().next())
            .map_or('C', |c| c.to_ascii_uppercase());
        let mut known: HashSet<char> = HashSet::new();
        loop {
            let cams = h.lock().unwrap().cameras.clone();
            for l in 'D'..='Z' {
                if l == sys {
                    continue;
                }
                let root = format!("{l}:\\");
                let present = Path::new(&root).is_dir() && camera::looks_like_card(Path::new(&root), &cams);
                if present && known.insert(l) {
                    let pick = {
                        let g = h.lock().unwrap();
                        picked_for(&g, &root, camera::volume_serial(&root).as_ref())
                    };
                    let card = scan(&root, &cams, pick.as_deref());
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
                        let body = format!("{root}  ·  {}  ·  {}", card.camera.name, card.summary());
                        let (h2, ctx2, root2) = (h.clone(), ctx.clone(), root.clone());
                        toast("SWORTA – Card detected", &body, Some("Import"), move |import| {
                            show_window(&h2, &ctx2);
                            if import {
                                begin_import(&h2, &ctx2, &root2);
                            }
                        });
                    }
                    // An unknown camera waits for the user instead of being sorted by guesswork.
                    if auto && card.how != How::Unknown {
                        let dest = card.camera.dest.clone().unwrap_or_else(|| h.lock().unwrap().settings.dest.clone());
                        start_import(&h, &ctx, &root, dest);
                    }
                    ctx.request_repaint();
                } else if !present && known.remove(&l) {
                    let mut g = h.lock().unwrap();
                    g.cards.retain(|c| c.root != root);
                    g.picked.remove(&root);
                    ctx.request_repaint();
                }
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
            loop {
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(it) = card.items.get(i) else { break };
                if let Some(p) = h.lock().unwrap().jobs.get_mut(&root) {
                    p.current = it
                        .src
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
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
    let name = it.src.file_name().unwrap().to_string_lossy().into_owned();
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
    let mut src = fs::File::open(&it.src)?;
    let mtime = src.metadata()?.modified().ok();
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
        let (ask, last, fixed) = {
            let g = h.lock().unwrap();
            let fixed = g.cards.iter().find(|c| c.root == root).and_then(|c| c.camera.dest.clone());
            (g.settings.ask_dest, g.settings.dest.clone(), fixed)
        };
        let dest = if let Some(d) = fixed {
            d
        } else if ask {
            let start = if last.exists() { last.clone() } else { last.parent().map(Path::to_path_buf).unwrap_or(last.clone()) };
            let picked = rfd::FileDialog::new()
                .set_title(format!("Destination folder for {root}"))
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
