//! macOS backend: an active CGEventTap (needs Accessibility) on its own CFRunLoop thread feeds
//! `chord::State`, plus the Carbon exclusive-hotkey probe behind `conflict_warning`.
//! Raw FFI, no tauri types, so it also builds in a standalone test crate.

use super::chord::*;
use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Ref = *mut c_void;
type TapCallback = extern "C" fn(Ref, u32, Ref, *mut c_void) -> Ref;

pub const NEEDS_ACCESS: &str = "OpenGlaido needs Accessibility permission for its hotkeys. \
Turn it on in System Settings › Privacy & Security › Accessibility.";

const KEY_DOWN: u32 = 10;
const KEY_UP: u32 = 11;
const FLAGS_CHANGED: u32 = 12;
/// Clicks and scrolls abort modifier-only holds (⌥-drag must not start a command).
// Left/right/other mouse down. Not scrolling: trackpad momentum keeps scrolling after the fingers lift.
const POINTER_EVENTS: [u32; 3] = [1, 3, 25];
/// Left button: a click on the dictation bar's cancel "x" (see `State::click`).
const LEFT_MOUSE_DOWN: u32 = 1;
const LEFT_MOUSE_UP: u32 = 2;
const TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;
const FIELD_AUTOREPEAT: u32 = 8;
const FIELD_KEYCODE: u32 = 9;
const FIELD_SOURCE_PID: u32 = 41;
const FLAG_FN: u64 = 0x80_0000;

/// Per group: (generic flag, [(device-dependent NX_DEVICE* flag, modifier bit); 2]).
const SIDES: [(u64, [(u64, u16); 2]); 4] = [
    (0x4_0000, [(0x1, CONTROL_LEFT), (0x2000, CONTROL_RIGHT)]),
    (0x8_0000, [(0x20, ALT_LEFT), (0x40, ALT_RIGHT)]),
    (0x2_0000, [(0x2, SHIFT_LEFT), (0x4, SHIFT_RIGHT)]),
    (0x10_0000, [(0x8, META_LEFT), (0x10, META_RIGHT)]),
];

/// flagsChanged keycode → modifier bit (CapsLock 57 deliberately ignored).
const MODIFIER_KEYS: [(i64, u16); 9] = [
    (55, META_LEFT),
    (54, META_RIGHT),
    (56, SHIFT_LEFT),
    (60, SHIFT_RIGHT),
    (58, ALT_LEFT),
    (61, ALT_RIGHT),
    (59, CONTROL_LEFT),
    (62, CONTROL_RIGHT),
    (63, FN),
];

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    static kAXTrustedCheckOptionPrompt: Ref;
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: Ref) -> u8;
    fn CGEventTapCreate(tap: u32, place: u32, options: u32, mask: u64, callback: TapCallback, user_info: *mut c_void) -> Ref;
    fn CGEventTapEnable(tap: Ref, enable: bool);
    fn CGEventGetIntegerValueField(event: Ref, field: u32) -> i64;
    fn CGEventGetFlags(event: Ref) -> u64;
    fn CGEventGetLocation(event: Ref) -> CGPoint;
    fn CGEventSourceKeyState(state: i32, key: u16) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFBooleanTrue: Ref;
    static kCFRunLoopCommonModes: Ref;
    // Only their addresses are used.
    static kCFTypeDictionaryKeyCallBacks: u8;
    static kCFTypeDictionaryValueCallBacks: u8;
    fn CFDictionaryCreate(alloc: Ref, keys: *const Ref, values: *const Ref, count: isize, key_callbacks: *const u8, value_callbacks: *const u8) -> Ref;
    fn CFRelease(cf: Ref);
    fn CFMachPortCreateRunLoopSource(alloc: Ref, port: Ref, order: isize) -> Ref;
    fn CFRunLoopGetCurrent() -> Ref;
    fn CFRunLoopAddSource(run_loop: Ref, source: Ref, mode: Ref);
    fn CFRunLoopRun();
}

#[repr(C)]
struct HotKeyId {
    signature: u32,
    id: u32,
}

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn GetApplicationEventTarget() -> Ref;
    fn RegisterEventHotKey(keycode: u32, modifiers: u32, id: HotKeyId, target: Ref, options: u32, out: *mut Ref) -> i32;
    fn UnregisterEventHotKey(hotkey: Ref) -> i32;
}

