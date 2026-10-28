#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod deepl;
#[cfg(windows)]
mod ocr;
mod origin;
#[cfg(windows)]
mod paint;
mod romaji;
mod translate;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use config::Config;
use serde::Serialize;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, State, WebviewWindow, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

struct AppState {
    translator: translate::Translator,
    config: Mutex<Config>,
    hotkey_error: Mutex<Option<String>>,
    /// The paint hotkey as registered, to tell it apart from the main one.
    paint_shortcut: Mutex<Option<Shortcut>>,
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
    paint_hotkey: String,
    ocr_language: String,
    start_in: String,
    debounce_ms: u64,
    /// Whether a DeepL key is set up.
    deepl: bool,
    /// "auto", "google" or "deepl".
    provider: String,
}

#[derive(Serialize)]
struct Limits {
    /// Set while Google is rate-limiting: how long its cooldown has left.
    google_retry_ms: Option<u64>,
    deepl: Option<deepl::Usage>,
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
        paint_hotkey: config.paint_hotkey.clone(),
        ocr_language: config.ocr_language.clone(),
        start_in: config.start_in.clone(),
        debounce_ms: config.debounce_ms,
        deepl: !config.deepl_key.is_empty(),
        provider: config.provider.clone(),
    }
}

/// What the top-right indicator shows: Google's cooldown and DeepL's quota.
#[tauri::command]
async fn limits(state: State<'_, AppState>) -> Result<Limits, ()> {
    Ok(Limits {
        google_retry_ms: state.translator.google_cooldown().map(|d| d.as_millis() as u64),
        deepl: state.translator.deepl_quota().await,
    })
}

/// Switches between "auto", "google" and "deepl", and saves the choice.
#[tauri::command]
fn set_provider(app: AppHandle, state: State<'_, AppState>, provider: String) -> Result<(), String> {
    let mut config = state.config.lock().unwrap().clone();
    config.provider = provider;
    config::save(&app, &config).map_err(|e| format!("Couldn't save the config: {e}"))?;
    state.translator.set_mode(config.mode());
    *state.config.lock().unwrap() = config;
    Ok(())
}

/// The text spoken in `lang`, as MP3 bytes.
#[tauri::command]
async fn speak(
    state: State<'_, AppState>,
    text: String,
    lang: String,
) -> Result<tauri::ipc::Response, translate::Error> {
    Ok(tauri::ipc::Response::new(state.translator.speech(&text, &lang).await?))
}

/// Saves a DeepL key after checking it works, returning this month's usage;
/// an empty key removes it.
#[tauri::command]
async fn set_deepl(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
) -> Result<Option<deepl::Usage>, String> {
    let mut config = state.config.lock().unwrap().clone();
    config.deepl_key = key.trim().to_owned();
    let usage = match config.deepl() {
        Some(deepl) => Some(state.translator.deepl_usage(&deepl).await.map_err(|e| e.message)?),
        None => None,
    };
    config::save(&app, &config).map_err(|e| format!("Couldn't save the config: {e}"))?;
    state.translator.set_deepl(config.deepl());
    *state.config.lock().unwrap() = config;
    Ok(usage)
}

/// `:ocr de`: the language painted text is read as; empty for Windows' choice.
/// When Windows has no recognizer of its own for it, the command that installs
/// one goes on the clipboard, and the reply says whether another stands in meanwhile.
#[tauri::command]
fn set_ocr_language(app: AppHandle, state: State<'_, AppState>, lang: String) -> Result<Option<String>, String> {
    #[allow(unused_mut)]
    let mut note = None;
    #[cfg(windows)]
    match ocr::engine(&lang) {
        Ok(engine) if !engine.exact => {
            set_clipboard(&state, ocr::install_command(&lang))?;
            note = Some(format!(
                "reading {lang} with a related recognizer · for its own, the install command is copied (admin PowerShell)"
            ));
        }
        Ok(_) => {}
        Err(ocr::EngineError::NotInstalled(lang)) => {
            set_clipboard(&state, ocr::install_command(&lang))?;
            return Err(format!(
                "Windows can't read {lang} text yet · install command copied, run it in an admin PowerShell"
            ));
        }
        Err(ocr::EngineError::Other(message)) => return Err(message),
    }
    let mut config = state.config.lock().unwrap().clone();
    config.ocr_language = lang;
    config::save(&app, &config).map_err(|e| format!("Couldn't save the config: {e}"))?;
    *state.config.lock().unwrap() = config;
    Ok(note)
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

#[derive(Serialize, Clone)]
struct Notice {
    text: String,
    kind: &'static str,
}

/// Paint hotkey handler: freeze the screen, let the user brush over text, read
/// it and open the popup with it, as if it had been selected.
#[cfg(windows)]
fn paint_from_hotkey(app: &AppHandle) {
    static PAINTING: AtomicBool = AtomicBool::new(false);
    if PAINTING.swap(true, Ordering::SeqCst) {
        return;
    }
    let popup_open = app.get_webview_window("main").is_some_and(|w| w.is_visible().unwrap_or(false));
    if popup_open {
        hide_popup(app);
    }
    let app = app.clone();
    // Its own thread: the overlay runs a message loop until the user is done.
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let previous = if popup_open {
            // The popup must be gone from the screen before it's captured.
            std::thread::sleep(Duration::from_millis(150));
            *state.previous_window.lock().unwrap()
        } else {
            origin::foreground()
        };
        let lang = state.config.lock().unwrap().ocr_language.clone();
        let outcome = match ocr::engine(&lang) {
            Err(ocr::EngineError::NotInstalled(lang)) => {
                Err(format!("Windows can't read {lang} text yet · :ocr {lang} copies the install command"))
            }
            Err(ocr::EngineError::Other(message)) => Err(message),
            Ok(engine) => match paint::run() {
                None => Ok(None),
                Some(painted) => match ocr::read(&engine, &painted) {
                    Ok(text) if text.trim().is_empty() => Err("no text found where you painted".into()),
                    Ok(text) => Ok(Some(text)),
                    Err(e) => Err(format!("Text recognition failed: {}", e.message())),
                },
            },
        };
        PAINTING.store(false, Ordering::SeqCst);
        match outcome {
            Ok(None) => {} // cancelled
            Ok(Some(text)) => show_for(&app, Some(text), previous),
            Err(text) => {
                show_for(&app, None, previous);
                let _ = app.emit("notice", Notice { text, kind: "error" });
            }
        }
    });
}

