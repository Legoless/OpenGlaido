# Microsoft speech helper

`helper.py` loads verified local VibeVoice-ASR, VibeVoice-ASR-Streaming-7B,
or Phi-4-multimodal checkpoints. The runtime builder freezes it into the
`openglaido-microsoft` executable. Users do not need a separate Python or package
installation. Model weights remain optional downloads.

The Python models require Apple Silicon and macOS 14 or later. The helper keeps
one model loaded. Its Hugging Face calls are local-only,
network access is disabled through Hugging Face offline settings, and stdout
is reserved for the binary protocol. Model/library logs go to stderr.

## Source and dependencies

VibeVoice source is copied from Microsoft commit
`1541f590c7099820f10ea012f48d2399282df69f`; `vendor-source.json` records the
original file hashes. Local patches report a streaming chunk's EOS/chunk-end
completion and honor the full model's assistant generation prompt. Output-token
exhaustion cannot become a successful partial transcript. Preserve the MIT and
Apache-2.0 notices when bundling.

Run `openglaido-microsoft --check-runtime` to import every model class and print
installed dependency versions without loading weights.

The shared runtime requires a matched PyTorch/torchvision pair,
`transformers==4.51.3`, `peft==0.13.2`, NumPy, SciPy, Pillow, accelerate,
`diffusers==0.32.2`, backoff, and tqdm. It receives PCM arrays directly, so neither
FFmpeg nor torchaudio is needed. Transformers 4.52 changes generation
inheritance required by the pinned Phi code; do not casually loosen that pin.

Full VibeVoice-ASR requires Qwen2.5-7B tokenizer files alongside its weights.
The streaming checkpoint carries its own tokenizer and frame configuration.
Phi Python classes are bundled from Microsoft model revision
`93f923e1a7727d1c4f446756212d9d3e8fcc5d81`. The helper instantiates those
classes directly rather than executing checkpoint `auto_map` references or
Hugging Face remote code. Model downloads contain only data, configuration and
licenses; no Python code is downloaded during inference. Both exported adapter
folders are unnecessary: the main Phi weight shards already contain speech
and vision adapters.

## Building and packaging

Install [uv](https://docs.astral.sh/uv/getting-started/installation/) on the build
Mac, then run:

```sh
python3 scripts/build-microsoft-runtime.py
```

The script creates a Python 3.12 build environment under
`src-tauri/target/microsoft-runtime`, installs the hash-locked dependencies and
uses PyInstaller to freeze an `onedir` runtime. It runs source and frozen helper
self-tests and import checks before staging the result in
`src-tauri/microsoft-runtime`. These generated directories are ignored by Git.
The portable runtime is approximately 1.2 GiB; weights are not included. Normal Tauri
development and app builds invoke this preparation step automatically.

Set `APPLE_SIGNING_IDENTITY` for release builds. The builder signs each bundled
Mach-O library and helper with hardened runtime and timestamps; Tauri's ordinary
resource copying does not sign nested Python extension libraries. The app's
release verifier checks the packaged helper after signing and notarization.
The builder materializes library aliases before signing, uses the standalone
Python library instead of an ambiguous copied framework, and prevents bytecode
writes into signed resources. CPython and bootloader licenses are included.

Artifact URLs, sizes and SHA-256 digests are pinned in
`src-tauri/src/models/microsoft-files.json`; model revisions are also recorded
in `model-revisions.json`. Full VibeVoice and streaming downloads are about
17.36 GB each, and Phi is 11.17 GB. Download readiness requires every companion
file. See [Microsoft transcription behavior](../docs/microsoft-transcription-models.md)
for languages, model selection and cloud configuration.

## Protocol

Launch with one family and a model-directory path:

```text
openglaido-microsoft vibevoice-asr MODEL_DIR
openglaido-microsoft vibevoice-asr-streaming MODEL_DIR
openglaido-microsoft phi4-multimodal MODEL_DIR
```

After loading, the helper sends status `0` and empty text. Requests contain
four little-endian `u32` values: operation, sample count, language byte count,
and dictionary-hints byte count. These are followed by a UTF-8 language code,
a UTF-8 JSON array of hint strings, and mono 16 kHz little-endian float32 PCM.
Metadata is bounded to 64 KiB, audio to 30 minutes, and responses to 4 MiB.
Responses contain status (`0` success, `1` error), UTF-8 byte count and text.

| Operation | Behavior |
| --- | --- |
| `0` | Transcribe a completed recording; sample count must be positive. |
| `1` | Start/reset a live session with hints; sample count must be zero. |
| `2` | Append microphone samples and return current completed content. |
| `3` | Append any tail, pad the final window and return the final transcript. |

Live operation is available for VibeVoice-ASR-Streaming-7B only. It uses the
checkpoint's chunk/lookahead settings and Microsoft's incremental KV-cache
API; it does not rerun the whole recording on every microphone chunk.
Finalization is accepted once per reset. Stopping or killing the child is the
parent's cancellation mechanism.

VibeVoice receives dictionary hotwords as context. Phi currently applies
replacement rules in OpenGlaido afterward. Full VibeVoice JSON is reduced to
content strings; the streaming model's anchored `Speaker N:` labels are removed.
Speaker metadata is never pasted. All generation
paths reject missing completion markers. Phi recordings longer than 30 seconds
use overlapping 30-second chunks with exact word-boundary deduplication.

Run the bounded protocol, parser and live-window checks without model packages:

```sh
python3 native-microsoft/helper.py --self-test
```

Real audio and packaged-runtime tests remain necessary after building. The
pure checks cannot establish MPS accuracy, memory consumption or latency.
