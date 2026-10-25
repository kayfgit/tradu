#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod azure;
mod config;
mod origin;
mod romaji;
mod translate;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use config::Config;
use serde::Serialize;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, PhysicalPosition, State, WebviewWindow, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

struct AppState {
    translator: translate::Translator,
    config: Mutex<Config>,
    hotkey_error: Mutex<Option<String>>,
    // Kept alive for the whole session: on Linux the clipboard empties when its owner drops.
    clipboard: Mutex<Option<arboard::Clipboard>>,
    previous_window: Mutex<origin::Target>,
}

/// How long the popup waits for the app's selection before opening without it.
/// A selection that arrives later is still delivered, until the user starts typing.
const SELECTION_WAIT: Duration = Duration::from_millis(40);
const SELECTION_LATE: Duration = Duration::from_millis(300);

#[derive(Serialize)]
struct UiConfig {
    hotkey: String,
    hotkey_error: Option<String>,
    start_in: String,
    debounce_ms: u64,
    /// Whether an Azure key is set up.
    azure: bool,
}

#[tauri::command]
async fn translate(
    state: State<'_, AppState>,
    text: String,
    from: String,
    to: String,
    pair: Option<Vec<String>>,
    as_typed: Option<bool>,
) -> Result<translate::Translation, translate::Error> {
    let pair = pair.unwrap_or_default();
    let request = translate::Request {
        text: &text,
        from: &from,
        to: &to,
        pair: &pair,
        as_typed: as_typed.unwrap_or(false),
    };
    state.translator.translate(request).await
}

#[tauri::command]
fn ui_config(state: State<'_, AppState>) -> UiConfig {
    let config = state.config.lock().unwrap();
    UiConfig {
        hotkey: config.hotkey.clone(),
        hotkey_error: state.hotkey_error.lock().unwrap().clone(),
        start_in: config.start_in.clone(),
        debounce_ms: config.debounce_ms,
        azure: !config.azure_key.is_empty(),
    }
}

/// Saves an Azure key after checking it works; an empty key removes it.
#[tauri::command]
async fn set_azure(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    region: String,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap().clone();
    config.azure_key = key.trim().to_owned();
    config.azure_region = region.trim().to_owned();
    if let Some(azure) = config.azure() {
        state.translator.check_azure(&azure).await.map_err(|e| e.message)?;
    }
    config::save(&app, &config).map_err(|e| format!("Couldn't save the config: {e}"))?;
    state.translator.set_azure(config.azure());
    *state.config.lock().unwrap() = config;
    Ok(())
}

#[tauri::command]
fn copy(state: State<'_, AppState>, text: String) -> Result<(), String> {
    set_clipboard(&state, text)
}

#[tauri::command]
fn hide(window: WebviewWindow) {
    let _ = window.hide();
}

/// Copies `text`, hides the popup and pastes into the window that was active before it.
#[tauri::command]
fn paste(window: WebviewWindow, state: State<'_, AppState>, text: String) -> Result<(), String> {
    set_clipboard(&state, text)?;
    let _ = window.hide();
    let target = *state.previous_window.lock().unwrap();
    // Off the main thread: the hide above is dispatched to it and must run first.
    std::thread::spawn(move || origin::paste_into(target));
    Ok(())
}

fn with_clipboard<T>(
    state: &AppState,
    f: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>,
) -> Result<T, String> {
    let mut clipboard = state.clipboard.lock().unwrap();
    if clipboard.is_none() {
        *clipboard = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
    }
    f(clipboard.as_mut().unwrap()).map_err(|e| e.to_string())
}

fn set_clipboard(state: &AppState, text: String) -> Result<(), String> {
    with_clipboard(state, |c| c.set_text(text))
}

fn popup_in_front(app: &AppHandle) -> bool {
    app.get_webview_window("main")
        .is_some_and(|w| w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false))
}

/// Tray and second-launch handler: hides the popup if it's in front, otherwise opens it.
fn toggle(app: &AppHandle) {
    if popup_in_front(app) {
        hide_popup(app);
    } else {
        show(app, None);
    }
}

/// Hotkey handler: like `toggle`, but text selected in the current app is
/// copied into the popup and translated straight away.
fn summon_from_hotkey(app: &AppHandle) {
    if popup_in_front(app) {
        return hide_popup(app);
    }
    let app = app.clone();
    // Off the main thread: waiting on the other app must not block the event loop.
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let previous = with_clipboard(&state, |c| c.get_text()).ok();
        let start = Instant::now();
        let request = origin::request_copy();
        if request.copied_by(start + SELECTION_WAIT) {
            let selection = take_selection(&state, previous);
            return show(&app, selection);
        }
        show(&app, None);
        if request.copied_by(start + SELECTION_LATE) {
            if let Some(selection) = take_selection(&state, previous) {
                let _ = app.emit("selection", selection);
            }
        }
    });
}

