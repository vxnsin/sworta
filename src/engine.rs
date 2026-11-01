use chrono::{DateTime, Local};
use std::{
    collections::{HashMap, HashSet},
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

use crate::config::{Notify, Settings};

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Jpg,
    Raw,
    Vid,
}

impl Kind {
    pub fn dir(self) -> &'static str {
        match self {
            Kind::Jpg => "JPG",
            Kind::Raw => "RAW",
            Kind::Vid => "VID",
        }
    }
}

#[derive(Clone)]
pub struct Item {
    pub src: PathBuf,
    pub kind: Kind,
    pub mtime: SystemTime,
    pub size: u64,
}

#[derive(Clone)]
pub struct Card {
    pub root: String,
    pub items: Arc<Vec<Item>>,
    pub jpg: usize,
    pub raw: usize,
    pub vid: usize,
    pub bytes: u64,
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
    pub update: crate::update::Update,
    /// Set by background threads, picked up by the UI thread.
    pub popup_request: Option<String>,
    pub want_full: bool,
}

pub type Handle = Arc<Mutex<Shared>>;

pub fn any_running(g: &Shared) -> bool {
    g.jobs.values().any(|p| p.state == State::Running)
}

fn walk(dir: &Path, f: &mut dyn FnMut(&Path, &fs::Metadata)) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(md) = e.metadata() else { continue };
        if md.is_dir() {
            walk(&p, f);
        } else {
            f(&p, &md);
        }
    }
}

fn ext(p: &Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn scan(root: &str) -> Card {
    let mut items = Vec::new();
    let base = Path::new(root);
    let mut add = |kind: Kind, p: &Path, md: &fs::Metadata| {
        let Ok(mtime) = md.modified() else { return };
        items.push(Item { src: p.to_path_buf(), kind, mtime, size: md.len() });
    };

    walk(&base.join("DCIM"), &mut |p, md| match ext(p).as_str() {
        "jpg" | "jpeg" => add(Kind::Jpg, p, md),
        "arw" => add(Kind::Raw, p, md),
        _ => {}
    });
    walk(&base.join("PRIVATE").join("M4ROOT").join("CLIP"), &mut |p, md| {
        if matches!(ext(p).as_str(), "mp4" | "xml") {
            add(Kind::Vid, p, md);
        }
    });

    items.sort_by(|a, b| a.src.cmp(&b.src));
    let count = |k: Kind| items.iter().filter(|i| i.kind == k).count();
    Card {
        root: root.to_string(),
        jpg: count(Kind::Jpg),
        raw: count(Kind::Raw),
        vid: count(Kind::Vid),
        bytes: items.iter().map(|i| i.size).sum(),
        items: Arc::new(items),
    }
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
            for l in 'D'..='Z' {
                if l == sys {
                    continue;
                }
                let root = format!("{l}:\\");
                let present = Path::new(&format!("{root}DCIM")).is_dir();
                if present && known.insert(l) {
                    let card = scan(&root);
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
                        let body = format!(
                            "{root}  ·  {} JPG  ·  {} RAW  ·  {} VID",
                            card.jpg, card.raw, card.vid
                        );
                        let (h2, ctx2, root2) = (h.clone(), ctx.clone(), root.clone());
                        toast("SWORTA – Card detected", &body, Some("Import"), move |import| {
                            show_window(&h2, &ctx2);
                            if import {
                                begin_import(&h2, &ctx2, &root2);
                            }
                        });
                    }
                    if auto {
                        let dest = h.lock().unwrap().settings.dest.clone();
                        start_import(&h, &ctx, &root, dest);
                    }
                    ctx.request_repaint();
                } else if !present && known.remove(&l) {
                    h.lock().unwrap().cards.retain(|c| c.root != root);
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
                let dt: DateTime<Local> = it.mtime.into();
                let date = dt.format(if long_date { "%Y-%m-%d" } else { "%y%m%d" }).to_string();
                let dir = dest.join(date).join(it.kind.dir());

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

/// Starts an import, asking for the destination first when that setting is on.
pub fn begin_import(h: &Handle, ctx: &eframe::egui::Context, root: &str) {
    let (h, ctx, root) = (h.clone(), ctx.clone(), root.to_string());
    thread::spawn(move || {
        let (ask, last) = {
            let g = h.lock().unwrap();
            (g.settings.ask_dest, g.settings.dest.clone())
        };
        let dest = if ask {
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
