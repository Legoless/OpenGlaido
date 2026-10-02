# Local speech helper

`build.rs` builds this persistent, isolated process on macOS. `tauri.macos.conf.json`
packages and signs it as `Contents/MacOS/openglaido-stt`; development builds copy it
next to the application executable. All engine/GGML code is linked statically and
Metal shaders are embedded, avoiding symbol collisions with Whisper and llama.cpp.
The CMake build cache lives under `target/native-stt/<target-triple>` and is shared
by Cargo check, Clippy, tests, and release builds.
Model weights remain separate, verified downloads; the helper does not use networking.
The pinned Metal engine requires macOS 12 or newer; older systems receive a clear
error before engine initialization (the existing Whisper engine is unaffected).

## Source pin

The build downloads the `transcribe.cpp-arkasr-b838a2b.tar.gz` source snapshot from
`harshav/ARK-ASR-3B-GGUF` at Hugging Face revision
`14239df600b76b1fd697423eb927f9f4622fe739`, verified against SHA-256
`776c2a7a690dc21dc629aa79d03003a0a9f9a463f1e0a4f3a4cefc6ee59dbe97` before extraction.

Compared with upstream `handy-computer/transcribe.cpp` revision `223c9b0`, its runtime
changes are the new `arch/arkasr` implementation, architecture/CMake registration,
and optional Qwen2 projection biases in `causal_lm`. Existing headers are identical.
The ARK additions load GGUF weights and execute local tensor graphs; they contain
no network or subprocess calls. GGML is the archive's vendored revision
`707321c4cf6d21cb4bc831aa8b687dbf01a521ce`.

This pin does not expose dictionary prompting for these five models. Local dictionary
replacements still apply. ARK infers language and ignores hints; Cohere requires an
explicit language. All five currently run on completed recordings, including Voxtral
Realtime. This helper does not implement live streaming.

## Protocol

Start `openglaido-stt <absolute-model.gguf>`. It loads the model/session once, then
returns a ready response with status 0 and zero text bytes. All integers and audio
samples are little-endian; stdout contains only protocol frames, stderr engine logs.

Request: `u32 sample_count`, `u32 language_byte_count`, language UTF-8 bytes, then
`sample_count` normalized mono 16 kHz float32 samples. Empty language selects automatic
detection where available. Requests are sequential, up to 30 minutes; language codes
are at most 32 bytes and contain lowercase ASCII letters/hyphens. NaN, infinity,
truncated frames, and out-of-range audio are rejected.

Response: `u32 status`, `u32 text_byte_count`, UTF-8 text. Status 0 means success;
status 1 means an error. Responses are limited to 4 MiB. Per-run engine failures
return an error without partial text; malformed protocol or initialization failure
returns an error and exits. EOF closes the helper. The parent owns timeouts and kill.

Recordings longer than 30 seconds are split near quiet boundaries with 750 ms overlap.
Each window also respects 90% of the engine's advertised input limit. Exact word or
UTF-8 boundary overlap is removed when joining results; this bounds peak memory and
avoids silently truncated long recordings. The chunk stitching is heuristic and
may retain duplicates where a model transcribes overlapping speech differently.

`openglaido-stt --self-test` tests framing, malformed input, nonfinite audio, chunk
boundaries, and overlap stitching without model downloads. It runs during every
helper build and again on the packaged binary in the local release workflow.
