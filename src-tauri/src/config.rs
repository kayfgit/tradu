use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

use crate::deepl::Deepl;

/// User-editable settings, stored as JSON in the app's config dir.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Global shortcut that toggles the popup, e.g. "Ctrl+Shift+Space" or "Alt+T".
    pub hotkey: String,
    /// Where focus lands when the popup opens: "input" (keep last languages) or
    /// "source" (pick source language, then target, then type).
    pub start_in: String,
    /// Hide the popup as soon as it loses focus.
    pub hide_on_blur: bool,
    /// Pause after the last keystroke before translating.
    pub debounce_ms: u64,
    /// "google": the free endpoint, with DeepL (if set up) while Google is
    /// rate-limiting. "deepl": always DeepL.
    pub translator: String,
    /// DeepL API key, usually set with `:deepl KEY`.
    pub deepl_key: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: "Ctrl+Shift+Space".into(),
            start_in: "input".into(),
            hide_on_blur: true,
            debounce_ms: 400,
            translator: "google".into(),
            deepl_key: String::new(),
        }
    }
}

impl Config {
    pub fn deepl(&self) -> Option<Deepl> {
        (!self.deepl_key.is_empty()).then(|| Deepl {
            key: self.deepl_key.clone(),
            always: self.translator == "deepl",
        })
    }
}

/// v0.1.0's default: so short that normal typing rhythm sent a request almost
/// every word. Files still holding it are moved to the new default.
const OLD_DEFAULT_DEBOUNCE_MS: u64 = 200;

pub fn path(app: &AppHandle) -> PathBuf {
    app.path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("config.json")
}

/// Reads the config, writing the defaults on first run so there's a file to edit.
pub fn load(app: &AppHandle) -> Config {
    let path = path(app);
    match std::fs::read_to_string(&path) {
        Ok(s) => {
            let mut config: Config = serde_json::from_str(&s).unwrap_or_else(|e| {
                eprintln!("Invalid {}: {e}; using defaults", path.display());
                Config::default()
            });
            if config.debounce_ms == OLD_DEFAULT_DEBOUNCE_MS {
                config.debounce_ms = Config::default().debounce_ms;
                let _ = std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap());
            }
            config
        }
        Err(_) => {
            let config = Config::default();
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap());
            config
        }
    }
}

pub fn save(app: &AppHandle, config: &Config) -> std::io::Result<()> {
    let path = path(app);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(config).unwrap())
}

pub fn open_in_editor(app: &AppHandle) {
    let path = path(app);
    #[cfg(windows)]
    let cmd = std::process::Command::new("notepad").arg(&path).spawn();
    #[cfg(target_os = "macos")]
    let cmd = std::process::Command::new("open").args(["-t"]).arg(&path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = std::process::Command::new("xdg-open").arg(&path).spawn();
    if let Err(e) = cmd {
        eprintln!("Couldn't open {}: {e}", path.display());
    }
}