/// Reads the selection the app just copied and puts the user's clipboard back.
fn take_selection(state: &AppState, previous: Option<String>) -> Option<String> {
    let selection = with_clipboard(state, |c| c.get_text())
        .ok()
        .filter(|t| !t.trim().is_empty());
    if let Some(previous) = previous {
        let _ = set_clipboard(state, previous);
    }
    selection
}

fn hide_popup(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

/// Shows the popup on the monitor under the mouse, optionally prefilled.
fn show(app: &AppHandle, selection: Option<String>) {
    let Some(window) = app.get_webview_window("main") else { return };
    let state = app.state::<AppState>();
    *state.previous_window.lock().unwrap() = origin::foreground();

    place_on_cursor_monitor(&window);
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
    let _ = window.emit("summon", selection);
}

fn place_on_cursor_monitor(window: &WebviewWindow) {
    let placed = (|| {
        let cursor = window.cursor_position().ok()?;
        let monitor = window.monitor_from_point(cursor.x, cursor.y).ok()??;
        let size = window.outer_size().ok()?;
        let (origin, area) = (monitor.position(), monitor.size());
        let x = origin.x + (area.width as i32 - size.width as i32) / 2;
        let y = origin.y + (area.height as i32 - size.height as i32) / 3;
        window.set_position(PhysicalPosition::new(x, y)).ok()
    })();
    if placed.is_none() {
        let _ = window.center();
    }
}

fn register_hotkey(app: &AppHandle) {
    let state = app.state::<AppState>();
    let hotkey = state.config.lock().unwrap().hotkey.clone();
    let shortcuts = app.global_shortcut();
    let _ = shortcuts.unregister_all();
    let error = shortcuts
        .register(hotkey.as_str())
        .err()
        .map(|e| format!("Couldn't register hotkey \"{hotkey}\": {e}"));
    if let Some(e) = &error {
        eprintln!("{e}");
    }
    *state.hotkey_error.lock().unwrap() = error;
}

fn reload_config(app: &AppHandle) {
    let state = app.state::<AppState>();
    let config = config::load(app);
    state.translator.set_azure(config.azure());
    *state.config.lock().unwrap() = config;
    register_hotkey(app);
    let _ = app.emit("config-changed", ());
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let autostart_on = app.autolaunch().is_enabled().unwrap_or(false);
    let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let autostart =
        CheckMenuItem::with_id(app, "autostart", "Start at login", true, autostart_on, None::<&str>)?;
    let edit = MenuItem::with_id(app, "edit", "Edit config", true, None::<&str>)?;
    let reload = MenuItem::with_id(app, "reload", "Reload config", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &show,
            &autostart,
            &PredefinedMenuItem::separator(app)?,
            &edit,
            &reload,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let autostart_item = autostart.clone();
    TrayIconBuilder::with_id("tray")
        .icon(app.default_window_icon().cloned().expect("bundled icon"))
        .tooltip("tradu")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "show" => toggle(app),
            "autostart" => {
                let launcher = app.autolaunch();
                let _ = if launcher.is_enabled().unwrap_or(false) {
                    launcher.disable()
                } else {
                    launcher.enable()
                };
                let _ = autostart_item.set_checked(launcher.is_enabled().unwrap_or(false));
            }
            "edit" => config::open_in_editor(app),
            "reload" => reload_config(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        // Launching a second copy just summons the running one.
        .plugin(tauri_plugin_single_instance::init(|app, _, _| toggle(app)))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _, event| {
                    if event.state() == ShortcutState::Pressed {
                        summon_from_hotkey(app);
                    }
                })
                .build(),
        )
        .setup(|app| {
            let handle = app.handle();
            let config = config::load(handle);
            let translator = translate::Translator::new();
            translator.set_azure(config.azure());
            app.manage(AppState {
                translator,
                config: Mutex::new(config),
                hotkey_error: Mutex::new(None),
                clipboard: Mutex::new(None),
                previous_window: Mutex::new(0),
            });
            register_hotkey(handle);
            build_tray(handle)?;

            let window = app.get_webview_window("main").expect("main window");
            let w = window.clone();
            window.on_window_event(move |event| match event {
                WindowEvent::Focused(false) => {
                    let hide_on_blur = w.state::<AppState>().config.lock().unwrap().hide_on_blur;
                    if hide_on_blur {
                        let _ = w.hide();
                    }
                }
                WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    let _ = w.hide();
                }
                _ => {}
            });

            // Without a working hotkey the app would be unreachable, so say so up front.
            if app.state::<AppState>().hotkey_error.lock().unwrap().is_some() {
                toggle(handle);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![translate, ui_config, set_azure, copy, hide, paste])
        .run(tauri::generate_context!())
        .expect("error while running tradu");
}
