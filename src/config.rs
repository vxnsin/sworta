use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
use winreg::{enums::*, RegKey};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_NAME: &str = "SWORTA";

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    pub dest: PathBuf,
    pub autostart: bool,
    pub notify: bool,
    pub auto_update: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let home = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into());
        Self {
            dest: PathBuf::from(home).join("Pictures").join("SWORTA"),
            autostart: true,
            notify: true,
            auto_update: true,
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
        .and_then(|s| serde_json::from_str(&s).ok())
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
