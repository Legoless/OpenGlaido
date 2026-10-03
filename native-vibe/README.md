# Microsoft BitNet speech helper

`openglaido-vibe` is a persistent, CPU-only helper built from Microsoft's
[VibeASR.cpp](https://github.com/microsoft/VibeASR.cpp). It accepts completed recordings,
keeps its models warm, and supports recognition context from Dictionary. It runs independently
of the app's Whisper and llama.cpp runtimes, avoiding GGML symbol collisions.

The runtime is specifically for **VibeVoice-ASR-BitNet**, not the larger 7B model. Its official
converter forcibly ternarizes projection weights and cannot be reused for the full 7B weights
without changing the model. Supported languages: English, Chinese, French, Italian, Korean,
Portuguese, Vietnamese. It detects the language and cannot force a selected language.

## Pinned source

- Microsoft VibeASR.cpp: `c4334009c88060f86cdbbd684b62662f710b6c20`
- Required llama.cpp/BitNet fork: `a2fdadc20285df2dce90402fca9264a93a8eb32f`
- Both source archives are SHA-256 verified before extracting. Their MIT notices and bundled
  dr_wav/dr_mp3 licenses are retained in `THIRD-PARTY-NOTICES.txt`.
- These pins include Microsoft's ARM numerical fixes (earlier source revisions produced
  incorrect transcripts on Apple Silicon).

Build with `scripts/build-native-vibe.sh aarch64-apple-darwin <cache-directory>`.
The helper links only system libraries and is staged in `src-tauri/binaries/` for Tauri's
macOS external binary packaging. It must receive the same Developer ID signature as the app;
release packaging verifies its architecture and signature before notarization.

## Protocol and safeguards

The server uses Microsoft's line-based requests: `CONTEXT:<hint text>` updates recognition
context; the next request is a private temporary WAV's absolute path. No shell is involved.
Context contains only trimmed, deduplicated, single-line dictionary phrases, capped at 3800
UTF-8 bytes to fit the upstream 4096-byte command buffer. Local replacements remain unchanged.

Replies are modified to use two little-endian u32 values (`status`, `byte length`) followed by
UTF-8 bytes. Status 0 means success, 1 means error; replies are capped at 4 MiB. Initial model
readiness and context acknowledgment are empty success frames. Complete text is emitted once,
so UTF-8/token/newline boundaries and text resembling control markers cannot corrupt framing.
Output-token exhaustion, context exhaustion and failed decode produce errors rather than
successful partial text. Diagnostics go to stderr and are suppressed by the app; protocol goes
only to stdout. Temporary audio is mode 0600 and deleted after the helper finishes or is killed.

Cancellation and quit interrupt response waits in at most one 50 ms polling interval, kill the
helper and wait for physical exit. Update activity admission is retained through process exit,
even if the async caller drops its future. A cancelled or failed helper is freshly started for
the next recording. `--self-test` checks completion and protocol byte order without downloading
weights; Rust regressions cover framing, input bounds, cancellation, and private audio cleanup.
