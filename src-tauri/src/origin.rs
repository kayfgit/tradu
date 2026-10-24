//! The app the popup was opened from: grabbing its selected text, and pasting
//! a translation back into it.

use std::time::Instant;

/// Opaque handle of the previously focused window (0 = unknown).
pub type Target = isize;

/// A copy request sent to the foreground app; poll it to learn whether the app
/// had a selection to copy.
pub struct CopyRequest {
    #[allow(dead_code)]
    clipboard_before: u32,
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
        KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, VIRTUAL_KEY, VK_CONTROL,
        VK_INSERT, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN,
    };

    /// Unassigned virtual key: tapping it while Alt/Win is held stops their release
    /// from opening the app's menu bar or the Start menu.
    const VK_MASK: VIRTUAL_KEY = 0xE8;
    pub const VK_V: VIRTUAL_KEY = 0x56;

    pub fn is_down(vk: VIRTUAL_KEY) -> bool {
        unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 }
    }

    pub fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
        let extended = matches!(vk, VK_INSERT | VK_RMENU | VK_RCONTROL | VK_LWIN | VK_RWIN);
        let mut flags = if up { KEYEVENTF_KEYUP } else { 0 };
        if extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    pub fn send(inputs: &[INPUT]) {
        unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
    }

    /// Ctrl+`vk`, leaving Ctrl alone if the user is already holding it.
    pub fn ctrl_chord(inputs: &mut Vec<INPUT>, vk: VIRTUAL_KEY) {
        let ctrl_held = is_down(VK_CONTROL);
        if !ctrl_held {
            inputs.push(key(VK_CONTROL, false));
        }
        inputs.extend([key(vk, false), key(vk, true)]);
        if !ctrl_held {
            inputs.push(key(VK_CONTROL, true));
        }
    }

    /// Sends Ctrl+Insert, the copy shortcut that is safe in terminals (Ctrl+C there
    /// would interrupt the running program when nothing is selected). The hotkey's
    /// own modifiers are released first so the app sees exactly Ctrl+Insert.
    pub fn send_copy() {
        let mut inputs = Vec::new();
        let held: Vec<_> = [VK_LSHIFT, VK_RSHIFT, VK_LMENU, VK_RMENU, VK_LWIN, VK_RWIN]
            .into_iter()
            .filter(|&vk| is_down(vk))
            .collect();
        if held.iter().any(|&vk| vk != VK_LSHIFT && vk != VK_RSHIFT) {
            inputs.extend([key(VK_MASK, false), key(VK_MASK, true)]);
        }
        inputs.extend(held.iter().map(|&vk| key(vk, true)));
        ctrl_chord(&mut inputs, VK_INSERT);
        send(&inputs);
    }

    pub fn clipboard_sequence() -> u32 {
        unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() }
    }
}

#[cfg(windows)]
pub fn foreground() -> Target {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as Target }
}

/// Asks the foreground app to copy its selection.
#[cfg(windows)]
pub fn request_copy() -> CopyRequest {
    let clipboard_before = win::clipboard_sequence();
    win::send_copy();
    CopyRequest { clipboard_before }
}

impl CopyRequest {
    /// Waits until `deadline` for the app to write the clipboard. Apps with nothing
    /// selected leave it untouched.
    #[cfg(windows)]
    pub fn copied_by(&self, deadline: Instant) -> bool {
        loop {
            if win::clipboard_sequence() != self.clipboard_before {
                // The app may still be writing its other formats.
                std::thread::sleep(std::time::Duration::from_millis(10));
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(4));
        }
    }

    #[cfg(not(windows))]
    pub fn copied_by(&self, _deadline: Instant) -> bool {
        false
    }
}

/// Focuses `target` and sends Ctrl+V. The text must already be on the clipboard.
#[cfg(windows)]
pub fn paste_into(target: Target) {
    use windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow;

    if target != 0 {
        unsafe { SetForegroundWindow(target as _) };
    }
    // Give the target a moment to process activation before the keystrokes land.
    std::thread::sleep(std::time::Duration::from_millis(60));
    let mut inputs = Vec::new();
    win::ctrl_chord(&mut inputs, win::VK_V);
    win::send(&inputs);
}

#[cfg(not(windows))]
pub fn foreground() -> Target {
    0
}

/// Not implemented off Windows yet: never reports a selection.
#[cfg(not(windows))]
pub fn request_copy() -> CopyRequest {
    CopyRequest { clipboard_before: 0 }
}

/// Not implemented off Windows yet: the text is left on the clipboard.
#[cfg(not(windows))]
pub fn paste_into(_target: Target) {}
