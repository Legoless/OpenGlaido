# Workflow Insights (Beta)

## Goal

Help an individual find the most accurate dictation setup for their own apps,
language and vocabulary. Accuracy comes first; speed and cost are secondary.
User-confirmed corrections are evidence. Model agreement and fluent text alone
are not evidence of transcription accuracy.

This feature follows the existing OpenGlaido UI. No separate design mock exists.

## Consent and controls

- Existing installations and new profiles start with Insights disabled. Both
  Beta features and the separate Insights opt-in must be enabled to collect.
- Enabling local Insights never starts additional network requests. It records
  allowlisted settings and measurements alongside existing History audio.
- Cloud analysis is separately disabled by default. Its explanation covers
  sending selected saved audio to the chosen transcription provider and selected
  text/references to configured judges, using the user's existing provider keys.
- Comparisons and judging require explicit actions. No periodic uploads or
  automatic configuration changes. Each action has a fixed request bound.
- Turning either gate off cancels active analysis and stops new measurements.
  Existing evidence remains readable and can be cleared even when disabled.
- Clear removes all Insights evidence. Deleting a History recording removes its
  associated evidence. In-flight tasks must not recreate cleared/deleted data.
  History review retains the original plus the latest eight comparisons and
  the latest three-judge findings per run/reference.
- API keys, full browser URLs, unrelated screen/field contents and provider
  response bodies do not belong in Insights records or diagnostic output.

## Recording evidence

Capture immutable original runs with raw STT, pre-dictionary formatting and
final output; effective formatting/model settings; actual microphone and fallback
when available; audio input/preprocessing identity; and stage timings. Record
capture startup separately from audio duration and processing time. Delivery
measurements describe a request to paste, not verified insertion in another app.
Live backup winner provenance remains explicitly unknown if the current live
API cannot identify it. Silent/empty outcomes contribute local counters without
retaining new audio. Insights failures must never prevent ordinary dictation.

## Corrections

History offers explicit confirmation or correction. A spoken reference answers
"What I said" and is used for normalized word-error measurements. An output
reference answers "What I wanted pasted" and is used for final-output review.
Keep these independent: filler removal, snippets, punctuation and deliberate
rewrites are not automatically STT errors. Unreviewed recordings remain unknown.
Neither correction overwrites the original transcript or delivered text.

## Comparisons and judges

An explicitly requested comparison replays the saved RNNoise WAV using current
settings, one configured backup, or compares local raw output. It creates a
separate run, never pastes and never invokes History Retry. Individual replay
providers must not silently fan out to all live backups. Cloud STT and cloud
formatting require cloud consent. Replay speed is labeled separately from live
dictation latency. Microphone quality comparisons need new controlled captures;
one saved clip cannot establish another microphone's accuracy or restore missing
opening speech.
Dynamic clipboard/date/time snippets or replacements cannot be reproduced from
an old recording and are refused during replay, without reading today's clipboard.
Dictionary changes during a recording are marked explicitly and excluded from
matching that recording's setup to other evidence.

Three configured OpenAI-compatible judges independently evaluate the same
confirmed reference and anonymized candidate outputs. Provider/model identifiers
are hidden from the evaluation prompt. Candidate order varies across judges.
Use a common rubric for missing/invented content, meaning, names, numbers,
negation and applicable formatting. Treat reference/candidate text as data,
never tools or instructions. Return bounded pass/fail/uncertain judgements with
specific evidence. Failures and disagreement stay visible; votes are not shown
as an accuracy percentage. No reference means no grounded judgement. Initial
Beta supports text judges, not proof from audio.

## Textbox edit suggestions (macOS)

"Detect edits after paste" is a separate off-by-default opt-in, available only
while Beta features and local Insights are enabled. It briefly reads the focused
destination textbox on this computer, verifies that our dictation was inserted,
and observes that same field for at most two minutes. This happens at delivery,
so a switch of apps during transcription does not attach edits to the wrong field.

Textboxes must expose bounded, editable, nonsecure native Accessibility text and
selection attributes. Secure, disabled, read-only and unsupported fields and
fields larger than 16K UTF-16 units are ignored. No screenshot, clipboard or
keyboard-logging fallback. Full field snapshots exist only transiently in memory;
the database receives only the original pasted passage and its stable edited
replacement, associated with the History recording and destination app ID.

The watcher stops on focus changes, a new dictation, timeout, consent changes,
Clear, deletion, or ambiguous attribution. Edits to surrounding text, new text
appended outside the pasted passage, disjoint edits, or an unverified insertion
produce no suggestion. Unsupported tracking and unchanged text never prove
that a transcript was correct. Boundary-only changes may be missed deliberately.
Restoring text that existed before the paste, wholesale field rewrites and
indistinguishable unchanged insertions are also ignored to avoid capturing
unrelated contents of an earlier draft or a new message.

History shows detected edits as unconfirmed suggestions, with the original and
edited passage. "Use as desired output" explicitly confirms an Output reference,
replacing an existing one only after the clearly labeled user action. It never
confirms what was spoken or labels the cause as a transcription/formatting error.
Dismissal stays available after opt-out. Application autocorrection can also
cause a detected edit, so observation alone is not evidence of user intent.
No automatic cloud analysis follows detection or confirmation.

## Insights UI

Settings General contains the opt-in, cloud choice and a shared-modal entry
point. Keep advanced judge fields inside the modal. Show evidence and counts by
setup, actual microphone, app and language context; separate reviewed, unknown,
failed and replayed samples. Suggestions require supporting confirmed examples;
small or unmatched samples must not produce an unsupported winner. Missing cost
data is unavailable, not zero. Let the user inspect raw → formatted → final,
feedback, comparison and judge outcomes and cancel active work.

## Validation

Test old configuration defaults; all opt-in and cloud gates; migrations;
immutable originals; independent references; Unicode/bounded word scoring;
deletion/clear races; secret-free snapshots; background cancellation and request
bounds; failed/malformed judges; evidence handling and unreviewed states. Validate
the real React controls and modal at normal and small supported window sizes,
locale completeness, TypeScript build, relevant Rust tests and lint checks.
Also test exact insertion over selections, surrogate-safe UTF-16 attribution,
surrounding/continuation/disjoint edits, secure/unsupported field exclusions,
stale delivery after a new dictation or off/on consent change, observed-versus-
confirmed reference separation, undo, terminal review states and stale revisions.
