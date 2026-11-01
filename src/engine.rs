use chrono::{DateTime, Local};
use std::{
    collections::HashSet,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use crate::config::Settings;

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
    pub date: String,
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
}

pub struct Shared {
    pub cards: Vec<Card>,
    pub progress: Progress,
    pub settings: Settings,
    pub update: crate::update::Update,
}

pub type Handle = Arc<Mutex<Shared>>;

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

fn scan(root: &str) -> Card {
    let mut items = Vec::new();
    let base = Path::new(root);
    let mut add = |kind: Kind, p: &Path, md: &fs::Metadata| {
        let Ok(mt) = md.modified() else { return };
        let dt: DateTime<Local> = mt.into();
        items.push(Item {
            src: p.to_path_buf(),
            kind,
            date: dt.format("%y%m%d").to_string(),
            size: md.len(),
        });
    };

    walk(&base.join("DCIM"), &mut |p, md| {
        match ext(p).as_str() {
            "jpg" | "jpeg" => add(Kind::Jpg, p, md),
            "arw" => add(Kind::Raw, p, md),
            _ => {}
        }
    });
    walk(
        &base.join("PRIVATE").join("M4ROOT").join("CLIP"),
        &mut |p, md| {
            if matches!(ext(p).as_str(), "mp4" | "xml") {
                add(Kind::Vid, p, md);
            }
        },
    );

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

fn ext(p: &Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

pub fn show_window(ctx: &eframe::egui::Context) {
    use eframe::egui::ViewportCommand as C;
    ctx.send_viewport_cmd(C::Visible(true));
    ctx.send_viewport_cmd(C::Minimized(false));
    ctx.send_viewport_cmd(C::Focus);
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
            on_click(action.map_or(false, |a| a.contains("import")));
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
                    let notify = {
                        let mut g = h.lock().unwrap();
                        g.cards.push(card.clone());
                        g.settings.notify
                    };
                    if notify {
                        let body = format!(
                            "{root}  ·  {} JPG  ·  {} RAW  ·  {} VID",
                            card.jpg, card.raw, card.vid
                        );
                        let (h2, ctx2, root2) = (h.clone(), ctx.clone(), root.clone());
                        toast("SWORTA – Karte erkannt", &body, Some("Importieren"), move |import| {
                            show_window(&ctx2);
                            if import {
                                start_import(&h2, &ctx2, &root2);
                            }
                        });
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

pub fn start_import(h: &Handle, ctx: &eframe::egui::Context, root: &str) {
    let (card, dest, notify) = {
        let mut g = h.lock().unwrap();
        if g.progress.state == State::Running {
            return;
        }
        let Some(card) = g.cards.iter().find(|c| c.root == root).cloned() else {
            return;
        };
        g.progress = Progress {
            state: State::Running,
            files_total: card.items.len(),
            bytes_total: card.bytes,
            ..Default::default()
        };
        (card, g.settings.dest.clone(), g.settings.notify)
    };

    let (h, ctx) = (h.clone(), ctx.clone());
    thread::spawn(move || {
        for it in card.items.iter() {
            {
                let mut g = h.lock().unwrap();
                g.progress.current = it
                    .src
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
            }
            let dir = dest.join(&it.date).join(it.kind.dir());
            let res = copy_one(it, &dir, &mut |n| {
                h.lock().unwrap().progress.bytes_done += n;
                ctx.request_repaint();
            });
            let mut g = h.lock().unwrap();
            g.progress.files_done += 1;
            match res {
                Ok(true) => g.progress.copied += 1,
                Ok(false) => {
                    g.progress.skipped += 1;
                    g.progress.bytes_done += it.size;
                }
                Err(_) => g.progress.failed += 1,
            }
            drop(g);
            ctx.request_repaint();
        }

        let p = {
            let mut g = h.lock().unwrap();
            g.progress.state = State::Done;
            g.progress.current.clear();
            g.progress.clone()
        };
        ctx.request_repaint();
        if notify {
            let body = format!(
                "{} kopiert  ·  {} übersprungen  ·  {} Fehler",
                p.copied, p.skipped, p.failed
            );
            let ctx2 = ctx.clone();
            toast("SWORTA – Fertig", &body, None, move |_| show_window(&ctx2));
        }
    });
}

/// Ok(true) = copied, Ok(false) = already present (skipped).
fn copy_one(it: &Item, dir: &Path, on_bytes: &mut dyn FnMut(u64)) -> std::io::Result<bool> {
    fs::create_dir_all(dir)?;
    let name = it.src.file_name().unwrap().to_string_lossy().into_owned();
    let (stem, extn) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (name.clone(), String::new()),
    };

    let mut target = dir.join(&name);
    let mut n = 0;
    while let Ok(md) = fs::metadata(&target) {
        if md.len() == it.size {
            return Ok(false);
        }
        n += 1;
        target = dir.join(format!("{stem}_{n}{extn}"));
    }

    let part = dir.join(format!("{}.part", target.file_name().unwrap().to_string_lossy()));
    let mut src = fs::File::open(&it.src)?;
    let mtime = src.metadata()?.modified().ok();
    let mut out = fs::File::create(&part)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let r = src.read(&mut buf)?;
        if r == 0 {
            break;
        }
        out.write_all(&buf[..r])?;
        on_bytes(r as u64);
    }
    out.flush()?;
    if let Some(t) = mtime {
        let _ = out.set_modified(t);
    }
    drop(out);
    fs::rename(&part, &target)?;
    Ok(true)
}
