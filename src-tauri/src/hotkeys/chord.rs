//! Binding strings + the pure hotkey state machine. No tauri/OS types in here, so everything
//! unit-tests on any platform; the macOS tap and the plugin backend are thin shells around it.

use HotkeyEvent::*;

/// Emitted by the engine; lib.rs maps these onto the recording state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// Hold (push-to-talk) binding became fully pressed.
    HoldPressed,
    /// Hold binding released normally.
    HoldReleased,
    /// Another key/modifier joined the hold chord (and it is not the toggle chord): cancel.
    HoldAborted,
    /// Hands-free binding pressed (once per press, never on autorepeat).
    TogglePressed,
    /// Esc pressed while armed via `set_recording(true)`.
    Cancel,
    /// The processing bar's cancel click; never affects a recording that starts meanwhile.
    CancelProcessing,
    /// Enter pressed while armed via `set_submit_enabled(true)`.
    Submit,
    // Commands hotkeys: hold/toggle drive the same recording state machine; Enter/Esc/Copy go to
    // `commands::handle_hotkey`.
    /// Commands hold binding became fully pressed.
    CommandHoldPressed,
    /// Commands hold binding released normally.
    CommandHoldReleased,
    /// Another key/modifier joined the commands hold chord: cancel.
    CommandHoldAborted,
    /// Commands hands-free binding pressed.
    CommandTogglePressed,
    /// Enter while armed via `set_command_keys(open = true, ..)`.
    CommandEnter,
    /// Esc while armed via `set_command_keys(open = true, ..)`.
    CommandEscape,
    /// ⌘C / Ctrl+C while armed via `set_command_keys(.., answer_ready = true)`.
    CommandCopy,
}

// One bit per physical modifier key (Fn has no side).
pub const FN: u16 = 1;
pub const CONTROL_LEFT: u16 = 1 << 1;
pub const CONTROL_RIGHT: u16 = 1 << 2;
pub const ALT_LEFT: u16 = 1 << 3;
pub const ALT_RIGHT: u16 = 1 << 4;
pub const SHIFT_LEFT: u16 = 1 << 5;
pub const SHIFT_RIGHT: u16 = 1 << 6;
pub const META_LEFT: u16 = 1 << 7;
pub const META_RIGHT: u16 = 1 << 8;
/// Control, Alt, Shift, Meta — each group = both side bits.
pub const GROUPS: [u16; 4] = [
    CONTROL_LEFT | CONTROL_RIGHT,
    ALT_LEFT | ALT_RIGHT,
    SHIFT_LEFT | SHIFT_RIGHT,
    META_LEFT | META_RIGHT,
];

/// Modifier tokens in canonical order. Side-agnostic tokens carry both bits of their group.
const MODIFIERS: [(&str, u16); 13] = [
    ("Fn", FN),
    ("Control", GROUPS[0]),
    ("ControlLeft", CONTROL_LEFT),
    ("ControlRight", CONTROL_RIGHT),
    ("Alt", GROUPS[1]),
    ("AltLeft", ALT_LEFT),
    ("AltRight", ALT_RIGHT),
    ("Shift", GROUPS[2]),
    ("ShiftLeft", SHIFT_LEFT),
    ("ShiftRight", SHIFT_RIGHT),
    ("Meta", GROUPS[3]),
    ("MetaLeft", META_LEFT),
    ("MetaRight", META_RIGHT),
];

/// macOS virtual keycode → `KeyboardEvent.code`. Also the list of key tokens a binding may use.
pub const KEYS: &[(u16, &str)] = &[
    (0, "KeyA"), (1, "KeyS"), (2, "KeyD"), (3, "KeyF"), (4, "KeyH"), (5, "KeyG"), (6, "KeyZ"),
    (7, "KeyX"), (8, "KeyC"), (9, "KeyV"), (10, "IntlBackslash"), (11, "KeyB"), (12, "KeyQ"),
    (13, "KeyW"), (14, "KeyE"), (15, "KeyR"), (16, "KeyY"), (17, "KeyT"), (18, "Digit1"),
    (19, "Digit2"), (20, "Digit3"), (21, "Digit4"), (22, "Digit6"), (23, "Digit5"), (24, "Equal"),
    (25, "Digit9"), (26, "Digit7"), (27, "Minus"), (28, "Digit8"), (29, "Digit0"),
    (30, "BracketRight"), (31, "KeyO"), (32, "KeyU"), (33, "BracketLeft"), (34, "KeyI"),
    (35, "KeyP"), (36, "Enter"), (37, "KeyL"), (38, "KeyJ"), (39, "Quote"), (40, "KeyK"),
    (41, "Semicolon"), (42, "Backslash"), (43, "Comma"), (44, "Slash"), (45, "KeyN"), (46, "KeyM"),
    (47, "Period"), (48, "Tab"), (49, "Space"), (50, "Backquote"), (51, "Backspace"),
    (53, "Escape"), (64, "F17"), (65, "NumpadDecimal"), (67, "NumpadMultiply"), (69, "NumpadAdd"),
    (71, "NumLock"), (75, "NumpadDivide"), (76, "NumpadEnter"), (78, "NumpadSubtract"), (79, "F18"),
    (80, "F19"), (81, "NumpadEqual"), (82, "Numpad0"), (83, "Numpad1"), (84, "Numpad2"),
    (85, "Numpad3"), (86, "Numpad4"), (87, "Numpad5"), (88, "Numpad6"), (89, "Numpad7"),
    (90, "F20"), (91, "Numpad8"), (92, "Numpad9"), (96, "F5"), (97, "F6"), (98, "F7"), (99, "F3"),
    (100, "F8"), (101, "F9"), (103, "F11"), (105, "F13"), (106, "F16"), (107, "F14"), (109, "F10"),
    (111, "F12"), (113, "F15"), (114, "Insert"), (115, "Home"), (116, "PageUp"), (117, "Delete"),
    (118, "F4"), (119, "End"), (120, "F2"), (121, "PageDown"), (122, "F1"), (123, "ArrowLeft"),
    (124, "ArrowRight"), (125, "ArrowDown"), (126, "ArrowUp"),
];

