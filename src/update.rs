use std::{
    fs,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use crate::engine::{self, Handle, State};

const REPO: &str = "vxnsin/sworta";
const ASSET: &str = "SWORTA.exe";
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);

/// Set while the app shuts down for real (tray quit or update) so the close
/// handler does not just hide the window.
pub static QUITTING: AtomicBool = AtomicBool::new(false);
/// Some(hidden) once an update is on disk and the new exe should be launched.
pub static RESTART: Mutex<Option<bool>> = Mutex::new(None);

#[derive(Clone, PartialEq)]
pub struct Release {
    pub tag: String,
    pub url: String,
}

#[derive(Clone, Default, PartialEq)]
pub enum Update {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Installing,
    Failed(String),
}

fn parse(v: &str) -> Option<(u32, u32, u32)> {
    let mut it = v.trim().trim_start_matches('v').split('.');
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

fn current_exe() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

/// Leftover from the previous version after an update.
pub fn cleanup_old() {
    let Some(exe) = current_exe() else { return };
    let old = exe.with_extension("old");
    // The previous process may still be shutting down and hold the file.
    thread::spawn(move || {
        for _ in 0..30 {
            if !old.exists() || fs::remove_file(&old).is_ok() {
                return;
            }
            thread::sleep(Duration::from_secs(1));
        }
    });
}

fn fetch_latest() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut resp = match ureq::get(&url)
        .header("User-Agent", "SWORTA")
        .header("Accept", "application/vnd.github+json")
        .call()
    {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let v: serde_json::Value = resp.body_mut().read_json().map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().ok_or("kein tag_name")?.to_string();
    if parse(&tag) <= parse(env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let url = v["assets"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["name"] == ASSET))
        .and_then(|x| x["browser_download_url"].as_str())
        .ok_or("Release enthält keine SWORTA.exe")?
        .to_string();
    Ok(Some(Release { tag, url }))
}

fn download_and_swap(url: &str) -> Result<(), String> {
    let mut resp = ureq::get(url)
        .header("User-Agent", "SWORTA")
        .call()
        .map_err(|e| e.to_string())?;
    let bytes = resp
        .body_mut()
        .with_config()
        .limit(100 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    if bytes.len() < 1_000_000 || &bytes[..2] != b"MZ" {
        return Err("Download ist keine gültige Exe".into());
    }

    let exe = current_exe().ok_or("Exe-Pfad unbekannt")?;
    let old = exe.with_extension("old");
    let new = exe.with_extension("new");
    fs::write(&new, &bytes).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(&old);
    // A running exe can be renamed but not overwritten.
    fs::rename(&exe, &old).map_err(|e| e.to_string())?;
    if let Err(e) = fs::rename(&new, &exe) {
        let _ = fs::rename(&old, &exe);
        return Err(e.to_string());
    }
    Ok(())
}

fn set(h: &Handle, ctx: &eframe::egui::Context, u: Update) {
    h.lock().unwrap().update = u;
    ctx.request_repaint();
}

pub fn quit(ctx: &eframe::egui::Context) {
    QUITTING.store(true, Ordering::SeqCst);
    ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Close);
    thread::spawn(|| {
        thread::sleep(Duration::from_secs(3));
        std::process::exit(0);
    });
}

pub fn install(h: &Handle, ctx: &eframe::egui::Context, release: Release, silent: bool) {
    set(h, ctx, Update::Installing);
    let (h, ctx) = (h.clone(), ctx.clone());
    thread::spawn(move || match download_and_swap(&release.url) {
        Ok(()) => {
            log("update installed, restarting");
            *RESTART.lock().unwrap() = Some(silent);
            quit(&ctx);
        }
        Err(e) => {
            log(&format!("install failed: {e}"));
            set(&h, &ctx, Update::Failed(e))
        }
    });
}

/// Checks GitHub once; installs right away when `auto` is on and no import runs.
pub fn check(h: &Handle, ctx: &eframe::egui::Context, user_started: bool) {
    set(h, ctx, Update::Checking);
    log("check start");
    match fetch_latest() {
        Ok(Some(rel)) => {
            log(&format!("found {}", rel.tag));
            let (auto, busy) = {
                let g = h.lock().unwrap();
                (g.settings.auto_update, g.progress.state == State::Running)
            };
            if auto && !busy && !user_started {
                install(h, ctx, rel, true);
                return;
            }
            let already = matches!(&h.lock().unwrap().update, Update::Available(r) if *r == rel);
            set(h, ctx, Update::Available(rel.clone()));
            if !already && h.lock().unwrap().settings.notify && !user_started {
                let ctx2 = ctx.clone();
                engine::toast(
                    "SWORTA – Update verfügbar",
                    &format!("Version {} steht bereit.", rel.tag),
                    None,
                    move |_| engine::show_window(&ctx2),
                );
            }
        }
        Ok(None) => {
            log("up to date");
            set(h, ctx, Update::UpToDate)
        }
        Err(e) => {
            log(&format!("check failed: {e}"));
            set(h, ctx, if user_started { Update::Failed(e) } else { Update::Idle })
        }
    }
}

pub fn spawn_checker(h: Handle, ctx: eframe::egui::Context) {
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(15));
        loop {
            check(&h, &ctx, false);
            thread::sleep(CHECK_EVERY);
        }
    });
}

fn log(msg: &str) {
    use std::io::Write;
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    let dir = PathBuf::from(base).join("SWORTA");
    let _ = fs::create_dir_all(&dir);
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(dir.join("update.log")) {
        let _ = writeln!(f, "{} {msg}", chrono::Local::now().format("%F %T"));
    }
}
