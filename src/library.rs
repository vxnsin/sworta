//! The camera library: every file in the repo's `cameras/<category>/`, fetched from GitHub.

use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, thread};

use crate::{
    camera::{self, Camera},
    engine::{self, Handle},
};

const BRANCH: &str = "master";

/// Folders below `cameras/`, in the order the app shows them.
pub const CATEGORIES: [&str; 6] = ["action", "mirrorless", "cinema", "drone", "360", "phone"];

#[derive(Serialize, Deserialize, Clone)]
pub struct Entry {
    pub category: String,
    pub camera: Camera,
}

#[derive(Clone, Default, PartialEq)]
pub enum Status {
    #[default]
    Idle,
    Loading,
    Ready,
    /// GitHub could not be reached; the saved list is shown.
    Offline(String),
}

#[derive(Clone, Default)]
pub struct Library {
    pub entries: Vec<Entry>,
    pub status: Status,
}

impl Library {
    /// Library cameras the user does not have yet.
    pub fn missing(&self, mine: &[Camera]) -> Vec<Camera> {
        self.entries
            .iter()
            .filter(|e| !mine.iter().any(|c| c.id == e.camera.id))
            .map(|e| e.camera.clone())
            .collect()
    }
}

fn cache_file() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("SWORTA").join("library.json")
}

pub fn load_cache() -> Library {
    let entries = fs::read_to_string(cache_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    Library { entries, status: Status::Idle }
}

fn save_cache(entries: &[Entry]) {
    if let Some(dir) = cache_file().parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string(entries) {
        let _ = fs::write(cache_file(), json);
    }
}

fn get(url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    ureq::get(url)
        .header("User-Agent", "SWORTA")
        .call()
        .map_err(|e| e.to_string())
}

/// `cameras/action/gopro-hero.json` → `action`.
fn category_of(path: &str) -> Option<&str> {
    let mut parts = path.split('/');
    let (Some("cameras"), Some(cat), Some(file), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return None;
    };
    file.ends_with(".json").then_some(cat)
}

fn order(cat: &str) -> usize {
    CATEGORIES.iter().position(|c| *c == cat).unwrap_or(CATEGORIES.len())
}

fn fetch() -> Result<Vec<Entry>, String> {
    let url = format!("https://api.github.com/repos/{}/git/trees/{BRANCH}?recursive=1", crate::update::REPO);
    let tree: serde_json::Value = get(&url)?.body_mut().read_json().map_err(|e| e.to_string())?;
    let paths: Vec<String> = tree["tree"]
        .as_array()
        .ok_or("GitHub sent no file list")?
        .iter()
        .filter(|f| f["type"] == "blob")
        .filter_map(|f| f["path"].as_str())
        .filter(|p| category_of(p).is_some())
        .map(str::to_string)
        .collect();

    let mut entries = Vec::new();
    for path in paths {
        let raw = format!("https://raw.githubusercontent.com/{}/{BRANCH}/{path}", crate::update::REPO);
        let Ok(text) = get(&raw).and_then(|mut r| r.body_mut().read_to_string().map_err(|e| e.to_string())) else {
            continue;
        };
        // One broken file must not hide the rest of the library.
        if let Ok(camera) = camera::parse(&text) {
            let category = category_of(&path).unwrap_or_default().to_string();
            entries.push(Entry { category, camera });
        }
    }
    if entries.is_empty() {
        return Err("the library is empty".into());
    }
    entries.sort_by(|a, b| {
        (order(&a.category), a.camera.name.to_lowercase()).cmp(&(order(&b.category), b.camera.name.to_lowercase()))
    });
    Ok(entries)
}

/// Loads the library in the background and sorts the inserted cards again with it.
pub fn refresh(h: &Handle, ctx: &eframe::egui::Context) {
    {
        let mut g = h.lock().unwrap();
        if g.library.status == Status::Loading {
            return;
        }
        g.library.status = Status::Loading;
    }
    ctx.request_repaint();
    let (h, ctx) = (h.clone(), ctx.clone());
    thread::spawn(move || {
        match fetch() {
            Ok(entries) => {
                save_cache(&entries);
                let mut g = h.lock().unwrap();
                g.library = Library { entries, status: Status::Ready };
                restore_picked(&mut g);
            }
            Err(e) => h.lock().unwrap().library.status = Status::Offline(e),
        }
        engine::rescan_all(&h, &ctx);
        ctx.request_repaint();
    });
}

/// Cards remembered for a camera the user no longer has locally (it used to be built in)
/// get that camera back from the library.
fn restore_picked(g: &mut engine::Shared) {
    let wanted: Vec<String> = g.settings.cards.values().cloned().collect();
    let mut added = false;
    for c in g.library.missing(&g.cameras) {
        if wanted.contains(&c.id) && camera::save(&c).is_ok() {
            added = true;
        }
    }
    if added {
        g.cameras = camera::load_all();
    }
}

/// Adds a library camera to the user's cameras.
pub fn install(h: &Handle, c: &Camera) -> Result<(), String> {
    camera::save(c).map_err(|e| format!("Could not save the camera ({e})."))?;
    h.lock().unwrap().cameras = camera::load_all();
    Ok(())
}

pub fn matches(e: &Entry, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    let c = &e.camera;
    [&c.name, &c.id, &e.category]
        .into_iter()
        .chain(c.models.iter())
        .any(|s| s.to_lowercase().contains(&q))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_category_files_count() {
        assert_eq!(category_of("cameras/action/gopro-hero.json"), Some("action"));
        assert_eq!(category_of("cameras/README.md"), None);
        assert_eq!(category_of("cameras/gopro.json"), None);
        assert_eq!(category_of("cameras/a/b/c.json"), None);
        assert_eq!(category_of("src/fallback.json"), None);
    }

    #[test]
    fn search_looks_at_name_model_and_category() {
        let e = Entry {
            category: "mirrorless".into(),
            camera: Camera { id: "sony-alpha".into(), name: "Sony Alpha".into(), models: vec!["ILCE-".into()], ..Default::default() },
        };
        assert!(matches(&e, ""));
        assert!(matches(&e, "sony"));
        assert!(matches(&e, "ilce"));
        assert!(matches(&e, "MIRROR"));
        assert!(!matches(&e, "gopro"));
    }
}
