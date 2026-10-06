//! Windows backend: low-level keyboard and mouse hooks on their own thread feed `chord::State`,
//! so modifier-only bindings such as Ctrl+Win work like the macOS tap. Raw FFI, no tauri types.

use super::chord::*;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex, OnceLock};

type HookProc = unsafe extern "system" fn(code: i32, wparam: usize, lparam: isize) -> isize;

const WH_KEYBOARD_LL: i32 = 13;
const WH_MOUSE_LL: i32 = 14;
const HC_ACTION: i32 = 0;
const WM_KEYDOWN: usize = 0x100;
const WM_SYSKEYDOWN: usize = 0x104;
/// Left/right/middle/X button down. Clicks abort modifier-only holds (Ctrl+Alt-drag must not
/// start a command); scrolling doesn't, like on macOS.
const POINTER_DOWN: [usize; 4] = [0x201, 0x204, 0x207, 0x20B];
const LLKHF_EXTENDED: u32 = 1;
const VK_CAPITAL: u32 = 0x14;
const INPUT_KEYBOARD: u32 = 1;
const KEYEVENTF_KEYUP: u32 = 2;
const MAPVK_VK_TO_VSC_EX: u32 = 4;
/// An unassigned virtual key (AutoHotkey's menu mask key). Pressed after a Win or Alt hotkey
/// goes down, so its release isn't a lone Win/Alt tap that opens Start or the menu bar.
const MASK_KEY: u16 = 0xE8;
/// dwExtraInfo of the mask key; enigo marks its Ctrl+V paste with `enigo::EVENT_MARKER`.
const OWN_MARKER: usize = 0x4F47_4C50;
const MASKED_MODIFIERS: u16 = META_LEFT | META_RIGHT | ALT_LEFT | ALT_RIGHT;

/// Side-specific modifier virtual keys (CapsLock deliberately ignored, as on macOS).
const MODIFIER_VKS: [(u32, u16); 8] = [
    (0xA2, CONTROL_LEFT),
    (0xA3, CONTROL_RIGHT),
    (0xA4, ALT_LEFT),
    (0xA5, ALT_RIGHT),
    (0xA0, SHIFT_LEFT),
    (0xA1, SHIFT_RIGHT),
    (0x5B, META_LEFT),
    (0x5C, META_RIGHT),
];

/// Scan code (0xE0 prefix for extended keys) → `KeyboardEvent.code`, for the keys a binding may
/// use. Physical positions, like the codes the webview reports.
const SCAN_CODES: &[(u32, &str)] = &[
    (0x01, "Escape"), (0x02, "Digit1"), (0x03, "Digit2"), (0x04, "Digit3"), (0x05, "Digit4"),
    (0x06, "Digit5"), (0x07, "Digit6"), (0x08, "Digit7"), (0x09, "Digit8"), (0x0A, "Digit9"),
    (0x0B, "Digit0"), (0x0C, "Minus"), (0x0D, "Equal"), (0x0E, "Backspace"), (0x0F, "Tab"),
    (0x10, "KeyQ"), (0x11, "KeyW"), (0x12, "KeyE"), (0x13, "KeyR"), (0x14, "KeyT"), (0x15, "KeyY"),
    (0x16, "KeyU"), (0x17, "KeyI"), (0x18, "KeyO"), (0x19, "KeyP"), (0x1A, "BracketLeft"),
    (0x1B, "BracketRight"), (0x1C, "Enter"), (0x1E, "KeyA"), (0x1F, "KeyS"), (0x20, "KeyD"),
    (0x21, "KeyF"), (0x22, "KeyG"), (0x23, "KeyH"), (0x24, "KeyJ"), (0x25, "KeyK"), (0x26, "KeyL"),
    (0x27, "Semicolon"), (0x28, "Quote"), (0x29, "Backquote"), (0x2B, "Backslash"), (0x2C, "KeyZ"),
    (0x2D, "KeyX"), (0x2E, "KeyC"), (0x2F, "KeyV"), (0x30, "KeyB"), (0x31, "KeyN"), (0x32, "KeyM"),
    (0x33, "Comma"), (0x34, "Period"), (0x35, "Slash"), (0x37, "NumpadMultiply"), (0x39, "Space"),
    (0x3B, "F1"), (0x3C, "F2"), (0x3D, "F3"), (0x3E, "F4"), (0x3F, "F5"), (0x40, "F6"), (0x41, "F7"),
    (0x42, "F8"), (0x43, "F9"), (0x44, "F10"), (0x47, "Numpad7"), (0x48, "Numpad8"), (0x49, "Numpad9"),
    (0x4A, "NumpadSubtract"), (0x4B, "Numpad4"), (0x4C, "Numpad5"), (0x4D, "Numpad6"), (0x4E, "NumpadAdd"),
    (0x4F, "Numpad1"), (0x50, "Numpad2"), (0x51, "Numpad3"), (0x52, "Numpad0"), (0x53, "NumpadDecimal"),
    (0x56, "IntlBackslash"), (0x57, "F11"), (0x58, "F12"), (0x64, "F13"), (0x65, "F14"), (0x66, "F15"),
    (0x67, "F16"), (0x68, "F17"), (0x69, "F18"), (0x6A, "F19"), (0x6B, "F20"),
    (0xE01C, "NumpadEnter"), (0xE035, "NumpadDivide"), (0xE047, "Home"), (0xE048, "ArrowUp"),
    (0xE049, "PageUp"), (0xE04B, "ArrowLeft"), (0xE04D, "ArrowRight"), (0xE04F, "End"),
    (0xE050, "ArrowDown"), (0xE051, "PageDown"), (0xE052, "Insert"), (0xE053, "Delete"),
];

