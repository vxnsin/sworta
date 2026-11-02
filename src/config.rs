use serde::{Deserialize, Deserializer, Serialize};
use std::{collections::HashMap, fs, path::PathBuf};
use winreg::{enums::*, RegKey};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_NAME: &str = "SWORTA";

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
pub enum Notify {
    Popup,
    Toast,
    Off,
}

/// Older versions stored a plain bool here.
fn de_notify<'de, D: Deserializer<'de>>(d: D) -> Result<Notify, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum V {
        Flag(bool),
        Mode(Notify),
    }
    Ok(match V::deserialize(d)? {
        V::Flag(true) => Notify::Popup,
        V::Flag(false) => Notify::Off,
        V::Mode(m) => m,
    })
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    pub dest: PathBuf,
    pub autostart: bool,
    #[serde(deserialize_with = "de_notify")]
    pub notify: Notify,
    pub auto_update: bool,
    pub auto_import: bool,
    /// Ask for the destination folder on every import; `dest` is the last one used.
    pub ask_dest: bool,
    pub open_when_done: bool,
    pub long_date: bool,
    /// Files copied at the same time per card.
    pub parallel: u32,
    /// MB/s over all running copies, 0 = unlimited.
    pub speed_limit: u32,
    /// Seconds the popup stays, 0 = until dismissed.
    pub popup_secs: u32,
    /// Volume serial of a card → camera the user picked for it.
    pub cards: HashMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        let home = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into());
        Self {
            dest: PathBuf::from(home).join("Pictures").join("SWORTA"),
            autostart: true,
            notify: Notify::Popup,
            auto_update: true,
            auto_import: false,
            ask_dest: true,
            open_when_done: false,
            long_date: false,
            parallel: 2,
            speed_limit: 0,
            popup_secs: 12,
            cards: HashMap::new(),
        }
    }
}

fn file() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("SWORTA").join("settings.json")
}

pub fn load() -> Settings {
    fs::read_to_string(file())
        .ok()
        .and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default()
}

pub fn save(s: &Settings) {
    let path = file();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(s) {
        let _ = fs::write(path, json);
    }
}

pub fn set_autostart(on: bool) {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(key) = hkcu.open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE) else {
        return;
    };
    if on {
        if let Ok(exe) = std::env::current_exe() {
            let _ = key.set_value(RUN_NAME, &format!("\"{}\" --tray", exe.display()));
        }
    } else {
        let _ = key.delete_value(RUN_NAME);
    }
}
