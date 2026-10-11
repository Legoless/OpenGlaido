//! Conservative, local observations of edits to a just-pasted dictation. Full textbox
//! snapshots stay on the watcher thread; only an attributed span can reach History.

use crate::frontmost::{self, FocusTarget, TextSelection};
use crate::AppState;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

const MAX_TEXT_UNITS: usize = 16_384;
const INSERTION_WINDOW: Duration = Duration::from_millis(700);
const WATCH_WINDOW: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_millis(300);
const STABLE_INTERVAL: Duration = Duration::from_millis(600);

#[derive(Default)]
pub struct EditTrackingState {
    generation: AtomicU64,
}

impl EditTrackingState {
    /// Invalidate a prepared insertion or active watcher without retaining a native handle.
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
}

/// Sendable process/window/control identity plus a RAM-only pre-insertion snapshot.
pub struct PreparedEdit {
    history_id: String,
    epoch: u64,
    generation: u64,
    focus: FocusTarget,
    app_bundle_id: String,
    tracker: SpanTracker,
}

/// Capture immediately before injection. Capability, consent and attribution failures
/// leave normal paste behavior unchanged.
pub fn prepare(
    app: &AppHandle,
    history_id: &str,
    expected_epoch: u64,
    expected_generation: u64,
    pasted: &str,
) -> Option<PreparedEdit> {
    let state = app.state::<AppState>();
    // A delayed delivery keeps the token captured when its recording started. Never
    // adopt a newer token here: a subsequent recording or opt-out has superseded it.
    if !cfg!(target_os = "macos") || !allowed(&state, expected_epoch, expected_generation) {
        return None;
    }
    // Never hold configuration or database locks during a native read.
    let before = guarded_read(allowed(&state, expected_epoch, expected_generation), || {
        frontmost::capture_textbox(None)
    })?;
    let tracker = SpanTracker::new(&before.text, before.selection, pasted)?;
    if !allowed(&state, expected_epoch, expected_generation) {
        return None;
    }
    Some(PreparedEdit {
        history_id: history_id.into(),
        epoch: expected_epoch,
        generation: expected_generation,
        focus: before.focus,
        app_bundle_id: before.app_bundle_id,
        tracker,
    })
}

/// Only call after successful key injection. This thread never takes recording jobs,
/// changes the HUD, reads the clipboard, or performs cloud analysis.
pub fn start(app: AppHandle, prepared: PreparedEdit) {
    if !allowed(
        &app.state::<AppState>(),
        prepared.epoch,
        prepared.generation,
    ) {
        return;
    }
    let _ = thread::Builder::new()
        .name("textbox-edit-watch".into())
        .spawn(move || watch(app, prepared));
}

fn allowed(state: &AppState, epoch: u64, generation: u64) -> bool {
    if state.edit_tracking.generation() != generation {
        return false;
    }
    let enabled = {
        let config = state.config.lock().unwrap();
        consented(
            config.beta_features,
            config.workflow_insights.enabled,
            config.workflow_insights.track_edits,
        )
    };
    enabled
        && current_writer(
            state.edit_tracking.generation(),
            generation,
            state.db.workflow_epoch().ok(),
            epoch,
        )
}

fn consented(beta: bool, insights: bool, track_edits: bool) -> bool {
    cfg!(target_os = "macos") && beta && insights && track_edits
}

fn current_writer(
    generation: u64,
    expected_generation: u64,
    epoch: Option<u64>,
    expected_epoch: u64,
) -> bool {
    generation == expected_generation && epoch == Some(expected_epoch)
}

/// All native reads pass this gate, allowing consent-off behavior to be verified
/// without inspecting a real user's field.
fn guarded_read<T>(permission: bool, read: impl FnOnce() -> Option<T>) -> Option<T> {
    if permission {
        read()
    } else {
        None
    }
}

