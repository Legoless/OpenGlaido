//! Global dictation hotkeys.
//!
//! Binding strings are "+"-joined tokens (canonical order: modifiers, then at most one key):
//! - side-specific modifiers: `Fn`, `ControlLeft`, `ControlRight`, `AltLeft`, `AltRight`,
//!   `ShiftLeft`, `ShiftRight`, `MetaLeft`, `MetaRight`
//! - side-agnostic modifiers: `Control`, `Alt`, `Shift`, `Meta`
//! - keys: `KeyboardEvent.code` names (`Space`, `KeyA`, `Digit1`, `F5`, `Enter`, `ArrowUp`, ...)
//! - "" means "disabled".
//!
//! macOS allows modifier-only bindings (e.g. `Fn`, `AltLeft+ControlLeft`); other platforms
//! require exactly one non-modifier key and no `Fn`.
//!
//! `chord` holds the parser and the pure state machine, `macos` the native event tap. This file
//! is the tauri glue plus the tauri-plugin-global-shortcut backend used off macOS.

// The state machine itself only runs behind the macOS tap; elsewhere it just holds settings.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod chord;
#[cfg(target_os = "macos")]
mod macos;

pub use chord::HotkeyEvent;

use chord::{Binding, Output, State};
use serde::Serialize;
use std::str::FromStr;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use tauri::{AppHandle, Emitter};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use HotkeyEvent::*;

