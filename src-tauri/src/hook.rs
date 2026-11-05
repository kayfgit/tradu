//! tradu's hotkeys, seen by a low-level keyboard hook as well as RegisterHotKey.
//!
//! Games like League of Legends take raw keyboard input with RIDEV_NOHOTKEYS,
//! which stops Windows from delivering registered hotkeys (tradu's, and Print
//! Screen's) while they're in front. A low-level hook sees every key before
//! that, the way Discord's push-to-talk does. When it catches a hotkey it
//! swallows the key, so the registered hotkey doesn't fire a second time;
//! if Windows ever drops the hook, RegisterHotKey still works everywhere else.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CONTROL,
    VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, SetWindowsHookExW, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, WH_KEYBOARD_LL, WM_KEYDOWN,
    WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

type Handler = Box<dyn Fn(Shortcut) + Send + Sync>;

static HANDLER: OnceLock<Handler> = OnceLock::new();
/// The hotkeys to catch, with their virtual-key codes.
static WATCHED: Mutex<Vec<(u16, Shortcut)>> = Mutex::new(Vec::new());
/// The key of a hotkey being held down: its repeats and its release are swallowed too.
static HELD: AtomicU32 = AtomicU32::new(0);

/// Starts the hook on its own thread; `on_press` runs on a fresh thread per press.
pub fn install(on_press: impl Fn(Shortcut) + Send + Sync + 'static) {
    if HANDLER.set(Box::new(on_press)).is_err() {
        return;
    }
    std::thread::spawn(|| unsafe {
        let instance = GetModuleHandleW(None).ok().map(Into::into);
        if let Err(e) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard), instance, 0) {
            eprintln!("Couldn't hook the keyboard: {e}");
            return;
        }
        // The hook is called on this thread, through its message loop.
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
    });
}

/// The hotkeys the hook catches from now on.
pub fn watch(shortcuts: &[Shortcut]) {
    *WATCHED.lock().unwrap() = shortcuts.iter().filter_map(|s| Some((virtual_key(s.key)?, *s))).collect();
}

unsafe extern "system" fn keyboard(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let event = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let message = wparam.0 as u32;
        // Keys tradu sends itself (Ctrl+Insert to copy a selection) pass through.
        let injected = event.flags.0 & LLKHF_INJECTED.0 != 0;
        if !injected && caught(event.vkCode, message) {
            return LRESULT(1);
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

/// Whether this key event belongs to a hotkey, firing it on the first press.
fn caught(vk: u32, message: u32) -> bool {
    let held = HELD.load(Ordering::Relaxed);
    match message {
        WM_KEYUP | WM_SYSKEYUP if vk == held => {
            HELD.store(0, Ordering::Relaxed);
            true
        }
        WM_KEYDOWN | WM_SYSKEYDOWN if vk == held => true,
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let Ok(watched) = WATCHED.try_lock() else { return false };
            let Some(&(_, shortcut)) = watched.iter().find(|(key, s)| *key as u32 == vk && s.mods == modifiers())
            else {
                return false;
            };
            HELD.store(vk, Ordering::Relaxed);
            tap_mask_key();
            // Off the hook's thread: Windows drops hooks that take too long.
            std::thread::spawn(move || HANDLER.get().map(|on_press| on_press(shortcut)));
            true
        }
        _ => false,
    }
}

/// An unassigned key, tapped when a hotkey is caught, as AutoHotkey does: it
/// keeps the Alt or Win held for the hotkey from opening a menu when let go,
/// and makes tradu the last to send input, which lets it take the foreground.
const MASK_KEY: u16 = 0xE8;

fn tap_mask_key() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(MASK_KEY), dwFlags: flags, ..Default::default() } },
    };
    unsafe { SendInput(&[key(Default::default()), key(KEYEVENTF_KEYUP)], size_of::<INPUT>() as i32) };
}

/// The modifiers held right now, as a shortcut spells them.
fn modifiers() -> Modifiers {
    let down = |vk: VIRTUAL_KEY| unsafe { GetAsyncKeyState(vk.0 as i32) } as u16 & 0x8000 != 0;
    let mut mods = Modifiers::empty();
    mods.set(Modifiers::CONTROL, down(VK_CONTROL));
    mods.set(Modifiers::ALT, down(VK_MENU));
    mods.set(Modifiers::SHIFT, down(VK_SHIFT));
    mods.set(Modifiers::SUPER, down(VK_LWIN) || down(VK_RWIN));
    mods
}

/// The virtual-key code for a hotkey's key, for the keys hotkeys tend to use.
fn virtual_key(key: Code) -> Option<u16> {
    let name = key.to_string();
    let char_key = |rest: &str| (rest.len() == 1).then(|| rest.as_bytes()[0] as u16);
    if let Some(letter) = name.strip_prefix("Key") {
        return char_key(letter);
    }
    if let Some(digit) = name.strip_prefix("Digit") {
        return char_key(digit);
    }
    if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<u16>().ok()) {
        return (1..=24).contains(&n).then_some(0x70 + n - 1);
    }
    Some(match key {
        Code::Space => 0x20,
        Code::Enter => 0x0D,
        Code::Tab => 0x09,
        Code::Insert => 0x2D,
        Code::Delete => 0x2E,
        Code::Home => 0x24,
        Code::End => 0x23,
        Code::PageUp => 0x21,
        Code::PageDown => 0x22,
        Code::Pause => 0x13,
        Code::Backquote => 0xC0,
        Code::Minus => 0xBD,
        Code::Equal => 0xBB,
        Code::BracketLeft => 0xDB,
        Code::BracketRight => 0xDD,
        Code::Backslash => 0xDC,
        Code::Semicolon => 0xBA,
        Code::Quote => 0xDE,
        Code::Comma => 0xBC,
        Code::Period => 0xBE,
        Code::Slash => 0xBF,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_hotkey_keys() {
        let vk = |s: &str| virtual_key(s.parse::<Shortcut>().unwrap().key);
        assert_eq!(vk("Alt+Shift+Space"), Some(0x20));
        assert_eq!(vk("Ctrl+T"), Some(b'T' as u16));
        assert_eq!(vk("Ctrl+7"), Some(b'7' as u16));
        assert_eq!(vk("F9"), Some(0x78));
        assert_eq!(vk("Ctrl+Backquote"), Some(0xC0));
    }
}
