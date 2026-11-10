//! tradu's hotkeys, seen by a low-level keyboard hook as well as RegisterHotKey.
//!
//! Games like League of Legends take raw keyboard input with RIDEV_NOHOTKEYS,
//! which stops Windows from delivering registered hotkeys (tradu's, and Print
//! Screen's) while they're in front. A low-level hook sees every key before
//! that, the way Discord's push-to-talk does. When it catches a hotkey it
//! swallows the key, so the registered hotkey doesn't fire a second time;
//! if Windows ever drops the hook, RegisterHotKey still works everywhere else.

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
/// The hotkey being held down: its repeats and its release are swallowed too.
static HELD: Mutex<Option<Hold>> = Mutex::new(None);

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
        if !injected && caught(event.vkCode, message, event.time) {
            return LRESULT(1);
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

/// Whether this key event belongs to a hotkey, firing it on the first press.
fn caught(vk: u32, message: u32, time: u32) -> bool {
    let Ok(watched) = WATCHED.try_lock() else { return false };
    let mut held = HELD.lock().unwrap();
    match judge(&mut held, &watched, KeyEvent { vk, message, mods: modifiers(), time }) {
        Verdict::Pass => false,
        Verdict::Swallow => true,
        Verdict::Fire(shortcut) => {
            tap_mask_key();
            // Off the hook's thread: Windows drops hooks that take too long.
            std::thread::spawn(move || HANDLER.get().map(|on_press| on_press(shortcut)));
            true
        }
    }
}

/// A key event as the hook sees it.
#[derive(Clone, Copy)]
struct KeyEvent {
    vk: u32,
    message: u32,
    mods: Modifiers,
    /// Milliseconds, from the event's own clock.
    time: u32,
}

/// The key of a hotkey being held down.
#[derive(Clone, Copy)]
struct Hold {
    vk: u32,
    /// The hotkey's modifiers: repeats come with them still held.
    mods: Modifiers,
    /// When the key last went down (pressed or repeated).
    last: u32,
}

/// The longest wait for a held key's next repeat: Windows' longest repeat
/// delay is a second. Anything later is a new press.
const REPEAT_GAP_MS: u32 = 1100;

#[derive(Debug, PartialEq)]
enum Verdict {
    /// Not tradu's: let the app have it.
    Pass,
    /// Part of a hotkey already fired (a repeat or the release): drop it.
    Swallow,
    /// A hotkey was pressed: drop the key and run the hotkey.
    Fire(Shortcut),
}

/// What to do with `event`, given the hotkey being held (updated) and the hotkeys watched.
///
/// The hold ends when its key comes up, but the hook doesn't always see that:
/// closing the popup hands the focus to another window while the key is still
/// down, and Windows keeps keys from the hook while an elevated window is in
/// front, for one. So a press of the held key only counts as a repeat while
/// the hotkey's modifiers are still down and it comes soon after the last one;
/// otherwise the hold is over and the press is judged afresh.
fn judge(held: &mut Option<Hold>, watched: &[(u16, Shortcut)], event: KeyEvent) -> Verdict {
    let KeyEvent { vk, message, mods, time } = event;
    let ours = held.filter(|h| h.vk == vk);
    match message {
        WM_KEYUP | WM_SYSKEYUP if ours.is_some() => {
            *held = None;
            Verdict::Swallow
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if let Some(hold) = ours {
                if hold.mods == mods && time.wrapping_sub(hold.last) <= REPEAT_GAP_MS {
                    *held = Some(Hold { last: time, ..hold });
                    return Verdict::Swallow;
                }
                *held = None;
            }
            let Some(&(_, shortcut)) = watched.iter().find(|(key, s)| *key as u32 == vk && s.mods == mods) else {
                return Verdict::Pass;
            };
            *held = Some(Hold { vk, mods, last: time });
            Verdict::Fire(shortcut)
        }
        _ => Verdict::Pass,
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

    const SPACE: u32 = 0x20;
    const CTRL_SHIFT: Modifiers = Modifiers::CONTROL.union(Modifiers::SHIFT);

    /// tradu's default hotkey, Ctrl+Shift+Space, followed by `events`: (message, mods, time).
    fn after_hotkey(events: &[(u32, Modifiers, u32)]) -> Vec<Verdict> {
        let hotkey: Shortcut = "Ctrl+Shift+Space".parse().unwrap();
        let watched = [(SPACE as u16, hotkey)];
        let mut held = None;
        let press = KeyEvent { vk: SPACE, message: WM_KEYDOWN, mods: CTRL_SHIFT, time: 0 };
        assert_eq!(judge(&mut held, &watched, press), Verdict::Fire(hotkey));
        events
            .iter()
            .map(|&(message, mods, time)| judge(&mut held, &watched, KeyEvent { vk: SPACE, message, mods, time }))
            .collect()
    }

    #[test]
    fn swallows_the_hotkeys_repeats_and_release() {
        let verdicts =
            after_hotkey(&[(WM_KEYDOWN, CTRL_SHIFT, 500), (WM_KEYDOWN, CTRL_SHIFT, 533), (WM_KEYUP, CTRL_SHIFT, 560)]);
        assert_eq!(verdicts, [Verdict::Swallow, Verdict::Swallow, Verdict::Swallow]);
    }

    #[test]
    fn lets_space_through_once_the_hotkey_is_released() {
        let verdicts = after_hotkey(&[(WM_KEYUP, CTRL_SHIFT, 100), (WM_KEYDOWN, Modifiers::empty(), 2000)]);
        assert_eq!(verdicts, [Verdict::Swallow, Verdict::Pass]);
    }

    // The popup closing hands the focus to another window while the hotkey is
    // still down, and the hook doesn't always see its key come back up (Windows
    // keeps keys from it while an elevated window is in front, for one).
    #[test]
    fn lets_space_through_when_the_hotkeys_release_went_unseen() {
        let verdicts = after_hotkey(&[(WM_KEYDOWN, Modifiers::empty(), 2000), (WM_KEYUP, Modifiers::empty(), 2080)]);
        assert_eq!(verdicts, [Verdict::Pass, Verdict::Pass]);
    }

    #[test]
    fn fires_again_when_the_hotkeys_release_went_unseen() {
        let hotkey: Shortcut = "Ctrl+Shift+Space".parse().unwrap();
        let verdicts = after_hotkey(&[(WM_KEYDOWN, CTRL_SHIFT, 3000)]);
        assert_eq!(verdicts, [Verdict::Fire(hotkey)]);
    }
}
