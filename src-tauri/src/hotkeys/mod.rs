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
        let copy_group = if NATIVE { chord::GROUPS[3] } else { chord::GROUPS[0] };
        let mut state = State::default();
        state.copy_group = copy_group;
        let state = Arc::new(Mutex::new(state));
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
        self.set_pair(0, hold, toggle, "Dictation and hands-free dictation need different hotkeys")
    }

    /// Commands hold/hands-free bindings; same rules as `set_bindings`, unique across all four.
    pub fn set_command_bindings(&self, hold: &str, toggle: &str) -> Result<(), String> {
        self.set_pair(1, hold, toggle, "Commands and hands-free commands need different hotkeys")
    }

    fn set_pair(&self, idx: usize, hold: &str, toggle: &str, same_msg: &str) -> Result<(), String> {
        let hold = chord::validate_for(NATIVE, hold)?;
        let toggle = chord::validate_for(NATIVE, toggle)?;
        if hold.is_some() && hold == toggle {
            return Err(same_msg.into());
        }
        if let (Some(h), Some(t)) = (&hold, &toggle) {
            if clash(h, t) {
                return Err(same_msg.into());
            }
        }
        let mut st = self.lock();
        // Unchanged: nothing to do (and an active hold must not lose its release).
        if st.pairs[idx].hold == hold && st.pairs[idx].toggle == toggle {
            return Ok(());
        }
        let other = &st.pairs[1 - idx];
        let taken = [&hold, &toggle]
            .into_iter()
            .flatten()
            .find(|b| [&other.hold, &other.toggle].into_iter().flatten().any(|o| clash(b, o)));
        if let Some(b) = taken {
            let what = if idx == 0 { "a command" } else { "a dictation" };
            return Err(format!("{} is already {what} hotkey", chord::label(b, NATIVE)));
        }
        for output in st.abort_active() {
            let _ = self.inner.tx.send(Msg::Out(output));
        }
        let set = |st: &mut State, h, t| if idx == 0 { st.set_bindings(h, t) } else { st.set_command_bindings(h, t) };
        if NATIVE || st.capturing() {
            set(&mut st, hold, toggle);
            return Ok(());
        }
        let old = (st.pairs[idx].hold.clone(), st.pairs[idx].toggle.clone());
        self.plugin_off(&st);
        set(&mut st, hold, toggle);
        if let Err(e) = self.plugin_on(&st) {
            set(&mut st, old.0, old.1);
            let _ = self.plugin_on(&st);
            return Err(e);
        }
        Ok(())
    }

    /// The hold of pair `idx` (0 dictation, 1 commands) is modifier-only, so its press may still
    /// turn out to be ⌥e or fn+→.
    pub fn hold_is_modifier_only(&self, idx: usize) -> bool {
        self.lock().pairs[idx].hold.as_ref().is_some_and(|b| b.key.is_none())
    }

    /// Arms/disarms Esc -> `HotkeyEvent::Cancel` (and consumes Esc while armed where possible).
    pub fn set_recording(&self, recording: bool) {
        self.update_armed(|st| st.recording = recording);
    }

    /// Screen rect (points) of the dictation bar's cancel "x", or None while it isn't shown.
    /// macOS only: the tap turns a click there into `HotkeyEvent::Cancel`.
    pub fn set_cancel_area(&self, area: Option<[f64; 4]>) {
        self.lock().cancel_area = area;
    }

    /// Arms/disarms Enter -> `HotkeyEvent::Submit`.
    pub fn set_submit_enabled(&self, enabled: bool) {
        self.update_armed(|st| st.submit = enabled);
    }

    /// Command window keys: Esc while it is shown, Enter while there is something to paste or
    /// approve, ⌘C/Ctrl+C while an answer is ready. Anything else types normally.
    pub fn set_command_keys(&self, esc: bool, enter: bool, copy: bool) {
        self.update_armed(|st| {
            st.command_esc = esc;
            st.command_enter = esc && enter;
            st.command_copy = esc && copy;
        });
    }

    /// Changes arming flags; the plugin backend (re)grabs Esc/Enter/Ctrl+C to match.
    fn update_armed(&self, change: impl FnOnce(&mut State)) {
        let mut st = self.lock();
        let before = armed(&st);
        change(&mut st);
        let after = armed(&st);
        if !NATIVE && !st.capturing() {
            for ((s, was), now) in [esc(), enter(), copy()].into_iter().zip(before).zip(after) {
                if was != now {
                    self.plugin_arm(s, now);
                }
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
        for output in st.abort_active() {
            let _ = self.inner.tx.send(Msg::Out(output));
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
        for b in bindings(st) {
            let Some(s) = shortcut(b) else { continue };
            if let Err(e) = gs.register(s) {
                for s in registered {
                    let _ = gs.unregister(s);
                }
                return Err(format!("{} is already used by another app ({e})", chord::label(b, false)));
            }
            registered.push(s);
        }
        for (s, on) in [esc(), enter(), copy()].into_iter().zip(armed(st)) {
            if on {
                self.plugin_arm(s, true);
            }
        }
        Ok(())
    }

    fn plugin_off(&self, st: &State) {
        let gs = self.inner.app.global_shortcut();
        for s in bindings(st).filter_map(shortcut).chain([esc(), enter(), copy()]) {
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

fn copy() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL), Code::KeyC)
}

/// All hold/toggle bindings of both pairs.
fn bindings(st: &State) -> impl Iterator<Item = &Binding> {
    st.pairs.iter().flat_map(|p| [&p.hold, &p.toggle]).flatten()
}

/// Whether Esc, Enter and Ctrl+C should currently be grabbed (plugin backend).
fn armed(st: &State) -> [bool; 3] {
    [st.recording || st.command_esc, st.submit || st.command_enter, st.command_copy]
}

/// Both bindings can fire on the same keys. Off macOS the plugin can't tell sides apart:
/// ControlLeft+KeyK and ControlRight+KeyK are one shortcut.
fn clash(a: &Binding, b: &Binding) -> bool {
    chord::overlaps(a, b) || (!NATIVE && shortcut(a).is_some_and(|s| Some(s.id()) == shortcut(b).map(|s| s.id())))
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
        return None;
    }
    let (dictation, commands) = (&st.pairs[0], &st.pairs[1]);
    if is(&dictation.hold) {
        Some(if pressed { HoldPressed } else { HoldReleased })
    } else if is(&commands.hold) {
        Some(if pressed { CommandHoldPressed } else { CommandHoldReleased })
    } else if !pressed {
        None
    } else if is(&dictation.toggle) {
        Some(TogglePressed)
    } else if is(&commands.toggle) {
        Some(CommandTogglePressed)
    } else if id == esc().id() {
        st.recording.then_some(Cancel).or(st.command_esc.then_some(CommandEscape))
    } else if id == enter().id() {
        st.submit.then_some(Submit).or(st.command_enter.then_some(CommandEnter))
    } else if id == copy().id() && st.command_copy {
        Some(CommandCopy)
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
    if cfg!(target_os = "macos") { "Fn+Space" } else { "Control+Shift+Meta+Space" }
}

pub fn default_commands_hold() -> &'static str {
    // Windows: never Ctrl+Alt without Win (that is AltGr on many layouts).
    if cfg!(target_os = "macos") { "AltRight" } else { "Control+Meta+KeyC" }
}

pub fn default_commands_toggle() -> &'static str {
    if cfg!(target_os = "macos") { "AltRight+Space" } else { "Control+Shift+Meta+KeyC" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_distinct() {
        assert!(validate_binding(default_hold()).is_ok());
        assert!(validate_binding(default_toggle()).is_ok());
        // The Windows defaults are valid there too (none uses Ctrl+Alt without Win).
        for b in ["Control+Shift+Space", "Control+Shift+Meta+Space", "Control+Meta+KeyC", "Control+Shift+Meta+KeyC"] {
            assert!(chord::validate_for(false, b).is_ok(), "{b}");
        }
        let all = [default_hold(), default_toggle(), default_commands_hold(), default_commands_toggle()];
        for (i, b) in all.iter().enumerate() {
            assert!(validate_binding(b).is_ok(), "{b}");
            assert!(!all[..i].contains(b), "{b} is used twice");
        }
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
    fn side_variants_clash_only_on_the_plugin() {
        let b = |s| chord::parse(s).unwrap().unwrap();
        let (left, right) = (b("ControlLeft+Meta+KeyK"), b("ControlRight+Meta+KeyK"));
        assert!(!chord::overlaps(&left, &right));
        assert_eq!(clash(&left, &right), !NATIVE);
        assert!(!clash(&b("Fn"), &b("AltRight")));
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
        st.set_capture(false);
        // Commands pair and command-window keys.
        st.recording = false;
        st.submit = false;
        st.set_command_bindings(parse("Control+Alt+KeyC"), parse("Control+Alt+Shift+KeyC"));
        let (cmd_hold, cmd_toggle) = (id("Control+Alt+KeyC"), id("Control+Alt+Shift+KeyC"));
        assert_eq!(plugin_event(&st, cmd_hold, true), Some(CommandHoldPressed));
        assert_eq!(plugin_event(&st, cmd_hold, false), Some(CommandHoldReleased));
        assert_eq!(plugin_event(&st, cmd_toggle, true), Some(CommandTogglePressed));
        assert_eq!(plugin_event(&st, esc().id(), true), None);
        st.command_esc = true;
        st.command_enter = true;
        assert_eq!(plugin_event(&st, esc().id(), true), Some(CommandEscape));
        assert_eq!(plugin_event(&st, enter().id(), true), Some(CommandEnter));
        assert_eq!(plugin_event(&st, copy().id(), true), None);
        st.command_copy = true;
        assert_eq!(plugin_event(&st, copy().id(), true), Some(CommandCopy));
        st.recording = true;
        assert_eq!(plugin_event(&st, esc().id(), true), Some(Cancel));
    }
}