/// Keys that exist on PC keyboards only.
const PC_ONLY_KEYS: [&str; 3] = ["PrintScreen", "ScrollLock", "Pause"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Binding {
    /// Side-specific modifier bits (and Fn) that must be held.
    pub exact: u16,
    /// Side-agnostic groups (both bits set): either or both sides satisfy it.
    pub any: u16,
    pub key: Option<&'static str>,
}

impl Binding {
    /// True when `held` is exactly this binding's modifier set (an extra modifier → false).
    pub fn matches(&self, held: u16) -> bool {
        held & !self.any == self.exact && GROUPS.iter().all(|g| self.any & g == 0 || held & g != 0)
    }
}

/// Canonical string: Fn, Control*, Alt*, Shift*, Meta*, key.
impl std::fmt::Display for Binding {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let mods = MODIFIERS.iter().filter(|(_, bits)| match bits.count_ones() {
            2 => self.any & bits == *bits,
            _ => self.exact & bits != 0,
        });
        let parts: Vec<&str> = mods.map(|(name, _)| *name).chain(self.key).collect();
        f.write_str(&parts.join("+"))
    }
}

fn key_token(token: &str) -> Option<&'static str> {
    KEYS.iter().map(|(_, name)| *name).chain(PC_ONLY_KEYS).find(|name| *name == token)
}

/// Parses tokens in any order; "" → `Ok(None)` (disabled). Platform rules live in `validate_for`.
pub fn parse(s: &str) -> Result<Option<Binding>, String> {
    if s.is_empty() {
        return Ok(None);
    }
    let mut b = Binding::default();
    for token in s.split('+') {
        if let Some(&(_, bits)) = MODIFIERS.iter().find(|(name, _)| *name == token) {
            if (b.exact | b.any) & bits != 0 {
                return Err(format!("\"{token}\" is repeated or overlaps another modifier in \"{s}\""));
            }
            if bits.count_ones() == 2 {
                b.any |= bits;
            } else {
                b.exact |= bits;
            }
        } else if let Some(key) = key_token(token) {
            if let Some(first) = b.key {
                return Err(format!("Only one non-modifier key is allowed (got {first} and {key})"));
            }
            b.key = Some(key);
        } else if token.is_empty() {
            return Err(format!("\"{s}\" has an empty key"));
        } else {
            let names = MODIFIERS.iter().map(|(name, _)| *name).chain(KEYS.iter().map(|(_, name)| *name));
            return Err(match names.chain(PC_ONLY_KEYS).find(|name| name.eq_ignore_ascii_case(token)) {
                Some(name) => format!("Unknown key \"{token}\" (did you mean \"{name}\"?)"),
                None => format!("Unknown key \"{token}\""),
            });
        }
    }
    Ok(Some(b))
}

/// `parse` + the platform rules: only macOS allows `Fn` and modifier-only bindings.
pub fn validate_for(platform_is_mac: bool, s: &str) -> Result<Option<Binding>, String> {
    let Some(b) = parse(s)? else { return Ok(None) };
    if platform_is_mac {
        if let Some(key) = b.key.filter(|k| PC_ONLY_KEYS.contains(k)) {
            return Err(format!("{key} doesn't exist on Mac keyboards"));
        }
    } else if b.exact & FN != 0 {
        return Err("fn can't be used on Windows".into());
    } else if b.key.is_none() {
        return Err("Add a key — only modifier keys are supported alone on macOS".into());
    } else if (b.exact | b.any) & GROUPS[0] != 0 && (b.exact | b.any) & GROUPS[1] != 0 && (b.exact | b.any) & GROUPS[3] == 0 {
        // AltGr arrives as Ctrl+Alt: such a hotkey would eat characters like ć, @ or & on many layouts.
        return Err("Ctrl+Alt is AltGr on many keyboards — add Win or choose another combination".into());
    }
    // A typing key without ⌃/⌥/⌘/fn would be swallowed in every app (Shift alone still types).
    if let Some(key) = b.key {
        let bare_ok = PC_ONLY_KEYS.contains(&key) || key.strip_prefix('F').is_some_and(|n| n.parse::<u8>().is_ok());
        if (b.exact | b.any) & !GROUPS[2] == 0 && !bare_ok {
            return Err(format!("Add a modifier key — {} alone would stop working for typing in every app", label(&b, platform_is_mac)));
        }
    }
    Ok(Some(b))
}

/// True when some physical modifier set triggers both bindings (same key or both modifier-only).
pub fn overlaps(a: &Binding, b: &Binding) -> bool {
    a.key == b.key && (0u16..512).any(|held| a.matches(held) && b.matches(held))
}

/// Short, side-less label for messages: "⌃⇧Space" on macOS, "Ctrl+Shift+Space" elsewhere.
pub fn label(b: &Binding, mac: bool) -> String {
    let held = b.exact | b.any;
    let names = if mac { ["⌃", "⌥", "⇧", "⌘"] } else { ["Ctrl", "Alt", "Shift", "Win"] };
    let mut parts: Vec<&str> = Vec::new();
    if held & FN != 0 {
        parts.push("fn");
    }
    parts.extend(GROUPS.iter().zip(names).filter(|(g, _)| held & *g != 0).map(|(_, name)| name));
    parts.extend(b.key.map(|k| k.strip_prefix("Key").or(k.strip_prefix("Digit")).unwrap_or(k)));
    parts.join(if mac { "" } else { "+" })
}