/// Keys whose scan codes are ambiguous (NumLock and Pause share 0x45), by virtual key.
const VK_CODES: [(u32, &str); 4] = [(0x13, "Pause"), (0x90, "NumLock"), (0x2C, "PrintScreen"), (0x91, "ScrollLock")];

#[repr(C)]
struct KbdLlHookStruct {
    vk: u32,
    scan: u32,
    flags: u32,
    time: u32,
    extra: usize,
}

#[repr(C)]
struct KeybdInput {
    vk: u16,
    scan: u16,
    flags: u32,
    time: u32,
    extra: usize,
}

/// INPUT with the keyboard member; the padding makes it as large as the mouse member.
#[repr(C)]
struct InputRecord {
    kind: u32,
    ki: KeybdInput,
    padding: [u8; 8],
}

#[repr(C)]
struct Msg {
    hwnd: *mut c_void,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: [i32; 2],
    private: u32,
}

#[link(name = "user32")]
extern "system" {
    fn SetWindowsHookExW(id: i32, proc_: HookProc, module: *mut c_void, thread: u32) -> *mut c_void;
    fn CallNextHookEx(hook: *mut c_void, code: i32, wparam: usize, lparam: isize) -> isize;
    fn GetMessageW(msg: *mut Msg, hwnd: *mut c_void, min: u32, max: u32) -> i32;
    fn GetAsyncKeyState(vk: i32) -> i16;
    fn MapVirtualKeyW(code: u32, map_type: u32) -> u32;
    fn SendInput(count: u32, inputs: *const InputRecord, size: i32) -> u32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
}

struct Ctx {
    state: Arc<Mutex<State>>,
    send: Box<dyn Fn(Output) + Send + Sync>,
    keys: Mutex<Keys>,
}