#[cfg(not(windows))]
fn paint_from_hotkey(app: &AppHandle) {
    show(app, None);
    let _ = app.emit("notice", Notice { text: "painting over text needs Windows for now".into(), kind: "error" });
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
    show_for(app, selection, origin::foreground());
}

/// `show`, pasting back into `previous` later.
fn show_for(app: &AppHandle, selection: Option<String>, previous: origin::Target) {
    let Some(window) = app.get_webview_window("main") else { return };
    let state = app.state::<AppState>();
    *state.previous_window.lock().unwrap() = previous;

    place_on_cursor_monitor(&window);
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
    let _ = window.emit("summon", selection);
}

/// Opens the popup on the monitor under the mouse: where `:move` left it, or
/// centered in the upper third. Always kept fully on screen.
fn place_on_cursor_monitor(window: &WebviewWindow) {
    let saved = window.state::<AppState>().config.lock().unwrap().window_position;
    let placed = (|| {
        let cursor = window.cursor_position().ok()?;
        let monitor = window.monitor_from_point(cursor.x, cursor.y).ok()??;
        let size = window.outer_size().ok()?;
        let (origin, area, scale) = (monitor.position(), monitor.size(), monitor.scale_factor());
        let (w, h) = (size.width as i32, size.height as i32);
        let (x, y) = match saved {
            Some([x, y]) => ((x * scale) as i32, (y * scale) as i32),
            None => ((area.width as i32 - w) / 2, (area.height as i32 - h) / 3),
        };
        let x = x.clamp(0, (area.width as i32 - w).max(0));
        let y = y.clamp(0, (area.height as i32 - h).max(0));
        window.set_position(PhysicalPosition::new(origin.x + x, origin.y + y)).ok()
    })();
    if placed.is_none() {
        let _ = window.center();
    }
}

const DEFAULT_SIZE: [f64; 2] = [760.0, 480.0];
const MIN_SIZE: [f64; 2] = [520.0, 320.0];

/// One `:move` step, in logical pixels.
#[tauri::command]
fn move_window(window: WebviewWindow, dx: f64, dy: f64) {
    let scale = window.scale_factor().unwrap_or(1.0);
    if let Ok(pos) = window.outer_position() {
        let (dx, dy) = ((dx * scale) as i32, (dy * scale) as i32);
        let _ = window.set_position(PhysicalPosition::new(pos.x + dx, pos.y + dy));
    }
}

/// One `:resize` step, in logical pixels.
#[tauri::command]
fn resize_window(window: WebviewWindow, dw: f64, dh: f64) {
    let scale = window.scale_factor().unwrap_or(1.0);
    if let Ok(size) = window.inner_size() {
        let size = size.to_logical::<f64>(scale);
        let width = (size.width + dw).max(MIN_SIZE[0]);
        let height = (size.height + dh).max(MIN_SIZE[1]);
        let _ = window.set_size(LogicalSize::new(width, height));
    }
}

