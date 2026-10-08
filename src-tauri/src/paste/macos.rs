//! Paste events carry their own Command flag instead of depending on the shared session's
//! modifier state, which physical keyboard/mouse events can reset between Command and V.

use std::ffi::c_void;
use std::thread::sleep;
use std::time::Duration;

type Ref = *mut c_void;
const PRIVATE_STATE: i32 = -1;
const HID_TAP: u32 = 0;
const COMMAND_KEY: u16 = 55;
// Generic Command plus the device-dependent left-Command bit for COMMAND_KEY.
const COMMAND_FLAG: u64 = 0x10_0000 | 0x8;
const FIELD_USER_DATA: u32 = 42;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn CGEventSourceCreate(state: i32) -> Ref;
    fn CGEventCreateKeyboardEvent(source: Ref, keycode: u16, down: bool) -> Ref;
    fn CGEventSetFlags(event: Ref, flags: u64);
    fn CGEventSetIntegerValueField(event: Ref, field: u32, value: i64);
    fn CGEventPost(tap: u32, event: Ref);
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(value: Ref);
}

struct Owned(Ref);

impl Owned {
    fn new(value: Ref) -> Result<Self, String> {
        if value.is_null() {
            Err("Couldn't create paste keyboard event; the text is on the clipboard".into())
        } else {
            Ok(Self(value))
        }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

struct PasteEvents {
    events: [Owned; 4],
    // The private state table belongs to this source; keep it alive until all events are released.
    _source: Owned,
}

fn paste_events(keycode: u16) -> Result<PasteEvents, String> {
    let source = Owned::new(unsafe { CGEventSourceCreate(PRIVATE_STATE) })?;
    let event = |key, down, flags| {
        let event = Owned::new(unsafe { CGEventCreateKeyboardEvent(source.0, key, down) })?;
        unsafe {
            CGEventSetFlags(event.0, flags);
            CGEventSetIntegerValueField(event.0, FIELD_USER_DATA, enigo::EVENT_MARKER as i64);
        }
        Ok::<_, String>(event)
    };
    // Allocate the entire sequence before posting anything: failure must not leave Command down.
    let events = [
        event(COMMAND_KEY, true, COMMAND_FLAG)?,
        event(keycode, true, COMMAND_FLAG)?,
        event(keycode, false, COMMAND_FLAG)?,
        event(COMMAND_KEY, false, 0)?,
    ];
    Ok(PasteEvents {
        events,
        _source: source,
    })
}

pub(super) fn press_paste() -> Result<(), String> {
    // Called on the main thread, as required by the Text Input Sources layout lookup.
    let keycode = u16::try_from(enigo::Key::Unicode('v'))
        .map_err(|_| "Couldn't resolve the paste key; the text is on the clipboard".to_owned())?;
    let events = paste_events(keycode)?;
    for event in &events.events {
        sleep(Duration::from_millis(20));
        unsafe { CGEventPost(HID_TAP, event.0) };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIELD_KEYCODE: u32 = 9;
    const FIELD_SOURCE_PID: u32 = 41;
    const FIELD_SOURCE_STATE: u32 = 45;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn CGEventSourceGetSourceStateID(source: Ref) -> i32;
        fn CGEventGetFlags(event: Ref) -> u64;
        fn CGEventGetType(event: Ref) -> u32;
        fn CGEventGetIntegerValueField(event: Ref, field: u32) -> i64;
    }

    #[test]
    fn paste_keys_have_command_without_posting_a_modifier_first() {
        // No events are posted, so the session never sees Command down. This exercises the
        // real native events without Accessibility permission or changing any focused field.
        let sequence = paste_events(9).unwrap();
        let events = &sequence.events;
        let state = unsafe { CGEventSourceGetSourceStateID(sequence._source.0) } as i64;
        assert_ne!(state, 0); // Combined session state.
        assert_ne!(state, 1); // Hardware state.
        for event in &events[..3] {
            assert_eq!(unsafe { CGEventGetFlags(event.0) }, COMMAND_FLAG);
        }
        assert_eq!(unsafe { CGEventGetFlags(events[3].0) }, 0);
        for event in events {
            assert_eq!(
                unsafe { CGEventGetIntegerValueField(event.0, FIELD_SOURCE_STATE) },
                state
            );
            assert_eq!(
                unsafe { CGEventGetIntegerValueField(event.0, FIELD_SOURCE_PID) },
                std::process::id() as i64
            );
            assert_eq!(
                unsafe { CGEventGetIntegerValueField(event.0, FIELD_USER_DATA) },
                enigo::EVENT_MARKER as i64
            );
        }
    }

    #[test]
    fn paste_sequence_uses_the_resolved_layout_key_and_releases_command() {
        // V is keycode 9 on QWERTY, 47 on Dvorak; the supplied layout resolution is preserved.
        for keycode in [9, 47] {
            let sequence = paste_events(keycode).unwrap();
            let events = &sequence.events;
            for (event, (key, kind)) in events.iter().zip([
                (COMMAND_KEY, 12),
                (keycode, 10),
                (keycode, 11),
                (COMMAND_KEY, 12),
            ]) {
                assert_eq!(
                    unsafe { CGEventGetIntegerValueField(event.0, FIELD_KEYCODE) },
                    key as i64
                );
                assert_eq!(unsafe { CGEventGetType(event.0) }, kind);
            }
            assert_eq!(unsafe { CGEventGetFlags(events[3].0) }, 0);
        }
    }
}