#[derive(Default)]
struct Keys {
    /// Non-modifier keys seen going down (code, virtual key): autorepeat and stale-key cleanup.
    held: Vec<(&'static str, u32)>,
    /// The mask key went out since every modifier was last up.
    masked: bool,
}

/// Hook procedures get no user data; there is one engine per process.
static CTX: OnceLock<Ctx> = OnceLock::new();

/// Installs the hooks on their own thread and pumps its messages forever. `send` gets the state
/// machine output (called on the hook thread, must not block); `on_status(error)` reports whether
/// the keyboard hook is running.
pub fn spawn(
    state: Arc<Mutex<State>>,
    send: impl Fn(Output) + Send + Sync + 'static,
    on_status: impl Fn(Option<String>) + Send + 'static,
) {
    let run = move || {
        if CTX.set(Ctx { state, send: Box::new(send), keys: Mutex::default() }).is_err() {
            return;
        }
        unsafe {
            let module = GetModuleHandleW(null());
            if SetWindowsHookExW(WH_KEYBOARD_LL, keyboard_proc, module, 0).is_null() {
                on_status(Some("Couldn't start the keyboard listener (SetWindowsHookEx failed). Restart OpenGlaido.".into()));
                return;
            }
            // Without it clicks just don't abort modifier-only holds.
            if SetWindowsHookExW(WH_MOUSE_LL, mouse_proc, module, 0).is_null() {
                eprintln!("[hotkeys] couldn't start the mouse listener");
            }
            on_status(None);
            let mut msg: Msg = std::mem::zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {}
        }
    };
    std::thread::Builder::new().name("hotkey-hook".into()).spawn(run).expect("spawn hotkey hook thread");
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: usize, lparam: isize) -> isize {
    if code == HC_ACTION {
        if let Some(ctx) = CTX.get() {
            let info = unsafe { &*(lparam as *const KbdLlHookStruct) };
            if ctx.key(wparam == WM_KEYDOWN || wparam == WM_SYSKEYDOWN, info) {
                return 1;
            }
        }
    }
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: usize, lparam: isize) -> isize {
    if code == HC_ACTION && POINTER_DOWN.contains(&wparam) {
        if let Some(ctx) = CTX.get() {
            let outputs = ctx.state.lock().unwrap_or_else(|e| e.into_inner()).handle(Input::Pointer).0;
            for output in outputs {
                (ctx.send)(output);
            }
        }
    }
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

impl Ctx {
    /// One keyboard event; true swallows it. Never panics: a panic here would abort the app.
    fn key(&self, down: bool, info: &KbdLlHookStruct) -> bool {
        // Our own synthetic keys (the Ctrl+V paste, the mask key) must never trigger hotkeys.
        if info.extra == OWN_MARKER || info.extra == enigo::EVENT_MARKER as usize || info.vk == VK_CAPITAL {
            return false;
        }
        let extended = info.flags & LLKHF_EXTENDED != 0;
        let bit = modifier_bit(info.vk, info.scan, extended);
        let mut outputs = Vec::new();
        let mut keys = self.keys.lock().unwrap_or_else(|e| e.into_inner());
        let (consume, mask) = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if down {
                // A key-up we never saw (lock screen, secure desktop, an elevated window) must not
                // leave a modifier or key stuck. The current key's own state isn't updated yet.
                for (vk, b) in MODIFIER_VKS {
                    if Some(b) != bit && st.mods() & b != 0 && !is_down(vk) {
                        outputs.extend(st.handle(Input::Modifier(b, false)).0);
                    }
                }
                keys.held.retain(|&(_, vk)| vk == info.vk || is_down(vk));
                st.retain_keys(|code| keys.held.iter().any(|&(held, _)| held == code));
            }
            let consume = match bit {
                Some(b) => {
                    outputs.extend(st.handle(Input::Modifier(b, down)).0);
                    false
                }
                None => {
                    let code = code_of(info.vk, info.scan, extended);
                    let input = if down {
                        let repeat = keys.held.contains(&(code, info.vk));
                        if !repeat {
                            keys.held.push((code, info.vk));
                        }
                        Input::KeyDown(code, repeat)
                    } else {
                        keys.held.retain(|&held| held != (code, info.vk));
                        Input::KeyUp(code)
                    };
                    let (out, consume) = st.handle(input);
                    outputs.extend(out);
                    consume
                }
            };
            let mods = st.mods();
            if mods == 0 {
                keys.masked = false;
            }
            let hotkey = st.capturing() || (consume && down) || modifier_only_hit(&st, mods);
            let mask = !keys.masked && mods & MASKED_MODIFIERS != 0 && hotkey;
            keys.masked |= mask;
            (consume, mask)
        };
        drop(keys);
        for output in outputs {
            (self.send)(output);
        }
        if mask {
            send_mask_key();
        }
        consume
    }
}

/// A modifier-only hold or hands-free binding of either pair is exactly what's held.
fn modifier_only_hit(st: &State, mods: u16) -> bool {
    st.pairs.iter().flat_map(|p| [&p.hold, &p.toggle]).flatten().any(|b| b.key.is_none() && b.matches(mods))
}

fn is_down(vk: u32) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) < 0 }
}