pub enum Input {
    /// Non-modifier key down (`KeyboardEvent.code`, is_autorepeat).
    KeyDown(&'static str, bool),
    KeyUp(&'static str),
    /// A modifier bit went down (true) or up (false).
    Modifier(u16, bool),
    /// A mouse button or scroll: like a key for modifier-only holds and taps (never swallowed).
    Pointer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    Event(HotkeyEvent),
    /// Native capture progress: canonical binding so far; `done` = final ("" = cancelled).
    Capture { keys: String, done: bool },
}

/// One hold/hands-free binding pair plus its press tracking.
#[derive(Default)]
pub struct Pair {
    pub hold: Option<Binding>,
    pub toggle: Option<Binding>,
    /// The hold "pressed" event was emitted and "released"/"aborted" not yet.
    hold_active: bool,
    /// Modifier-only toggle: exactly its modifiers are held and nothing else was touched.
    tap_armed: bool,
    /// Another key/modifier joined since the first modifier went down: no tap this time.
    tap_spoiled: bool,
}

impl Pair {
    fn set(&mut self, hold: Option<Binding>, toggle: Option<Binding>) {
        self.hold = hold;
        self.toggle = toggle;
        self.reset();
    }

    fn reset(&mut self) {
        self.hold_active = false;
        self.tap_armed = false;
    }
}

/// Events of pair 0 (dictation) and pair 1 (commands): hold pressed/released/aborted, toggle.
const PAIR_EVENTS: [[HotkeyEvent; 4]; 2] = [
    [HoldPressed, HoldReleased, HoldAborted, TogglePressed],
    [CommandHoldPressed, CommandHoldReleased, CommandHoldAborted, CommandTogglePressed],
];

#[derive(Default)]
pub struct State {
    /// [dictation, commands]
    pub pairs: [Pair; 2],
    /// Esc → Cancel while set.
    pub recording: bool,
    /// Screen rect (x, y, width, height in points) of the dictation bar's cancel "x" while a
    /// hands-free recording shows it. A left click there → Cancel, and the click is swallowed.
    pub cancel_area: Option<[f64; 4]>,
    pub cancel_processing: bool,
    /// The cancel click's button-up still has to be swallowed.
    swallow_up: bool,
    /// Enter → Submit while set.
    pub submit: bool,
    /// Command window open: Esc → CommandEscape (dictation arming wins).
    pub command_esc: bool,
    /// Answer ready or approval pending: Enter → CommandEnter.
    pub command_enter: bool,
    /// Command answer ready: ⌘C (macOS) / Ctrl+C → CommandCopy.
    pub command_copy: bool,
    /// Which modifier group copies (Meta on macOS, Control elsewhere).
    pub copy_group: u16,
    capturing: bool,
    capture_max: u16,
    mods: u16,
    keys_down: Vec<&'static str>,
    /// Keys whose keydown we swallowed: their repeats and keyup are swallowed too.
    consumed: Vec<&'static str>,
}

impl State {
    pub fn mods(&self) -> u16 {
        self.mods
    }

    pub fn capturing(&self) -> bool {
        self.capturing
    }

    /// Left button down/up at screen point (x, y). Returns (Cancel event, swallow): a click on the
    /// bar's cancel "x" cancels like Esc, and both of its halves are kept from the app underneath.
    pub fn click(&mut self, down: bool, x: f64, y: f64) -> (Option<Output>, bool) {
        if !down {
            return (None, std::mem::take(&mut self.swallow_up));
        }
        let hit = self.cancel_area.is_some_and(|[left, top, w, h]| x >= left && x < left + w && y >= top && y < top + h);
        self.swallow_up = hit;
        if hit && !self.cancel_processing {
            // Like Esc: the recording ends, and a held toggle modifier must not start another.
            for pair in &mut self.pairs {
                pair.hold_active = false;
                pair.tap_spoiled = true;
                pair.tap_armed = false;
            }
        }
        (hit.then_some(Output::Event(if self.cancel_processing { CancelProcessing } else { Cancel })), hit)
    }

    pub fn set_bindings(&mut self, hold: Option<Binding>, toggle: Option<Binding>) {
        self.pairs[0].set(hold, toggle);
    }

    pub fn set_command_bindings(&mut self, hold: Option<Binding>, toggle: Option<Binding>) {
        self.pairs[1].set(hold, toggle);
    }

    pub fn set_capture(&mut self, on: bool) {
        self.capturing = on;
        self.capture_max = 0;
        self.pairs.iter_mut().for_each(Pair::reset);
    }

    /// Ends every active hold (their "aborted" events), e.g. before bindings change or capture
    /// starts, so a recording can't stay stuck waiting for a release that will never come.
    pub fn abort_active(&mut self) -> Vec<Output> {
        let mut out = Vec::new();
        for (pair, events) in self.pairs.iter_mut().zip(PAIR_EVENTS) {
            if pair.hold_active {
                pair.hold_active = false;
                out.push(Output::Event(events[2]));
            }
        }
        out
    }

    /// Modifiers that belong to an active hold (allowed alongside Esc, e.g. Fn-hold + Esc).
    fn hold_mods(&self) -> u16 {
        self.pairs
            .iter()
            .filter(|p| p.hold_active)
            .filter_map(|p| p.hold.as_ref())
            .fold(0, |m, b| m | b.exact | b.any)
    }

    /// Aborts active modifier-only holds and spoils taps (another key or a click joined).
    /// A click leaves a Fn-only hold alone: fn-click means nothing, ⌥-click and ⌘-click do.
    fn interrupt(&mut self, except: Option<usize>, click: bool, out: &mut Vec<Output>) {
        for (i, (pair, events)) in self.pairs.iter_mut().zip(PAIR_EVENTS).enumerate() {
            pair.tap_spoiled = true;
            pair.tap_armed = false;
            let aborts = pair.hold.as_ref().is_some_and(|h| h.key.is_none() && !(click && h.exact | h.any == FN));
            if Some(i) != except && pair.hold_active && aborts {
                pair.hold_active = false;
                out.push(Output::Event(events[2]));
            }
        }
    }

    /// Drops keys that are no longer physically down (a keyup we never saw).
    pub fn retain_keys(&mut self, still_down: impl Fn(&'static str) -> bool) {
        self.keys_down.retain(|k| still_down(k));
    }

    /// Returns the outputs and whether the OS event should be swallowed (never for modifiers).
    pub fn handle(&mut self, input: Input) -> (Vec<Output>, bool) {
        let mut out = Vec::new();
        let consume = match input {
            Input::Modifier(bit, down) => {
                self.modifier(bit, down, &mut out);
                false
            }
            Input::KeyDown(code, repeat) => self.key_down(code, repeat, &mut out),
            Input::KeyUp(code) => self.key_up(code, &mut out),
            Input::Pointer => {
                if !self.capturing {
                    self.interrupt(None, true, &mut out);
                }
                false
            }
        };
        (out, consume)
    }

    fn modifier(&mut self, bit: u16, down: bool, out: &mut Vec<Output>) {
        let before = self.mods;
        self.mods = if down { before | bit } else { before & !bit };
        if self.mods == before {
            return;
        }
        if self.capturing {
            if down {
                self.capture_max |= bit;
                out.push(Output::Capture { keys: canonical(self.capture_max, None), done: false });
            } else if self.mods == 0 && self.capture_max != 0 {
                out.push(Output::Capture { keys: canonical(self.capture_max, None), done: true });
                self.capture_max = 0;
            }
            return;
        }
        let (mods, keys_idle) = (self.mods, self.keys_down.is_empty());
        for (pair, [pressed, released, aborted, toggled]) in self.pairs.iter_mut().zip(PAIR_EVENTS) {
            if let Some(hold) = pair.hold.as_ref().filter(|h| h.key.is_none()) {
                if pair.hold_active && !hold.matches(mods) {
                    pair.hold_active = false;
                    out.push(Output::Event(if down { aborted } else { released }));
                } else if !pair.hold_active && down && keys_idle && hold.matches(mods) {
                    pair.hold_active = true;
                    out.push(Output::Event(pressed));
                }
            }
            if let Some(toggle) = pair.toggle.as_ref().filter(|t| t.key.is_none()) {
                if down {
                    if before == 0 {
                        pair.tap_spoiled = !keys_idle;
                    }
                    if mods & !(toggle.exact | toggle.any) != 0 {
                        pair.tap_spoiled = true;
                    }
                    pair.tap_armed = !pair.tap_spoiled && toggle.matches(mods);
                } else {
                    if pair.tap_armed {
                        out.push(Output::Event(toggled));
                    }
                    pair.tap_armed = false;
                }
            }
        }
    }

    fn key_down(&mut self, code: &'static str, repeat: bool, out: &mut Vec<Output>) -> bool {
        if repeat {
            return self.consumed.contains(&code);
        }
        self.consumed.retain(|k| *k != code);
        if !self.keys_down.contains(&code) {
            self.keys_down.push(code);
        }
        if self.capturing {
            let keys = if code == "Escape" && self.mods == 0 { String::new() } else { canonical(self.mods, Some(code)) };
            out.push(Output::Capture { keys, done: true });
            self.capture_max = 0;
            // Never swallow while capturing: the recorder's own window gets the key (and
            // preventDefaults it), and a capture left on by mistake can't eat typing elsewhere.
            return false;
        }
        let mods = self.mods;
        let hits = |b: &Option<Binding>| b.as_ref().is_some_and(|b| b.key == Some(code) && b.matches(mods));
        // ⌘C / Ctrl+C with nothing else held.
        let bare = |group: u16| mods != 0 && mods & !group == 0;
        // Esc/Enter only count with no other modifiers (⇧Enter, ⌥⌘Esc stay the app's), except
        // those of an active hold (Fn-hold + Esc cancels).
        let plain = mods & !self.hold_mods() == 0;
        let event = if let Some(i) = (0..2).find(|&i| hits(&self.pairs[i].toggle)) {
            // The other pair's modifier-only hold (a prefix of this chord) doesn't also fire.
            self.interrupt(Some(i), false, out);
            PAIR_EVENTS[i][3]
        } else if let Some(i) = (0..2).find(|&i| hits(&self.pairs[i].hold)) {
            self.interrupt(Some(i), false, out);
            self.pairs[i].hold_active = true;
            PAIR_EVENTS[i][0]
        } else if code == "Escape" && self.recording && plain {
            self.pairs[0].hold_active = false;
            self.pairs[1].hold_active = false;
            Cancel
        } else if code == "Enter" && self.submit && plain {
            Submit
        } else if code == "Escape" && self.command_esc && mods == 0 {
            CommandEscape
        } else if code == "Enter" && self.command_enter && mods == 0 {
            CommandEnter
        } else if code == "KeyC" && self.command_copy && bare(self.copy_group) {
            CommandCopy
        } else {
            self.interrupt(None, false, out);
            return false;
        };
        for pair in &mut self.pairs {
            pair.tap_spoiled = true;
            pair.tap_armed = false;
        }
        out.push(Output::Event(event));
        self.consume(code)
    }

    fn key_up(&mut self, code: &'static str, out: &mut Vec<Output>) -> bool {
        self.keys_down.retain(|k| *k != code);
        for (pair, events) in self.pairs.iter_mut().zip(PAIR_EVENTS) {
            if pair.hold_active && pair.hold.as_ref().is_some_and(|h| h.key == Some(code)) {
                pair.hold_active = false;
                out.push(Output::Event(events[1]));
            }
        }
        let consumed = self.consumed.contains(&code);
        self.consumed.retain(|k| *k != code);
        consumed
    }

    fn consume(&mut self, code: &'static str) -> bool {
        self.consumed.push(code);
        true
    }
}

fn canonical(mods: u16, key: Option<&'static str>) -> String {
    Binding { exact: mods, any: 0, key }.to_string()
}


#[cfg(test)]
mod tests {
    use super::*;

    fn state(hold: &str, toggle: &str) -> State {
        let mut st = State::default();
        st.set_bindings(parse(hold).unwrap(), parse(toggle).unwrap());
        st
    }

    /// Runs inputs, returns all HotkeyEvents and the consume flag of the last input.
    fn run(st: &mut State, inputs: Vec<Input>) -> (Vec<HotkeyEvent>, bool) {
        let mut events = Vec::new();
        let mut last = false;
        for input in inputs {
            let is_mod = matches!(input, Input::Modifier(..));
            let (out, consume) = st.handle(input);
            assert!(!(is_mod && consume), "modifier events must never be consumed");
            for o in out {
                match o {
                    Output::Event(e) => events.push(e),
                    Output::Capture { .. } => panic!("capture output outside capture mode"),
                }
            }
            last = consume;
        }
        (events, last)
    }

    fn down(bit: u16) -> Input {
        Input::Modifier(bit, true)
    }
    fn up(bit: u16) -> Input {
        Input::Modifier(bit, false)
    }
    fn kd(code: &'static str) -> Input {
        Input::KeyDown(code, false)
    }
    fn ku(code: &'static str) -> Input {
        Input::KeyUp(code)
    }

    fn canon(s: &str) -> String {
        parse(s).unwrap().unwrap().to_string()
    }

    #[test]
    fn canonical_round_trips() {
        assert_eq!(canon("AltLeft+ControlLeft"), "ControlLeft+AltLeft");
        assert_eq!(canon("Space+Fn"), "Fn+Space");
        assert_eq!(canon("KeyA+Meta+Shift"), "Shift+Meta+KeyA");
        assert_eq!(canon("MetaRight+ControlRight+ShiftLeft+AltRight+Fn+F5"), "Fn+ControlRight+AltRight+ShiftLeft+MetaRight+F5");
        assert_eq!(canon("ControlRight+ControlLeft"), "ControlLeft+ControlRight");
        for s in ["Fn", "Fn+Space", "Control+Shift+Space", "Control+Shift+Alt+Space", "ControlLeft+AltLeft", "MetaRight+ShiftRight", "Enter", "Shift+Digit1"] {
            assert_eq!(canon(&canon(s)), canon(s), "{s}");
        }
        assert_eq!(canon("Control+Shift+Alt+Space"), "Control+Alt+Shift+Space");
        assert_eq!(parse("").unwrap(), None);
        for (_, key) in KEYS {
            assert_eq!(canon(&format!("Shift+{key}")), format!("Shift+{key}"));
        }
    }

    #[test]
    fn parse_errors_are_helpful() {
        assert_eq!(parse("Fn+Foo").unwrap_err(), "Unknown key \"Foo\"");
        assert_eq!(parse("Control+space").unwrap_err(), "Unknown key \"space\" (did you mean \"Space\"?)");
        assert_eq!(parse("fn").unwrap_err(), "Unknown key \"fn\" (did you mean \"Fn\"?)");
        assert_eq!(parse("Control+KeyA+KeyB").unwrap_err(), "Only one non-modifier key is allowed (got KeyA and KeyB)");
        assert!(parse("Control+ControlLeft+KeyA").unwrap_err().contains("overlaps"));
        assert!(parse("Fn+Fn").unwrap_err().contains("repeated"));
        assert_eq!(parse("Fn+").unwrap_err(), "\"Fn+\" has an empty key");
        assert!(parse("ControlLeft").is_ok());
    }

    #[test]
    fn overlapping_bindings() {
        let b = |s: &str| parse(s).unwrap().unwrap();
        assert!(overlaps(&b("Alt+Space"), &b("AltRight+Space")));
        assert!(!overlaps(&b("AltLeft+Space"), &b("AltRight+Space")));
        assert!(!overlaps(&b("Fn"), &b("Fn+Space")));
        assert!(overlaps(&b("Fn"), &b("Fn")));
    }

    #[test]
    fn pointer_and_modifier_rules() {
        let mut st = state("Fn", "Fn+Space");
        st.set_command_bindings(parse("AltRight").unwrap(), parse("AltRight+Space").unwrap());
        // ⌥-click aborts the Right-⌥ hold.
        assert_eq!(run(&mut st, vec![down(ALT_RIGHT), Input::Pointer, up(ALT_RIGHT)]).0, vec![CommandHoldPressed, CommandHoldAborted]);
        // A click while holding Fn keeps dictating.
        assert_eq!(run(&mut st, vec![down(FN), Input::Pointer, up(FN)]).0, vec![HoldPressed, HoldReleased]);
        // Armed Enter/Esc ignore modified presses (⇧Enter, ⌥⌘Esc) but Fn-hold + Esc cancels.
        st.command_esc = true;
        st.command_enter = true;
        assert_eq!(run(&mut st, vec![down(SHIFT_LEFT), kd("Enter")]), (vec![], false));
        run(&mut st, vec![ku("Enter"), up(SHIFT_LEFT)]);
        st.recording = true;
        assert_eq!(run(&mut st, vec![down(ALT_LEFT), down(META_LEFT), kd("Escape")]), (vec![], false));
        run(&mut st, vec![ku("Escape"), up(META_LEFT), up(ALT_LEFT)]);
        assert_eq!(run(&mut st, vec![down(FN), kd("Escape")]).0, vec![HoldPressed, Cancel]);
        run(&mut st, vec![ku("Escape"), up(FN)]);
        // A chord of one pair aborts the other pair's modifier-only prefix hold.
        let mut st = state("Fn", "");
        st.set_command_bindings(None, parse("Fn+KeyK").unwrap());
        assert_eq!(run(&mut st, vec![down(FN), kd("KeyK")]).0, vec![HoldPressed, HoldAborted, CommandTogglePressed]);
        // abort_active ends a hold so the recording can't stay stuck.
        let mut st = state("Fn", "");
        run(&mut st, vec![down(FN)]);
        assert_eq!(st.abort_active(), vec![Output::Event(HoldAborted)]);
        assert_eq!(run(&mut st, vec![up(FN)]).0, vec![]);
    }

    #[test]
    fn platform_validation() {
        // macOS: modifier-only and Fn are fine.
        for s in ["", "Fn", "Fn+Space", "AltLeft+ControlLeft", "AltRight", "Control+Shift+Space"] {
            assert!(validate_for(true, s).is_ok(), "{s}");
        }
        assert_eq!(validate_for(true, "Control+Pause").unwrap_err(), "Pause doesn't exist on Mac keyboards");
        // Windows: needs a key, no Fn; sides are accepted (registered as generic).
        for s in ["", "Control+Shift+Space", "Control+Shift+Meta+Space", "ControlLeft+KeyK", "Control+Pause", "F5", "Control+Alt+Meta+KeyC"] {
            assert!(validate_for(false, s).is_ok(), "{s}");
        }
        // Ctrl+Alt without Win is AltGr on many layouts (ć, @, &): refused on Windows only.
        for s in ["Control+Alt+KeyC", "ControlLeft+AltRight+Shift+Space"] {
            assert!(validate_for(false, s).unwrap_err().contains("AltGr"), "{s}");
        }
        assert!(validate_for(true, "Control+Alt+KeyC").is_ok());
        assert_eq!(validate_for(false, "Fn").unwrap_err(), "fn can't be used on Windows");
        assert_eq!(validate_for(false, "Fn+Space").unwrap_err(), "fn can't be used on Windows");
        assert_eq!(validate_for(false, "AltLeft+ControlLeft").unwrap_err(), "Add a key — only modifier keys are supported alone on macOS");
        assert!(validate_for(false, "Control+Nope").is_err());
        // Bare typing keys (or Shift + typing key) would block typing everywhere; F-keys are fine.
        for mac in [true, false] {
            for s in ["Space", "KeyA", "Enter", "Tab", "Digit1", "ShiftLeft+KeyA", "Shift+Space", "ArrowUp"] {
                assert!(validate_for(mac, s).unwrap_err().starts_with("Add a modifier key"), "{s}");
            }
            assert!(validate_for(mac, "F5").is_ok());
            assert!(validate_for(mac, "Shift+F5").is_ok());
        }
        assert!(validate_for(true, "Fn+Space").is_ok());
    }

    #[test]
    fn side_agnostic_and_exact_matching() {
        let generic = parse("Control+Shift+Space").unwrap().unwrap();
        assert!(generic.matches(CONTROL_LEFT | SHIFT_RIGHT));
        assert!(generic.matches(CONTROL_RIGHT | CONTROL_LEFT | SHIFT_LEFT));
        assert!(!generic.matches(CONTROL_LEFT));
        assert!(!generic.matches(CONTROL_LEFT | SHIFT_LEFT | ALT_LEFT), "extra modifier");
        assert!(!generic.matches(CONTROL_LEFT | SHIFT_LEFT | FN), "extra fn");
        let sided = parse("ControlLeft+AltLeft").unwrap().unwrap();
        assert!(sided.matches(CONTROL_LEFT | ALT_LEFT));
        assert!(!sided.matches(CONTROL_RIGHT | ALT_LEFT));
        assert!(!sided.matches(CONTROL_LEFT | CONTROL_RIGHT | ALT_LEFT));
    }

    #[test]
    fn labels() {
        let b = |s| parse(s).unwrap().unwrap();
        assert_eq!(label(&b("ShiftLeft+Meta+Space"), true), "⇧⌘Space");
        assert_eq!(label(&b("Control+Shift+KeyK"), false), "Ctrl+Shift+K");
        assert_eq!(label(&b("Meta+Digit1"), false), "Win+1");
    }

    #[test]
    fn modifier_only_hold() {
        let mut st = state("AltLeft+ControlLeft", "");
        assert_eq!(run(&mut st, vec![down(ALT_LEFT)]).0, vec![]);
        assert_eq!(run(&mut st, vec![down(CONTROL_LEFT)]).0, vec![HoldPressed]);
        assert_eq!(run(&mut st, vec![up(ALT_LEFT), up(CONTROL_LEFT)]).0, vec![HoldReleased]);

        // A non-modifier key while held aborts (and is not swallowed).
        assert_eq!(run(&mut st, vec![down(ALT_LEFT), down(CONTROL_LEFT), kd("KeyA")]), (vec![HoldPressed, HoldAborted], false));
        assert_eq!(run(&mut st, vec![ku("KeyA"), up(ALT_LEFT), up(CONTROL_LEFT)]), (vec![], false));

        // An extra modifier aborts; wrong side never matches.
        assert_eq!(run(&mut st, vec![down(ALT_LEFT), down(CONTROL_LEFT), down(SHIFT_LEFT)]).0, vec![HoldPressed, HoldAborted]);
        assert_eq!(run(&mut st, vec![up(SHIFT_LEFT), up(ALT_LEFT), up(CONTROL_LEFT)]).0, vec![]);
        assert_eq!(run(&mut st, vec![down(ALT_LEFT), down(CONTROL_RIGHT), up(ALT_LEFT), up(CONTROL_RIGHT)]).0, vec![]);

        // Only a press completes it: releasing down into the exact set doesn't, nor does a held key.
        assert_eq!(run(&mut st, vec![down(ALT_LEFT), down(CONTROL_LEFT), down(SHIFT_LEFT)]).0, vec![HoldPressed, HoldAborted]);
        assert_eq!(run(&mut st, vec![up(SHIFT_LEFT)]).0, vec![]);
        assert_eq!(run(&mut st, vec![up(ALT_LEFT), up(CONTROL_LEFT)]).0, vec![]);
        assert_eq!(run(&mut st, vec![kd("KeyA"), down(ALT_LEFT), down(CONTROL_LEFT)]).0, vec![]);
        assert_eq!(run(&mut st, vec![ku("KeyA"), up(ALT_LEFT), up(CONTROL_LEFT)]).0, vec![]);

        // A missed keyup (tap disabled etc.) is pruned by the backend so hold works again.
        run(&mut st, vec![kd("KeyB")]);
        st.retain_keys(|_| false);
        assert_eq!(run(&mut st, vec![down(ALT_LEFT), down(CONTROL_LEFT)]).0, vec![HoldPressed]);
    }

    #[test]
    fn fn_hold_prefix_of_fn_space_toggle() {
        let mut st = state("Fn", "Fn+Space");
        assert_eq!(run(&mut st, vec![down(FN)]).0, vec![HoldPressed]);
        assert_eq!(run(&mut st, vec![kd("Space")]), (vec![TogglePressed], true));
        assert_eq!(run(&mut st, vec![Input::KeyDown("Space", true)]), (vec![], true), "autorepeat swallowed, no event");
        assert_eq!(run(&mut st, vec![ku("Space")]), (vec![], true));
        assert_eq!(run(&mut st, vec![up(FN)]).0, vec![HoldReleased]);
        // Plain Space still types.
        assert_eq!(run(&mut st, vec![kd("Space")]), (vec![], false));
        assert_eq!(run(&mut st, vec![ku("Space")]), (vec![], false));
        // Fn + another key (Fn+Up = PageUp) aborts the hold and passes the key through.
        assert_eq!(run(&mut st, vec![down(FN), kd("PageUp")]), (vec![HoldPressed, HoldAborted], false));
        assert_eq!(run(&mut st, vec![ku("PageUp"), up(FN)]), (vec![], false));
    }

    #[test]
    fn modifier_only_toggle_is_a_tap() {
        let mut st = state("", "MetaRight+ShiftRight");
        assert_eq!(run(&mut st, vec![down(META_RIGHT), down(SHIFT_RIGHT), up(SHIFT_RIGHT), up(META_RIGHT)]).0, vec![TogglePressed]);
        // Nothing fires on press, only on release.
        assert_eq!(run(&mut st, vec![down(META_RIGHT), down(SHIFT_RIGHT)]).0, vec![]);
        assert_eq!(run(&mut st, vec![up(META_RIGHT), up(SHIFT_RIGHT)]).0, vec![TogglePressed]);
        // ⌘⇧K using the same modifiers: no toggle, K not swallowed.
        assert_eq!(run(&mut st, vec![down(META_RIGHT), down(SHIFT_RIGHT), kd("KeyK")]), (vec![], false));
        assert_eq!(run(&mut st, vec![ku("KeyK"), up(SHIFT_RIGHT), up(META_RIGHT)]).0, vec![]);
        // Extra modifier involved: no toggle.
        assert_eq!(run(&mut st, vec![down(META_RIGHT), down(SHIFT_RIGHT), down(ALT_LEFT), up(ALT_LEFT), up(SHIFT_RIGHT), up(META_RIGHT)]).0, vec![]);
        // Key held from before: no toggle. Typing earlier doesn't spoil a later tap.
        assert_eq!(run(&mut st, vec![kd("KeyA"), down(META_RIGHT), down(SHIFT_RIGHT), up(SHIFT_RIGHT), up(META_RIGHT), ku("KeyA")]).0, vec![]);
        assert_eq!(run(&mut st, vec![down(META_RIGHT), down(SHIFT_RIGHT), up(META_RIGHT), up(SHIFT_RIGHT)]).0, vec![TogglePressed]);
        // Generic single-modifier toggle accepts either side.
        let mut st = state("", "Alt");
        assert_eq!(run(&mut st, vec![down(ALT_RIGHT), up(ALT_RIGHT), down(ALT_LEFT), up(ALT_LEFT)]).0, vec![TogglePressed, TogglePressed]);
    }

    #[test]
    fn chord_bindings() {
        let mut st = state("Control+Shift+Space", "MetaRight+ShiftRight+KeyK");
        assert_eq!(run(&mut st, vec![down(CONTROL_LEFT), down(SHIFT_RIGHT), kd("Space")]), (vec![HoldPressed], true));
        assert_eq!(run(&mut st, vec![Input::KeyDown("Space", true)]), (vec![], true));
        assert_eq!(run(&mut st, vec![ku("Space")]), (vec![HoldReleased], true));
        assert_eq!(run(&mut st, vec![up(CONTROL_LEFT), up(SHIFT_RIGHT)]).0, vec![]);
        // Extra modifier: no match, key passes through.
        assert_eq!(run(&mut st, vec![down(CONTROL_LEFT), down(SHIFT_LEFT), down(ALT_LEFT), kd("Space")]), (vec![], false));
        assert_eq!(run(&mut st, vec![ku("Space"), up(ALT_LEFT), up(SHIFT_LEFT), up(CONTROL_LEFT)]), (vec![], false));
        // Toggle chord fires once per press, sides matter.
        assert_eq!(run(&mut st, vec![down(META_RIGHT), down(SHIFT_RIGHT), kd("KeyK")]), (vec![TogglePressed], true));
        assert_eq!(run(&mut st, vec![Input::KeyDown("KeyK", true), ku("KeyK")]), (vec![], true));
        assert_eq!(run(&mut st, vec![kd("KeyK")]), (vec![TogglePressed], true));
        assert_eq!(run(&mut st, vec![ku("KeyK"), up(SHIFT_RIGHT), up(META_RIGHT)]).0, vec![]);
        assert_eq!(run(&mut st, vec![down(META_LEFT), down(SHIFT_RIGHT), kd("KeyK")]), (vec![], false));
    }

    #[test]
    fn escape_and_enter_only_when_armed() {
        let mut st = state("Fn", "Fn+Space");
        assert_eq!(run(&mut st, vec![kd("Escape")]), (vec![], false));
        assert_eq!(run(&mut st, vec![ku("Escape")]), (vec![], false));
        assert_eq!(run(&mut st, vec![kd("Enter"), ku("Enter")]), (vec![], false));
        st.recording = true;
        assert_eq!(run(&mut st, vec![kd("Escape")]), (vec![Cancel], true));
        assert_eq!(run(&mut st, vec![ku("Escape")]), (vec![], true));
        assert_eq!(run(&mut st, vec![kd("Enter")]), (vec![], false), "Enter needs submit armed");
        run(&mut st, vec![ku("Enter")]);
        st.submit = true;
        assert_eq!(run(&mut st, vec![kd("Enter")]), (vec![Submit], true));
        assert_eq!(run(&mut st, vec![ku("Enter")]), (vec![], true));
        // Esc during a modifier hold cancels without a later HoldReleased.
        assert_eq!(run(&mut st, vec![down(FN), kd("Escape"), ku("Escape"), up(FN)]).0, vec![HoldPressed, Cancel]);
    }

    #[test]
    fn command_pair_mirrors_dictation() {
        let mut st = state("Fn", "Fn+Space");
        st.set_command_bindings(parse("AltRight").unwrap(), parse("AltRight+Space").unwrap());
        // Right ⌥ hold / release, and the prefix → hands-free upgrade.
        assert_eq!(run(&mut st, vec![down(ALT_RIGHT), up(ALT_RIGHT)]).0, vec![CommandHoldPressed, CommandHoldReleased]);
        assert_eq!(
            run(&mut st, vec![down(ALT_RIGHT), kd("Space")]),
            (vec![CommandHoldPressed, CommandTogglePressed], true)
        );
        assert_eq!(run(&mut st, vec![ku("Space"), up(ALT_RIGHT)]), (vec![CommandHoldReleased], false));
        // Typing ⌥-characters with Right ⌥ aborts the hold and passes the key through.
        assert_eq!(run(&mut st, vec![down(ALT_RIGHT), kd("KeyE")]), (vec![CommandHoldPressed, CommandHoldAborted], false));
        run(&mut st, vec![ku("KeyE"), up(ALT_RIGHT)]);
        // Left ⌥ is a different key: nothing.
        assert_eq!(run(&mut st, vec![down(ALT_LEFT), up(ALT_LEFT)]).0, vec![]);
        // Dictation still works alongside.
        assert_eq!(run(&mut st, vec![down(FN), up(FN)]).0, vec![HoldPressed, HoldReleased]);
    }

    #[test]
    fn command_window_keys_only_while_armed() {
        let mut st = state("Fn", "Fn+Space");
        st.copy_group = GROUPS[3];
        assert_eq!(run(&mut st, vec![kd("Enter")]), (vec![], false));
        run(&mut st, vec![ku("Enter")]);
        st.command_esc = true;
        // Enter only while there is something to paste or approve.
        assert_eq!(run(&mut st, vec![kd("Enter")]), (vec![], false));
        run(&mut st, vec![ku("Enter")]);
        st.command_enter = true;
        assert_eq!(run(&mut st, vec![kd("Enter")]), (vec![CommandEnter], true));
        assert_eq!(run(&mut st, vec![ku("Enter")]), (vec![], true));
        assert_eq!(run(&mut st, vec![kd("Escape")]), (vec![CommandEscape], true));
        run(&mut st, vec![ku("Escape")]);
        // ⌘C only once an answer is ready, and only as plain ⌘C.
        assert_eq!(run(&mut st, vec![down(META_LEFT), kd("KeyC")]), (vec![], false));
        run(&mut st, vec![ku("KeyC"), up(META_LEFT)]);
        st.command_copy = true;
        assert_eq!(run(&mut st, vec![down(META_LEFT), kd("KeyC")]), (vec![CommandCopy], true));
        run(&mut st, vec![ku("KeyC"), up(META_LEFT)]);
        assert_eq!(run(&mut st, vec![down(META_LEFT), down(SHIFT_LEFT), kd("KeyC")]), (vec![], false));
        run(&mut st, vec![ku("KeyC"), up(SHIFT_LEFT), up(META_LEFT)]);
        // Dictation Esc/Enter arming wins while recording.
        st.recording = true;
        st.submit = true;
        assert_eq!(run(&mut st, vec![kd("Escape")]).0, vec![Cancel]);
        run(&mut st, vec![ku("Escape")]);
        assert_eq!(run(&mut st, vec![kd("Enter")]).0, vec![Submit]);
    }

    fn capture_run(st: &mut State, inputs: Vec<Input>) -> (Vec<(String, bool)>, bool) {
        let mut caps = Vec::new();
        let mut last = false;
        for input in inputs {
            let is_mod = matches!(input, Input::Modifier(..));
            let (out, consume) = st.handle(input);
            assert!(!(is_mod && consume));
            for o in out {
                match o {
                    Output::Capture { keys, done } => caps.push((keys, done)),
                    Output::Event(e) => panic!("{e:?} while capturing"),
                }
            }
            last = consume;
        }
        (caps, last)
    }

    #[test]
    fn capture_mode() {
        let mut st = state("Fn", "Fn+Space");
        st.recording = true;
        st.set_capture(true);
        let c = |k: &str, d| (k.to_string(), d);
        // Modifier-only: progress on each press, done with the maximal set once all are released.
        assert_eq!(
            capture_run(&mut st, vec![down(CONTROL_LEFT), down(ALT_LEFT), up(ALT_LEFT), up(CONTROL_LEFT)]).0,
            vec![c("ControlLeft", false), c("ControlLeft+AltLeft", false), c("ControlLeft+AltLeft", true)]
        );
        assert_eq!(capture_run(&mut st, vec![down(FN), up(FN)]).0, vec![c("Fn", false), c("Fn", true)]);
        // Chord: done on the key, key passed through (down, repeat, up), no events even though Fn+Space is bound.
        assert_eq!(capture_run(&mut st, vec![down(FN), kd("Space")]), (vec![c("Fn", false), c("Fn+Space", true)], false));
        assert_eq!(capture_run(&mut st, vec![Input::KeyDown("Space", true)]), (vec![], false));
        assert_eq!(capture_run(&mut st, vec![ku("Space")]), (vec![], false));
        assert_eq!(capture_run(&mut st, vec![up(FN)]), (vec![], false), "no second done after a chord");
        assert_eq!(capture_run(&mut st, vec![down(META_RIGHT), down(SHIFT_RIGHT), kd("KeyK")]).0.last(), Some(&c("ShiftRight+MetaRight+KeyK", true)));
        capture_run(&mut st, vec![ku("KeyK"), up(SHIFT_RIGHT), up(META_RIGHT)]);
        // Bare key, and Esc alone = cancel (""), Esc with a modifier is a real chord.
        assert_eq!(capture_run(&mut st, vec![kd("F5")]), (vec![c("F5", true)], false));
        capture_run(&mut st, vec![ku("F5")]);
        assert_eq!(capture_run(&mut st, vec![kd("Escape")]), (vec![c("", true)], false));
        assert_eq!(capture_run(&mut st, vec![ku("Escape")]), (vec![], false));
        assert_eq!(capture_run(&mut st, vec![down(SHIFT_LEFT), kd("Escape")]).0.last(), Some(&c("ShiftLeft+Escape", true)));
        capture_run(&mut st, vec![ku("Escape"), up(SHIFT_LEFT)]);
        // Back to normal afterwards.
        st.set_capture(false);
        assert_eq!(run(&mut st, vec![down(FN), up(FN)]).0, vec![HoldPressed, HoldReleased]);
    }

    #[test]
    fn bar_cancel_click_cancels_and_swallows_only_its_own_click() {
        let mut st = State::default();
        let x = [1000.0, 800.0, 30.0, 38.0];
        // Nothing armed: every click passes through.
        assert_eq!(st.click(true, 1010.0, 820.0), (None, false));
        assert_eq!(st.click(false, 1010.0, 820.0), (None, false));
        st.cancel_area = Some(x);
        assert_eq!(st.click(true, 1010.0, 820.0), (Some(Output::Event(HotkeyEvent::Cancel)), true), "processing is cancellable without a recording");
        assert_eq!(st.click(false, 1010.0, 820.0), (None, true));
        st.recording = true;
        assert_eq!(st.click(true, 999.0, 820.0), (None, false), "left of the x");
        assert_eq!(st.click(true, 1010.0, 838.0), (None, false), "below the x");
        assert_eq!(st.click(false, 1010.0, 820.0), (None, false), "its button-up passes too");
        assert_eq!(st.click(true, 1010.0, 820.0), (Some(Output::Event(Cancel)), true));
        // The button-up is swallowed wherever the pointer went, once.
        assert_eq!(st.click(false, 0.0, 0.0), (None, true));
        assert_eq!(st.click(false, 1010.0, 820.0), (None, false));
        st.cancel_area = None;
        assert_eq!(st.click(true, 1010.0, 820.0), (None, false), "x hidden");
    }

    #[test]
    fn bar_cancel_click_spoils_a_held_toggle_tap_like_esc() {
        let mut st = state("Fn", "AltRight");
        st.recording = true;
        st.cancel_area = Some([0.0, 0.0, 10.0, 10.0]);
        run(&mut st, vec![down(ALT_RIGHT)]);
        assert_eq!(st.click(true, 5.0, 5.0).0, Some(Output::Event(Cancel)));
        st.click(false, 5.0, 5.0);
        st.recording = false;
        assert_eq!(run(&mut st, vec![up(ALT_RIGHT)]).0, vec![], "no new hands-free recording");
    }
    #[test]
    fn cancelling_processing_never_spoils_a_new_recording_hold() {
        let mut st = state("Fn", "Fn+Space");
        st.cancel_area = Some([0.0, 0.0, 10.0, 10.0]);
        st.cancel_processing = true;
        assert_eq!(run(&mut st, vec![down(FN)]).0, vec![HoldPressed]);
        assert_eq!(st.click(true, 5.0, 5.0), (Some(Output::Event(CancelProcessing)), true));
        assert_eq!(st.click(false, 5.0, 5.0), (None, true));
        assert_eq!(run(&mut st, vec![up(FN)]).0, vec![HoldReleased]);
    }

}
