use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: "Ctrl+Shift+Space".into(),
            start_in: "input".into(),
            hide_on_blur: true,
            debounce_ms: 200,
        }
    }
}

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
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            eprintln!("Invalid {}: {e}; using defaults", path.display());
            Config::default()
        }),
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