/// Modifier bit for a virtual key. Synthetic input may use the side-less VK_SHIFT/CONTROL/MENU.
fn modifier_bit(vk: u32, scan: u32, extended: bool) -> Option<u16> {
    match vk {
        0x10 => Some(if scan == 0x36 { SHIFT_RIGHT } else { SHIFT_LEFT }),
        0x11 => Some(if extended { CONTROL_RIGHT } else { CONTROL_LEFT }),
        0x12 => Some(if extended { ALT_RIGHT } else { ALT_LEFT }),
        _ => MODIFIER_VKS.iter().find(|(v, _)| *v == vk).map(|(_, bit)| *bit),
    }
}

/// `KeyboardEvent.code` of a non-modifier key; synthetic input without a scan code gets the
/// layout's scan code for its virtual key.
fn code_of(vk: u32, scan: u32, extended: bool) -> &'static str {
    if let Some((_, code)) = VK_CODES.iter().find(|(v, _)| *v == vk) {
        return code;
    }
    let scan = match (scan, extended) {
        (0, _) => unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_VSC_EX) },
        (scan, true) => 0xE000 | scan,
        (scan, false) => scan,
    };
    scan_code_name(scan)
}

fn scan_code_name(scan: u32) -> &'static str {
    SCAN_CODES.iter().find(|(s, _)| *s == scan).map_or("Unidentified", |(_, code)| *code)
}

fn send_mask_key() {
    let key = |flags| InputRecord {
        kind: INPUT_KEYBOARD,
        ki: KeybdInput { vk: MASK_KEY, scan: 0, flags, time: 0, extra: OWN_MARKER },
        padding: [0; 8],
    };
    let inputs = [key(0), key(KEYEVENTF_KEYUP)];
    unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<InputRecord>() as i32) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_codes_name_binding_keys() {
        for (scan, code) in SCAN_CODES.iter().chain(&VK_CODES) {
            assert!(parse(&format!("Control+{code}")).is_ok(), "{code}");
            assert!(SCAN_CODES.iter().filter(|(s, _)| s == scan).count() <= 1, "{scan:#x}");
        }
        assert_eq!(code_of(0x47, 0x22, false), "KeyG");
        assert_eq!(code_of(0x25, 0x4B, true), "ArrowLeft");
        assert_eq!(code_of(0x64, 0x4B, false), "Numpad4");
        assert_eq!(code_of(0x90, 0x45, true), "NumLock");
        assert_eq!(code_of(0xFF, 0x7F, false), "Unidentified");
    }

    #[test]
    fn modifiers_keep_their_side() {
        assert_eq!(modifier_bit(0xA3, 0x1D, true), Some(CONTROL_RIGHT));
        assert_eq!(modifier_bit(0x11, 0x1D, false), Some(CONTROL_LEFT));
        assert_eq!(modifier_bit(0x12, 0x38, true), Some(ALT_RIGHT));
        assert_eq!(modifier_bit(0x10, 0x36, false), Some(SHIFT_RIGHT));
        assert_eq!(modifier_bit(0x5B, 0x5B, true), Some(META_LEFT));
        assert_eq!(modifier_bit(VK_CAPITAL, 0x3A, false), None);
    }

    #[test]
    fn input_record_matches_the_win32_layout() {
        // sizeof(INPUT): 40 on 64-bit, 28 on 32-bit.
        assert_eq!(std::mem::size_of::<InputRecord>(), if cfg!(target_pointer_width = "64") { 40 } else { 28 });
    }
}