/// Accessibility trust; with `prompt` macOS shows its "allow in Settings" dialog if missing.
pub fn trusted(prompt: bool) -> bool {
    unsafe {
        if !prompt {
            return AXIsProcessTrusted() != 0;
        }
        let (keys, values) = ([kAXTrustedCheckOptionPrompt], [kCFBooleanTrue]);
        let options = CFDictionaryCreate(null_mut(), keys.as_ptr(), values.as_ptr(), 1, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
        let ok = AXIsProcessTrustedWithOptions(options) != 0;
        CFRelease(options);
        ok
    }
}

/// Global display coordinates in points, origin at the top left of the main display.
#[repr(C)]
struct CGPoint {
    x: f64,
    y: f64,
}

struct Ctx {
    state: Arc<Mutex<State>>,
    send: Box<dyn Fn(Output) + Send>,
    tap: Ref,
}

/// Runs the tap on its own thread: prompts for Accessibility once, polls every 2 s until it is
/// granted, then taps keyDown/keyUp/flagsChanged forever. `send` gets the state machine output
/// (called on the tap thread, must not block); `on_status(granted, error)` reports problems.
pub fn spawn(
    state: Arc<Mutex<State>>,
    send: impl Fn(Output) + Send + 'static,
    on_status: impl Fn(bool, Option<String>) + Send + 'static,
) {
    let run = move || {
        if !trusted(true) {
            on_status(false, Some(NEEDS_ACCESS.into()));
            while !trusted(false) {
                std::thread::sleep(Duration::from_secs(2));
            }
        }
        let ctx = Box::into_raw(Box::new(Ctx { state, send: Box::new(send), tap: null_mut() }));
        let keys = (1u64 << KEY_DOWN) | (1 << KEY_UP) | (1 << FLAGS_CHANGED) | (1 << LEFT_MOUSE_UP);
        let mask = POINTER_EVENTS.iter().fold(keys, |m, t| m | (1 << t));
        unsafe {
            // Session tap, head insert, active (can swallow events).
            let tap = CGEventTapCreate(1, 0, 0, mask, callback, ctx.cast());
            if tap.is_null() {
                on_status(true, Some("Couldn't start the keyboard listener (CGEventTapCreate failed). Try removing and re-adding OpenGlaido under Accessibility.".into()));
                return;
            }
            (*ctx).tap = tap;
            let source = CFMachPortCreateRunLoopSource(null_mut(), tap, 0);
            CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
            CGEventTapEnable(tap, true);
            on_status(true, None);
            CFRunLoopRun();
        }
    };
    std::thread::Builder::new().name("hotkey-tap".into()).spawn(run).expect("spawn hotkey tap thread");
}

extern "C" fn callback(_proxy: Ref, ty: u32, event: Ref, ctx: *mut c_void) -> Ref {
    let ctx = unsafe { &*(ctx as *const Ctx) };
    if ty == TAP_DISABLED_BY_TIMEOUT || ty == TAP_DISABLED_BY_USER_INPUT {
        unsafe { CGEventTapEnable(ctx.tap, true) };
        // A modifier released while the tap was off would leave its hold recording.
        let mut outputs = Vec::new();
        {
            let mut st = ctx.state.lock().unwrap_or_else(|e| e.into_inner());
            for (key, bit) in MODIFIER_KEYS {
                if st.mods() & bit != 0 && !unsafe { CGEventSourceKeyState(0, key as u16) } {
                    outputs.extend(st.handle(Input::Modifier(bit, false)).0);
                }
            }
        }
        for output in outputs {
            (ctx.send)(output);
        }
        return event;
    }
    if ty == LEFT_MOUSE_DOWN || ty == LEFT_MOUSE_UP {
        let at = unsafe { CGEventGetLocation(event) };
        let (cancel, swallow) = ctx.state.lock().unwrap_or_else(|e| e.into_inner()).click(ty == LEFT_MOUSE_DOWN, at.x, at.y);
        if let Some(output) = cancel {
            (ctx.send)(output);
        }
        if swallow {
            return null_mut();
        }
        if ty == LEFT_MOUSE_UP {
            return event;
        }
    }
    if POINTER_EVENTS.contains(&ty) {
        let outputs = ctx.state.lock().unwrap_or_else(|e| e.into_inner()).handle(Input::Pointer).0;
        for output in outputs {
            (ctx.send)(output);
        }
        return event;
    }
    let (keycode, flags, repeat, pid) = unsafe {
        (
            CGEventGetIntegerValueField(event, FIELD_KEYCODE),
            CGEventGetFlags(event),
            CGEventGetIntegerValueField(event, FIELD_AUTOREPEAT) != 0,
            CGEventGetIntegerValueField(event, FIELD_SOURCE_PID),
        )
    };
    // Our own synthetic keys (Cmd+V paste) must never trigger hotkeys.
    if pid == std::process::id() as i64 {
        return event;
    }
    let mut outputs = Vec::new();
    let consume = {
        let mut st = ctx.state.lock().unwrap_or_else(|e| e.into_inner());
        let changed = match ty {
            FLAGS_CHANGED => MODIFIER_KEYS.iter().find(|(k, _)| *k == keycode).map_or(0, |(_, bit)| *bit),
            _ => 0,
        };
        if ty == FLAGS_CHANGED {
            st.retain_keys(|code| keycode_of(code).is_some_and(|k| unsafe { CGEventSourceKeyState(0, k) }));
        }
        // Resync from the event's flags (releases first) so a missed event can't leave a modifier stuck.
        let target = mods_from_flags(flags, st.mods(), changed);
        let diff = st.mods() ^ target;
        for down in [false, true] {
            for (_, bit) in MODIFIER_KEYS {
                if diff & bit != 0 && (target & bit != 0) == down {
                    outputs.extend(st.handle(Input::Modifier(bit, down)).0);
                }
            }
        }
        let code = KEYS.iter().find(|(k, _)| *k as i64 == keycode).map_or("Unidentified", |(_, name)| *name);
        let input = match ty {
            KEY_DOWN => Some(Input::KeyDown(code, repeat)),
            KEY_UP => Some(Input::KeyUp(code)),
            _ => None,
        };
        input.is_some_and(|input| {
            let (out, consume) = st.handle(input);
            outputs.extend(out);
            consume
        })
    };
    for output in outputs {
        (ctx.send)(output);
    }
    if consume { null_mut() } else { event }
}

/// Modifier bits implied by an event's flags. Device-dependent side bits decide; an event with
/// only the generic bit (some synthetic events) keeps what we tracked plus the key that changed.
/// The Fn flag also rides on arrows/F-keys, so only a keycode-63 event can turn Fn on.
fn mods_from_flags(flags: u64, held: u16, changed: u16) -> u16 {
    let mut mods = 0;
    for (generic, sides) in SIDES {
        let group = sides[0].1 | sides[1].1;
        let device = sides.iter().filter(|(mask, _)| flags & mask != 0).fold(0, |m, (_, bit)| m | bit);
        mods |= match (flags & generic != 0, device) {
            (false, _) => 0,
            (true, 0) => (held | changed) & group,
            (true, device) => device,
        };
    }
    if flags & FLAG_FN != 0 {
        mods |= (held | changed) & FN;
    }
    mods
}

fn keycode_of(code: &str) -> Option<u16> {
    KEYS.iter().find(|(_, name)| *name == code).map(|(k, _)| *k)
}

/// True when another process registered `b` with RegisterEventHotKey: an exclusive registration
/// of the same combo then fails with eventHotKeyExistsErr (-9878). Modifier-only and Fn
/// bindings can't be registered that way, so they're never reported.
pub fn hotkey_taken(b: &Binding) -> bool {
    let Some(keycode) = b.key.and_then(keycode_of) else { return false };
    if b.exact & FN != 0 {
        return false;
    }
    let held = b.exact | b.any;
    // controlKey, optionKey, shiftKey, cmdKey
    let carbon = GROUPS.iter().zip([0x1000, 0x800, 0x200, 0x100]).filter(|(g, _)| held & *g != 0).fold(0, |m, (_, c)| m | c);
    let id = HotKeyId { signature: u32::from_be_bytes(*b"OGlp"), id: 1 };
    let mut hotkey = null_mut();
    // kEventHotKeyExclusive = 1
    let status = unsafe { RegisterEventHotKey(keycode as u32, carbon, id, GetApplicationEventTarget(), 1, &mut hotkey) };
    if status == 0 {
        unsafe { UnregisterEventHotKey(hotkey) };
    }
    status == -9878
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_resync() {
        // Right ⌘ down: generic + device bit.
        assert_eq!(mods_from_flags(0x10_0010, 0, META_RIGHT), META_RIGHT);
        // Left ⌥ + Left ⌃ held, then an event without them: both cleared.
        assert_eq!(mods_from_flags(0x100, ALT_LEFT | CONTROL_LEFT, 0), 0);
        // Synthetic event with only the generic ⌘ bit keeps the tracked side / adds the changed key.
        assert_eq!(mods_from_flags(0x10_0000, META_LEFT, 0), META_LEFT);
        assert_eq!(mods_from_flags(0x10_0000, 0, META_LEFT), META_LEFT);
        // Fn flag: set only by the Fn key itself, cleared by any event without it.
        assert_eq!(mods_from_flags(FLAG_FN, 0, 0), 0, "arrow keys carry the Fn flag");
        assert_eq!(mods_from_flags(FLAG_FN, 0, FN), FN);
        assert_eq!(mods_from_flags(FLAG_FN, FN, 0), FN);
        assert_eq!(mods_from_flags(0, FN, 0), 0);
    }

    #[test]
    fn modifier_keys_are_not_in_the_key_table() {
        for (k, _) in MODIFIER_KEYS {
            assert!(KEYS.iter().all(|(key, _)| *key as i64 != k));
        }
    }
}