fn watch(app: AppHandle, mut prepared: PreparedEdit) {
    let started = Instant::now();
    let state = app.state::<AppState>();
    // The field must show the exact selected replacement. If the application rewrites
    // the paste, switches focus, or does not expose it in time, attribution is unknown.
    loop {
        if !allowed(&state, prepared.epoch, prepared.generation)
            || started.elapsed() >= INSERTION_WINDOW
        {
            return;
        }
        let Some(snapshot) =
            guarded_read(allowed(&state, prepared.epoch, prepared.generation), || {
                frontmost::capture_textbox(Some(&prepared.focus))
            })
        else {
            return;
        };
        if !snapshot.focus.same_focus(&prepared.focus) || started.elapsed() >= INSERTION_WINDOW {
            return;
        }
        if prepared.tracker.verify_insertion(&snapshot.text) {
            break;
        }
        if snapshot.text != prepared.tracker.before {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }

    let mut changed_at: Option<Instant> = None;
    let mut saved: Option<String> = None;
    while started.elapsed() < WATCH_WINDOW {
        thread::sleep(POLL_INTERVAL);
        if started.elapsed() >= WATCH_WINDOW
            || !allowed(&state, prepared.epoch, prepared.generation)
        {
            return;
        }
        let Some(snapshot) =
            guarded_read(allowed(&state, prepared.epoch, prepared.generation), || {
                frontmost::capture_textbox(Some(&prepared.focus))
            })
        else {
            return;
        };
        if !snapshot.focus.same_focus(&prepared.focus) || started.elapsed() >= WATCH_WINDOW {
            return;
        }
        match prepared.tracker.observe(&snapshot.text) {
            Observation::Stop => return,
            Observation::Changed => changed_at = Some(Instant::now()),
            Observation::Unchanged => {}
        }
        let stable = changed_at.is_some_and(|at| at.elapsed() >= STABLE_INTERVAL);
        let edited = &prepared.tracker.current;
        if stable && saved.as_ref() != Some(edited) {
            // Serialize the final consent check with opt-out and Clear. The database
            // also checks its epoch and History ownership, and rejects terminal reviews.
            let config = state.config.lock().unwrap();
            if !config.beta_features
                || !config.workflow_insights.enabled
                || !config.workflow_insights.track_edits
                || state.edit_tracking.generation() != prepared.generation
            {
                return;
            }
            if state
                .db
                .workflow_save_edit_observation(
                    &prepared.history_id,
                    &prepared.tracker.original,
                    edited,
                    Some(&prepared.app_bundle_id),
                    prepared.epoch,
                )
                .is_err()
            {
                return;
            }
            saved = Some(edited.clone());
            let _ = app.emit("workflow-updated", ());
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Observation {
    Unchanged,
    Changed,
    Stop,
}

/// Fixed surrounding anchors disambiguate repeated text without fuzzy searching. A
/// textbox selection is measured in UTF-16 by Accessibility, not Rust byte offsets.
struct SpanTracker {
    before: String,
    before_fingerprint: [u8; 32],
    expected: String,
    prefix: String,
    suffix: String,
    original: String,
    current: String,
    inserted: bool,
}

impl SpanTracker {
    fn new(before: &str, selection: TextSelection, pasted: &str) -> Option<Self> {
        if pasted.trim().is_empty()
            || before.encode_utf16().count() > MAX_TEXT_UNITS
            || pasted.encode_utf16().count() > MAX_TEXT_UNITS
        {
            return None;
        }
        let start = utf16_boundary(before, selection.location)?;
        let end = utf16_boundary(before, selection.location.checked_add(selection.length)?)?;
        let prefix = before.get(..start)?.to_owned();
        let suffix = before.get(end..)?.to_owned();
        let expected = format!("{prefix}{pasted}{suffix}");
        if expected == before || expected.encode_utf16().count() > MAX_TEXT_UNITS {
            return None;
        }
        Some(Self {
            before: before.into(),
            before_fingerprint: Sha256::digest(before.as_bytes()).into(),
            expected,
            prefix,
            suffix,
            original: pasted.into(),
            current: pasted.into(),
            inserted: false,
        })
    }

    fn verify_insertion(&mut self, text: &str) -> bool {
        if text != self.expected {
            return false;
        }
        self.inserted = true;
        // No surrounding field needs to survive the exact insertion check twice.
        self.before.clear();
        self.expected.clear();
        true
    }

    fn observe(&mut self, text: &str) -> Observation {
        if !self.inserted
            || text.encode_utf16().count() > MAX_TEXT_UNITS
            || <[u8; 32]>::from(Sha256::digest(text.as_bytes())) == self.before_fingerprint
            || !text.starts_with(&self.prefix)
            || !text.ends_with(&self.suffix)
        {
            return Observation::Stop;
        }
        let Some(end) = text.len().checked_sub(self.suffix.len()) else {
            return Observation::Stop;
        };
        let Some(edited) = text.get(self.prefix.len()..end) else {
            return Observation::Stop;
        };
        if edited == self.current {
            return Observation::Unchanged;
        }
        if edited.trim().is_empty() {
            // Clearing/submitting a composer is not evidence of the desired output.
            return Observation::Stop;
        }
        if edited == self.original {
            // An exact undo is known, including restoring a deleted end of the span.
            // Saving the original again clears an existing pending observation.
            self.current = edited.into();
            return Observation::Changed;
        }
        if edited.contains(&self.current)
            || (edited != self.original && edited.contains(&self.original))
        {
            // The intact pasted text with extra text around it is a continuation,
            // even when the dictation originally filled an otherwise empty field.
            return Observation::Stop;
        }
        let (start, old_end, new_end) = change_bounds(&self.current, edited);
        // Pure boundary typing could be an unrelated next message or sentence.
        if start == old_end && (start == 0 || start == self.current.len()) {
            return Observation::Stop;
        }
        let owns_field = self.prefix.is_empty() && self.suffix.is_empty();
        if !owns_field && (start == 0 || old_end == self.current.len()) {
            return Observation::Stop;
        }
        let entire_rewrite =
            owns_field && start == 0 && old_end == self.current.len() && new_end == edited.len();
        if entire_rewrite
            || has_disjoint_word_edits(&self.current[start..old_end], &edited[start..new_end])
        {
            return Observation::Stop;
        }
        self.current = edited.into();
        Observation::Changed
    }
}

fn utf16_boundary(text: &str, requested: usize) -> Option<usize> {
    let mut units = 0;
    for (offset, ch) in text.char_indices() {
        if units == requested {
            return Some(offset);
        }
        units += ch.len_utf16();
        if units > requested {
            return None; // A selection splitting a surrogate pair is invalid.
        }
    }
    (units == requested).then_some(text.len())
}

/// Return UTF-8 boundaries of the smallest changed range in both strings.
fn change_bounds(old: &str, new: &str) -> (usize, usize, usize) {
    let mut start = 0;
    for (a, b) in old.chars().zip(new.chars()) {
        if a != b {
            break;
        }
        start += a.len_utf8();
    }
    let mut old_end = old.len();
    let mut new_end = new.len();
    for (a, b) in old[start..].chars().rev().zip(new[start..].chars().rev()) {
        if a != b {
            break;
        }
        old_end -= a.len_utf8();
        new_end -= b.len_utf8();
    }
    (start, old_end, new_end)
}

/// A bounded edit alignment catches separate changes around untouched whitespace.
/// Within-word spelling changes can contain unchanged letters; edits in distinct
/// words during one polling interval have insufficient attribution and stop tracking.
fn has_disjoint_word_edits(old: &str, new: &str) -> bool {
    let a: Vec<char> = old.chars().collect();
    let b: Vec<char> = new.chars().collect();
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a.len() > 512 || b.len() > 512 {
        return true;
    }
    let columns = b.len() + 1;
    let mut distance = vec![0usize; (a.len() + 1) * columns];
    for i in 0..=a.len() {
        distance[i * columns] = i;
    }
    for (j, cell) in distance[..columns].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            distance[i * columns + j] = (distance[(i - 1) * columns + j] + 1)
                .min(distance[i * columns + j - 1] + 1)
                .min(distance[(i - 1) * columns + j - 1] + usize::from(a[i - 1] != b[j - 1]));
        }
    }
    let mut i = a.len();
    let mut j = b.len();
    let mut saw_edit = false;
    let mut unchanged_whitespace = false;
    while i > 0 || j > 0 {
        let value = distance[i * columns + j];
        if i > 0 && j > 0 && a[i - 1] == b[j - 1] && value == distance[(i - 1) * columns + j - 1] {
            unchanged_whitespace |= saw_edit && a[i - 1].is_whitespace();
            i -= 1;
            j -= 1;
        } else {
            if saw_edit && unchanged_whitespace {
                return true;
            }
            saw_edit = true;
            if i > 0 && j > 0 && value == distance[(i - 1) * columns + j - 1] + 1 {
                i -= 1;
                j -= 1;
            } else if i > 0 && value == distance[(i - 1) * columns + j] + 1 {
                i -= 1;
            } else {
                j -= 1;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker(before: &str, location: usize, length: usize, pasted: &str) -> SpanTracker {
        let mut tracker =
            SpanTracker::new(before, TextSelection { location, length }, pasted).unwrap();
        let expected = tracker.expected.clone();
        assert!(tracker.verify_insertion(&expected));
        tracker
    }

    #[test]
    fn exact_insertion_uses_utf16_selection_and_rejects_split_surrogates() {
        let mut span = SpanTracker::new(
            "a😀OLDz",
            TextSelection {
                location: 3,
                length: 3,
            },
            "new😀",
        )
        .unwrap();
        assert!(!span.verify_insertion("a😀newz"));
        assert!(span.verify_insertion("a😀new😀z"));
        assert!(SpanTracker::new(
            "a😀z",
            TextSelection {
                location: 2,
                length: 0
            },
            "x"
        )
        .is_none());
        assert!(SpanTracker::new(
            "a😀z",
            TextSelection {
                location: 1,
                length: 1
            },
            "x"
        )
        .is_none());
        assert!(SpanTracker::new(
            "abc",
            TextSelection {
                location: usize::MAX,
                length: 2
            },
            "x"
        )
        .is_none());
    }

    #[test]
    fn anchors_identify_repeated_pasted_text_and_never_search_a_second_match() {
        let mut span = tracker("same / same", 7, 4, "one same word");
        assert_eq!(
            span.observe("same / one correct word"),
            Observation::Changed
        );
        assert_eq!(span.current, "one correct word");
        assert_eq!(
            span.observe("changed / one correct word"),
            Observation::Stop
        );
    }

    #[test]
    fn internal_replacement_deletion_and_insertion_preserve_only_owned_span() {
        let mut span = tracker("before [] after", 8, 0, "the wrong word here");
        assert_eq!(
            span.observe("before [the right word here] after"),
            Observation::Changed
        );
        assert_eq!(
            span.observe("before [the right here] after"),
            Observation::Changed
        );
        assert_eq!(
            span.observe("before [the right new here] after"),
            Observation::Changed
        );
        assert_eq!(span.current, "the right new here");
        assert_eq!(span.original, "the wrong word here");
    }

    #[test]
    fn unicode_word_edits_never_slice_a_codepoint_or_surrogate() {
        let mut span = tracker("", 0, 0, "Use 😀 mikrofon now.");
        assert_eq!(span.observe("Use 🎤 mikrofon now."), Observation::Changed);
        assert_eq!(span.observe("Use 🎤 microphone now."), Observation::Changed);
        assert_eq!(span.current, "Use 🎤 microphone now.");
        assert_eq!(
            span.observe("Use 🎤 microphone now."),
            Observation::Unchanged
        );
    }

    #[test]
    fn boundary_typing_and_outside_edits_stop_instead_of_becoming_references() {
        for edited in [
            "hello world. next",
            "next hello world.",
            "next hello world. next",
            "",
            "   ",
        ] {
            let mut span = tracker("", 0, 0, "hello world.");
            assert_eq!(span.observe(edited), Observation::Stop, "{edited:?}");
        }
        let mut span = tracker("left|right", 5, 0, "hello world.");
        assert_eq!(span.observe("LEFT|hello world.right"), Observation::Stop);
        let mut span = tracker("left|right", 5, 0, "hello world.");
        assert_eq!(span.observe("left|hello world.RIGHT"), Observation::Stop);
        let mut span = tracker("left|right", 5, 0, "hello world.");
        assert_eq!(
            span.observe("left|greetings world.right"),
            Observation::Stop
        );
    }

    #[test]
    fn wholesale_field_rewrites_are_unknown_even_when_originally_owned() {
        let mut owned = tracker("old message", 0, 11, "hello there");
        assert_eq!(owned.observe("completely revised"), Observation::Stop);
        let mut partial = tracker("left|right", 5, 0, "hello there");
        assert_eq!(
            partial.observe("left|completely revisedright"),
            Observation::Stop
        );
    }

    #[test]
    fn disjoint_word_edits_are_unknown_but_a_single_spelling_fix_is_supported() {
        let mut span = tracker("", 0, 0, "a cat and dog here.");
        assert_eq!(span.observe("a bat and fog here."), Observation::Stop);
        let mut span = tracker("", 0, 0, "a transciprtion here.");
        assert_eq!(span.observe("a transcription here."), Observation::Changed);
    }

    #[test]
    fn provisional_insertion_clear_and_cancel_fail_closed() {
        let mut span = SpanTracker::new(
            "",
            TextSelection {
                location: 0,
                length: 0,
            },
            "hello",
        )
        .unwrap();
        assert_eq!(span.observe("hullo"), Observation::Stop);
        assert!(span.verify_insertion("hello"));
        assert_eq!(span.observe(""), Observation::Stop);
        let state = EditTrackingState::default();
        let generation = state.generation();
        state.cancel();
        assert_ne!(state.generation(), generation);
    }

    #[test]
    fn reverted_edits_can_clear_a_pending_observation() {
        let mut span = tracker("", 0, 0, "the wrong word.");
        assert_eq!(span.observe("the right word."), Observation::Changed);
        assert_eq!(span.observe("the wrong word."), Observation::Changed);
        assert_eq!(span.current, span.original);
        let mut span = tracker("", 0, 0, "the wrong word.");
        assert_eq!(span.observe("the wrong word"), Observation::Changed);
        assert_eq!(span.observe("the wrong word."), Observation::Changed);
        assert_eq!(span.current, span.original);
    }

    #[test]
    fn oversized_or_invalid_snapshots_are_never_attributed() {
        let long = "a".repeat(MAX_TEXT_UNITS + 1);
        assert!(SpanTracker::new(
            &long,
            TextSelection {
                location: 0,
                length: 0
            },
            "x"
        )
        .is_none());
        assert!(SpanTracker::new(
            "",
            TextSelection {
                location: 0,
                length: 0
            },
            &long
        )
        .is_none());
        let mut span = tracker("", 0, 0, "short");
        assert_eq!(span.observe(&long), Observation::Stop);
    }

    #[test]
    fn disabled_consent_cannot_invoke_a_native_snapshot_reader() {
        let reads = std::cell::Cell::new(0);
        for (beta, insights, edits) in [
            (false, false, false),
            (false, true, true),
            (true, false, true),
            (true, true, false),
        ] {
            let snapshot = guarded_read(consented(beta, insights, edits), || {
                reads.set(reads.get() + 1);
                Some("private mock text")
            });
            assert!(snapshot.is_none());
        }
        assert_eq!(reads.get(), 0);
    }

    #[test]
    fn cancelled_or_invalidated_watchers_cannot_read_or_resume_after_reenable() {
        let reads = std::cell::Cell::new(0);
        for (generation, expected_generation, epoch, expected_epoch) in [
            (2, 1, Some(4), 4), // Cancelled, including opt-out followed by opt-in.
            (1, 1, Some(5), 4), // Clear or History deletion invalidated writers.
            (1, 1, None, 4),    // Database unavailable: no observation.
        ] {
            let snapshot = guarded_read(
                current_writer(generation, expected_generation, epoch, expected_epoch),
                || {
                    reads.set(reads.get() + 1);
                    Some("private mock text")
                },
            );
            assert!(snapshot.is_none());
        }
        assert_eq!(reads.get(), 0);
        assert!(current_writer(1, 1, Some(4), 4));
    }

    #[test]
    fn delayed_delivery_cannot_adopt_a_new_recordings_generation() {
        let state = EditTrackingState::default();
        state.cancel(); // Recording A begins.
        let recording_a = state.generation();
        state.cancel(); // Recording B begins while A is still processing.
        let recording_b = state.generation();
        let reads = std::cell::Cell::new(0);
        let delayed_a = guarded_read(
            current_writer(state.generation(), recording_a, Some(4), 4),
            || {
                reads.set(reads.get() + 1);
                Some("private mock field")
            },
        );
        assert!(delayed_a.is_none());
        assert_eq!(reads.get(), 0);
        assert!(current_writer(state.generation(), recording_b, Some(4), 4));
    }

    #[test]
    fn opt_out_then_reenable_does_not_rearm_an_inflight_delivery() {
        let state = EditTrackingState::default();
        state.cancel(); // Recording starts with tracking enabled.
        let recording = state.generation();
        state.cancel(); // Tracking is disabled while its transcript is processing.
        state.cancel(); // Tracking is enabled again before delivery.
        let reads = std::cell::Cell::new(0);
        let delayed = guarded_read(
            current_writer(state.generation(), recording, Some(4), 4),
            || {
                reads.set(reads.get() + 1);
                Some("private mock field")
            },
        );
        assert!(delayed.is_none());
        assert_eq!(reads.get(), 0);
    }

    #[test]
    fn cancellation_between_capture_and_start_prevents_a_second_native_read() {
        let state = EditTrackingState::default();
        state.cancel();
        let recording = state.generation();
        let reads = std::cell::Cell::new(0);
        let before = guarded_read(
            current_writer(state.generation(), recording, Some(4), 4),
            || {
                reads.set(reads.get() + 1);
                Some("private mock field")
            },
        );
        assert!(before.is_some());
        state.cancel(); // Native capture finished; a new recording/opt-out now wins.
        let first_watch_read = guarded_read(
            current_writer(state.generation(), recording, Some(4), 4),
            || {
                reads.set(reads.get() + 1);
                Some("private mock field")
            },
        );
        assert!(first_watch_read.is_none());
        assert_eq!(reads.get(), 1);
    }

    #[test]
    fn undo_restoring_preexisting_private_text_is_never_a_correction() {
        let private = "Use private account now.";
        let mut span = tracker(private, 0, private.encode_utf16().count(), "Use MXL now.");
        assert!(span.before.is_empty()); // Only its RAM-only fingerprint survives.
        assert_eq!(span.observe(private), Observation::Stop);
        assert_eq!(span.current, "Use MXL now.");
    }

    #[test]
    fn unchanged_selected_paste_has_no_observable_insertion_proof() {
        assert!(SpanTracker::new(
            "Use MXL now.",
            TextSelection {
                location: 0,
                length: 12
            },
            "Use MXL now.",
        )
        .is_none());
    }

    #[test]
    fn preexisting_text_fingerprint_does_not_block_an_owned_internal_edit() {
        let private = "Use private account now.";
        let mut span = tracker(private, 0, private.encode_utf16().count(), "Use MXL now.");
        assert_eq!(span.observe("Use Samson now."), Observation::Changed);
        assert_eq!(span.current, "Use Samson now.");
    }
}
