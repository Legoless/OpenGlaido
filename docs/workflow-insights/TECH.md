# Workflow Insights implementation

## Boundaries

Use the existing SQLite database and History audio. `workflow.rs` owns typed,
allowlisted settings snapshots, append-only evaluation runs, explicit references,
bounded word-error scoring and database migrations. `workflow_analysis.rs` owns
one cancellable comparison/judge task at a time and exposes Tauri commands.
`lib.rs` owns session instrumentation and IPC registration. New React modules
own the modal and History feedback, reusing shared controls and localization.

Store settings under `TranscriptionConfig.workflow_insights` with serde defaults:
`enabled`, `cloud_analysis`, and at most three judge configurations containing
provider, endpoint and model identifiers. Credentials remain in existing scoped
OS-keychain accounts and are resolved only for actual analysis requests.

## Evidence lifecycle

Record one immutable original run per History row and append comparison runs.
Foreign keys or explicit same-transaction deletion tie evidence to History.
The data store has a clear-generation token: writes from analysis begun before
Clear are rejected. Feedback updates invalidate judgements based on the previous
reference revision. Bound text length, row retrieval, candidate count, request
count, timeouts and judge output; untrusted model output cannot add arbitrary
data or claim a run outside its input set.
Retain one original and the latest eight comparisons per History recording;
replace old jury batches with the latest three slots per run/reference (at most
54 judgement rows). Reports retrieve at most 200 histories. Word scoring runs
after the shared database lock is released. History deletion also advances the
writer generation, preventing a late legacy cancellation from recreating
Insights even if it recreates a History row; pending evaluation may need rerun.

Snapshot construction is an allowlist, never serialization of hydrated config.
Include model/source and configured-backup provenance, resolved formatting,
language and dictionary revision, app identity and website host only. Capture
native microphone metadata and first-sample timestamp before audio processing;
measure clipping only for opted-in sessions. Live original audio and saved
RNNoise replay inputs are labeled distinctly.
Capture the collection generation before microphone startup and reject presses
preceding Clear or a consent change. If dictionary terms change during capture
or processing, flag the original snapshot as uncertain instead of attributing
new terms to its old revision.

## Analysis

Explicit comparisons reuse cancellable STT and a detailed text finishing helper.
Disable backup fanout for an individual replay. Require idle dictation/processing
before starting, cancel on new capture or opt-out, and hold updater activity so
installation cannot interrupt analysis. Do not add analysis to HUD processing or
paste delivery queues. A comparison makes at most one STT and one formatting
request. A judge action makes at most three HTTP requests, without automatic
retries. Cloud consent is checked in the backend, not just in disabled buttons.

Judges use credential-free configured URLs (HTTPS or local HTTP), no redirects,
bounded response parsing and generic failures. Three distinct configurations
are required. Judge provider keys are selected from existing scoped accounts.
Never log submitted references, transcripts, keys or raw provider bodies.
Validate returned verdicts, issue lengths/categories and candidate attribution.
The shared formatter also refuses redirects, so a local endpoint cannot forward
text remotely behind the cloud consent gate. Replays refuse dynamic snippet or
dictionary expansions rather than reading the current clipboard or clock.

## Limits

There is no always-on editor monitoring, automatic settings change, telemetry
server, audio-based jury or verified paste-insertion measurement. Saved audio
cannot benchmark microphone alternatives. Replay timing does not measure normal
live latency. Three agreeing text judges do not establish correctness. Costs
remain unavailable unless directly measured; fixed per-action request bounds
limit usage without inventing dollar estimates.

## Bounded edit observation

`edit_tracking.rs` owns a single generation-bound watcher. Capture its generation
with the recording, and pass it through the ordinary dictation paste context.
Snapshot the destination field immediately before successful keyboard injection;
recheck clipboard ownership after the optional AX query. Delayed deliveries
cannot adopt a newer recording's generation or resume after consent off/on.
Commands and History Retry do not start observation.

`frontmost::capture_textbox` reacquires AX handles on each sample and validates
process/window/control identity. Existing retained FocusTarget handles remain
identity-only. Native messaging is bounded to 100ms overall and 20ms per call;
require AXNumberOfCharacters, AXSelectedTextRange and AXStringForRange, typed
UTF-16 ranges and a 16K-unit cap. Secure/read-only/disabled/self/terminal targets
fail closed before any range text read.

Verify the exact selected replacement before observing. Keep unchanged whole-
field prefix/suffix anchors in RAM, poll every 300ms for at most 120s, and persist
only stable edited middles after 600ms. Abort ambiguous changes; do not infer
unchanged means correct. A separate FK-backed, epoch-checked observation table
stores one latest suggestion per History row. Confirm/dismiss use observation
revision checks; terminal reviews reject late watcher updates. Explicit
confirmation writes only Output feedback and invalidates its prior jury.
Clear and History deletion cascade observations. No observer request uses
keychain credentials, models, cloud services, screenshots or current clipboard.
Keep a RAM-only fingerprint of the pre-insertion field: an Undo that restores it
must stop observation instead of storing the old selection. Reject whole-field
rewrites and pastes that do not observably change the field. These conservative
cases remain manual feedback; no prior field text or fingerprint enters SQLite.

## Verification

Use SQLite/temp-directory regression tests and local mock HTTP endpoints; never
run paid requests or personal recordings as tests. React tests mock IPC and use
explicit synthetic transcripts. Include an interactive fixture for the opt-in,
feedback, review and analysis UI. Keep product notes aligned as implementation
reveals constraints; run the existing repository checks before completion.