/// macOS runs the native event tap; everything else the global-shortcut plugin.
const NATIVE: bool = cfg!(target_os = "macos");

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HotkeyStatus {
    /// "native" (macOS event tap) or "plugin" (tauri-plugin-global-shortcut).
    pub engine: String,
    /// False when the OS permission the engine needs (macOS Accessibility) is missing.
    pub permission_granted: bool,
    /// Last registration/backend error, if any.
    pub error: Option<String>,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
enum Msg {
    Out(Output),
    /// Plugin shortcut id + pressed. Resolved on the dispatcher thread because the plugin holds
    /// its shortcut lock while calling us (and set_bindings takes our lock, then the plugin's).
    Plugin(u32, bool),
}

/// Cheap to clone; manage it with `app.manage(engine)`.
#[derive(Clone)]
pub struct HotkeyEngine {
    inner: Arc<Inner>,
}

struct Inner {
    app: AppHandle,
    state: Arc<Mutex<State>>,
    status: Mutex<HotkeyStatus>,
    tx: Sender<Msg>,
}

impl Inner {
    fn set_status(&self, permission_granted: bool, error: Option<String>) {
        let mut status = self.status.lock().unwrap_or_else(|e| e.into_inner());
        if status.permission_granted == permission_granted && status.error == error {
            return;
        }
        if let Some(e) = &error {
            eprintln!("[hotkeys] {e}");
        }
        status.permission_granted = permission_granted;
        status.error = error;
        let _ = self.app.emit("hotkey-status", status.clone());
    }
}

impl HotkeyEngine {
    /// Starts the platform backend. `on_event` runs on a background thread and must not block.
    pub fn start(app: &AppHandle, on_event: impl Fn(HotkeyEvent) + Send + Sync + 'static) -> Self {
        let state = Arc::new(Mutex::new(State::default()));
        let (tx, rx) = channel();
        // One dispatcher thread keeps events ordered and off the tap/plugin callbacks.
        let (app2, state2) = (app.clone(), state.clone());
        std::thread::spawn(move || {
            for msg in rx {
                let event = match msg {
                    Msg::Out(Output::Event(e)) => Some(e),
                    Msg::Out(Output::Capture { keys, done }) => {
                        let _ = app2.emit("hotkey-capture", serde_json::json!({ "keys": keys, "done": done }));
                        None
                    }
                    Msg::Plugin(id, pressed) => plugin_event(&state2.lock().unwrap_or_else(|e| e.into_inner()), id, pressed),
                };
                if let Some(e) = event {
                    on_event(e);
                }
            }
        });

        #[cfg(target_os = "macos")]
        let status = {
            let granted = macos::trusted(false);
            let error = (!granted).then(|| macos::NEEDS_ACCESS.to_string());
            HotkeyStatus { engine: "native".into(), permission_granted: granted, error }
        };
        #[cfg(not(target_os = "macos"))]
        let status = HotkeyStatus { engine: "plugin".into(), permission_granted: true, error: None };

        let inner = Arc::new(Inner { app: app.clone(), state, status: Mutex::new(status), tx });
        #[cfg(target_os = "macos")]
        {
            let (tx, status_inner) = (inner.tx.clone(), inner.clone());
            macos::spawn(
                inner.state.clone(),
                move |output| {
                    let _ = tx.send(Msg::Out(output));
                },
                move |granted, error| status_inner.set_status(granted, error),
            );
        }
        Self { inner }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Validates and applies both bindings atomically; on Err the previous bindings stay active.
    pub fn set_bindings(&self, hold: &str, toggle: &str) -> Result<(), String> {
        let hold = chord::validate_for(NATIVE, hold)?;
        let toggle = chord::validate_for(NATIVE, toggle)?;
        if hold.is_some() && hold == toggle {
            return Err("Dictation and hands-free dictation need different hotkeys".into());
        }
        let mut st = self.lock();
        if NATIVE || st.capturing() {
            st.set_bindings(hold, toggle);
            return Ok(());
        }
        let old = (st.hold.clone(), st.toggle.clone());
        self.plugin_off(&st);
        st.set_bindings(hold, toggle);
        if let Err(e) = self.plugin_on(&st) {
            st.set_bindings(old.0, old.1);
            let _ = self.plugin_on(&st);
            return Err(e);
        }
        Ok(())
    }

    /// Arms/disarms Esc -> `HotkeyEvent::Cancel` (and consumes Esc while armed where possible).
    pub fn set_recording(&self, recording: bool) {
        let mut st = self.lock();
        if st.recording != recording {
            st.recording = recording;
            if !NATIVE && !st.capturing() {
                self.plugin_arm(esc(), recording);
            }
        }
    }

    /// Arms/disarms Enter -> `HotkeyEvent::Submit`.
    pub fn set_submit_enabled(&self, enabled: bool) {
        let mut st = self.lock();
        if st.submit != enabled {
            st.submit = enabled;
            if !NATIVE && !st.capturing() {
                self.plugin_arm(enter(), enabled);
            }
        }
    }

    /// Suspends all hotkey events until `stop_capture`. Returns true when native capture is
    /// available: the engine then emits "hotkey-capture" events with payload
    /// `{ "keys": "<canonical binding>", "done": bool }` to all windows.
    pub fn start_capture(&self) -> bool {
        let mut st = self.lock();
        if !NATIVE && !st.capturing() {
            self.plugin_off(&st);
        }
        st.set_capture(true);
        // Without a live tap (no Accessibility / tap failed) the webview has to capture keys itself.
        let status = self.status();
        NATIVE && status.permission_granted && status.error.is_none()
    }

    pub fn stop_capture(&self) {
        let mut st = self.lock();
        if !st.capturing() {
            return;
        }
        st.set_capture(false);
        if !NATIVE {
            if let Err(e) = self.plugin_on(&st) {
                self.inner.set_status(true, Some(e));
            }
        }
    }

    pub fn status(&self) -> HotkeyStatus {
        self.inner.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Forwarded from the tauri-plugin-global-shortcut handler in lib.rs (plugin backend only).
    pub fn handle_plugin_event(&self, shortcut: &Shortcut, state: ShortcutState) {
        if !NATIVE {
            let _ = self.inner.tx.send(Msg::Plugin(shortcut.id(), state == ShortcutState::Pressed));
        }
    }

    /// Plugin backend: registers hold/toggle (rolling back on failure), then Esc/Enter if armed.
    fn plugin_on(&self, st: &State) -> Result<(), String> {
        let gs = self.inner.app.global_shortcut();
        let mut registered = Vec::new();
        for b in [&st.hold, &st.toggle].into_iter().flatten() {
            let Some(s) = shortcut(b) else { continue };
            if let Err(e) = gs.register(s) {
                for s in registered {
                    let _ = gs.unregister(s);
                }
                return Err(format!("{} is already used by another app ({e})", chord::label(b, false)));
            }
            registered.push(s);
        }
        if st.recording {
            self.plugin_arm(esc(), true);
        }
        if st.submit {
            self.plugin_arm(enter(), true);
        }
        Ok(())
    }

    fn plugin_off(&self, st: &State) {
        let gs = self.inner.app.global_shortcut();
        let bindings = [&st.hold, &st.toggle].into_iter().flatten().filter_map(shortcut);
        for s in bindings.chain([esc(), enter()]) {
            let _ = gs.unregister(s);
        }
    }

    /// Esc/Enter are only grabbed while armed so they keep working in other apps.
    fn plugin_arm(&self, s: Shortcut, on: bool) {
        let gs = self.inner.app.global_shortcut();
        if !on {
            let _ = gs.unregister(s);
        } else if let Err(e) = gs.register(s) {
            eprintln!("[hotkeys] couldn't register {}: {e}", s.into_string());
        }
    }
}

fn esc() -> Shortcut {
    Shortcut::new(None, Code::Escape)
}

fn enter() -> Shortcut {
    Shortcut::new(None, Code::Enter)
}

/// Plugin shortcut for a binding; sides collapse to generic modifiers. None for modifier-only.
fn shortcut(b: &Binding) -> Option<Shortcut> {
    let code = Code::from_str(b.key?).ok()?;
    let held = b.exact | b.any;
    let flags = [Modifiers::CONTROL, Modifiers::ALT, Modifiers::SHIFT, Modifiers::SUPER];
    let mods = chord::GROUPS.iter().zip(flags).filter(|(g, _)| held & *g != 0).fold(Modifiers::empty(), |m, (_, f)| m | f);
    Some(Shortcut::new(Some(mods), code))
}

fn plugin_event(st: &State, id: u32, pressed: bool) -> Option<HotkeyEvent> {
    let is = |b: &Option<Binding>| b.as_ref().and_then(shortcut).is_some_and(|s| s.id() == id);
    if st.capturing() {
        None
    } else if is(&st.hold) {
        Some(if pressed { HoldPressed } else { HoldReleased })
    } else if !pressed {
        None
    } else if is(&st.toggle) {
        Some(TogglePressed)
    } else if st.recording && id == esc().id() {
        Some(Cancel)
    } else if st.submit && id == enter().id() {
        Some(Submit)
    } else {
        None
    }
}

/// Ok if `s` is "" or a binding this platform can register.
pub fn validate_binding(s: &str) -> Result<(), String> {
    chord::validate_for(NATIVE, s).map(|_| ())
}

/// Human-readable warning when another app already owns `s` (macOS: exclusive Carbon probe).
pub fn conflict_warning(s: &str) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let b = chord::parse(s).ok()??;
        if macos::hotkey_taken(&b) {
            return Some(format!(
                "Another app already uses {}. OpenGlaido still receives it first, but that app's shortcut will stop working.",
                chord::label(&b, true)
            ));
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = s;
    None
}

pub fn default_hold() -> &'static str {
    if cfg!(target_os = "macos") { "Fn" } else { "Control+Shift+Space" }
}

pub fn default_toggle() -> &'static str {
    if cfg!(target_os = "macos") { "Fn+Space" } else { "Control+Shift+Alt+Space" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_distinct() {
        assert!(validate_binding(default_hold()).is_ok());
        assert!(validate_binding(default_toggle()).is_ok());
        assert!(chord::validate_for(false, "Control+Shift+Space").is_ok());
        assert!(chord::validate_for(false, "Control+Shift+Alt+Space").is_ok());
        assert_ne!(default_hold(), default_toggle());
    }

    #[test]
    fn every_key_token_maps_to_a_plugin_shortcut() {
        for (_, key) in chord::KEYS.iter().chain(&[(0, "PrintScreen"), (0, "ScrollLock"), (0, "Pause")]) {
            let b = chord::parse(&format!("ControlLeft+Shift+{key}")).unwrap().unwrap();
            let s = shortcut(&b).unwrap_or_else(|| panic!("{key}"));
            assert_eq!(s.mods, Modifiers::CONTROL | Modifiers::SHIFT);
        }
        assert!(shortcut(&chord::parse("AltLeft+ControlLeft").unwrap().unwrap()).is_none());
    }

    #[test]
    fn plugin_events_map_to_hotkey_events() {
        let mut st = State::default();
        let parse = |s| chord::parse(s).unwrap();
        st.set_bindings(parse("Control+Shift+Space"), parse("ControlLeft+Shift+Alt+Space"));
        let id = |s| shortcut(&chord::parse(s).unwrap().unwrap()).unwrap().id();
        let hold = id("Control+Shift+Space");
        let toggle = id("Control+Shift+Alt+Space");
        assert_eq!(plugin_event(&st, hold, true), Some(HoldPressed));
        assert_eq!(plugin_event(&st, hold, false), Some(HoldReleased));
        assert_eq!(plugin_event(&st, toggle, true), Some(TogglePressed));
        assert_eq!(plugin_event(&st, toggle, false), None);
        assert_eq!(plugin_event(&st, esc().id(), true), None);
        assert_eq!(plugin_event(&st, enter().id(), true), None);
        st.recording = true;
        st.submit = true;
        assert_eq!(plugin_event(&st, esc().id(), true), Some(Cancel));
        assert_eq!(plugin_event(&st, enter().id(), true), Some(Submit));
        st.set_capture(true);
        assert_eq!(plugin_event(&st, hold, true), None);
    }
}