/// Remembers where `:move` ("position") or `:resize` ("size") left the popup.
#[tauri::command]
fn save_placement(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    what: String,
) -> Result<(), String> {
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let mut config = state.config.lock().unwrap().clone();
    if what == "position" {
        let monitor = window.current_monitor().ok().flatten().ok_or("The popup isn't on a monitor")?;
        let pos = window.outer_position().map_err(|e| e.to_string())?;
        let origin = monitor.position();
        config.window_position = Some([(pos.x - origin.x) as f64 / scale, (pos.y - origin.y) as f64 / scale]);
    } else {
        let size = window.inner_size().map_err(|e| e.to_string())?.to_logical::<f64>(scale);
        config.window_size = Some([size.width, size.height]);
    }
    config::save(&app, &config).map_err(|e| format!("Couldn't save the config: {e}"))?;
    *state.config.lock().unwrap() = config;
    Ok(())
}

/// `:move reset` / `:resize reset`: back to centered, or to the default size.
#[tauri::command]
fn reset_placement(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    what: String,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap().clone();
    if what == "position" {
        config.window_position = None;
    } else {
        config.window_size = None;
        let _ = window.set_size(LogicalSize::new(DEFAULT_SIZE[0], DEFAULT_SIZE[1]));
    }
    config::save(&app, &config).map_err(|e| format!("Couldn't save the config: {e}"))?;
    *state.config.lock().unwrap() = config;
    place_on_cursor_monitor(&window);
    Ok(())
}

fn register_hotkey(app: &AppHandle) {
    let state = app.state::<AppState>();
    let (hotkey, paint_hotkey) = {
        let config = state.config.lock().unwrap();
        (config.hotkey.clone(), config.paint_hotkey.trim().to_owned())
    };
    let shortcuts = app.global_shortcut();
    let _ = shortcuts.unregister_all();
    let mut errors = Vec::new();
    if let Err(e) = shortcuts.register(hotkey.as_str()) {
        errors.push(format!("Couldn't register hotkey \"{hotkey}\": {e}"));
    }
    let mut paint = None;
    if !paint_hotkey.is_empty() {
        match paint_hotkey.parse::<Shortcut>() {
            Ok(shortcut) => match shortcuts.register(shortcut) {
                Ok(()) => paint = Some(shortcut),
                Err(e) => errors.push(format!("Couldn't register paint hotkey \"{paint_hotkey}\": {e}")),
            },
            Err(e) => errors.push(format!("Couldn't read paint hotkey \"{paint_hotkey}\": {e}")),
        }
    }
    let error = (!errors.is_empty()).then(|| errors.join(" · "));
    if let Some(e) = &error {
        eprintln!("{e}");
    }
    *state.paint_shortcut.lock().unwrap() = paint;
    *state.hotkey_error.lock().unwrap() = error;
}

fn reload_config(app: &AppHandle) {
    let state = app.state::<AppState>();
    let config = config::load(app);
    state.translator.set_deepl(config.deepl());
    state.translator.set_mode(config.mode());
    *state.config.lock().unwrap() = config;
    register_hotkey(app);
    let _ = app.emit("config-changed", ());
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let autostart_on = app.autolaunch().is_enabled().unwrap_or(false);
    let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let paint = MenuItem::with_id(app, "paint", "Paint text on screen", true, None::<&str>)?;
    let autostart =
        CheckMenuItem::with_id(app, "autostart", "Start at login", true, autostart_on, None::<&str>)?;
    let edit = MenuItem::with_id(app, "edit", "Edit config", true, None::<&str>)?;
    let reload = MenuItem::with_id(app, "reload", "Reload config", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &show,
            &paint,
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
            "paint" => paint_from_hotkey(app),
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
                .with_handler(|app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    let paint = *app.state::<AppState>().paint_shortcut.lock().unwrap();
                    if paint.as_ref() == Some(shortcut) {
                        paint_from_hotkey(app);
                    } else {
                        summon_from_hotkey(app);
                    }
                })
                .build(),
        )
        .setup(|app| {
            let handle = app.handle();
            let config = config::load(handle);
            let translator = translate::Translator::new();
            translator.set_deepl(config.deepl());
            translator.set_mode(config.mode());
            app.manage(AppState {
                translator,
                config: Mutex::new(config),
                hotkey_error: Mutex::new(None),
                paint_shortcut: Mutex::new(None),
                clipboard: Mutex::new(None),
                previous_window: Mutex::new(0),
            });
            register_hotkey(handle);
            build_tray(handle)?;

            let window = app.get_webview_window("main").expect("main window");
            if let Some([width, height]) = app.state::<AppState>().config.lock().unwrap().window_size {
                let _ = window.set_size(LogicalSize::new(width, height));
            }
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
        .invoke_handler(tauri::generate_handler![
            translate,
            ui_config,
            set_deepl,
            set_provider,
            set_ocr_language,
            limits,
            speak,
            move_window,
            resize_window,
            save_placement,
            reset_placement,
            copy,
            hide,
            paste
        ])
        .run(tauri::generate_context!())
        .expect("error while running tradu");
}
