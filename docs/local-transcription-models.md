# Local transcription models

OpenGlaido's Mac model picker includes five additional transcription families alongside Whisper. Each is an optional, single-file GGUF download from Hugging Face. Model weights are separate from the app installer; users download only the models they select. The catalog pins each repository revision and verifies the file's exact size and SHA-256 before installation.

All five require macOS 12 or newer and run after recording stops. “Realtime” is part of the Voxtral model's name, not a live transcription mode in OpenGlaido. Audio stays on the Mac during local transcription; a separately enabled cloud language model still receives the transcript for formatting. Turn the language model off, or select a local one, to keep that step local too.

## Downloads and hosting

Metadata below was checked against the Hugging Face API on 2026-10-02. These Q8 variants prioritize fidelity over minimum download size. The earlier compact-format estimates mixed CoreML, MLX and smaller GGUF quantizations; those are not the files used here.

| Model | Download bytes | Decimal size | Publisher / revision |
| --- | ---: | ---: | --- |
| Parakeet Ultra 0.6B Q8 | 739,508,704 | 0.74 GB | [Nairod785](https://huggingface.co/Nairod785/parakeet-ultra-gguf/tree/b03613ba54a195238f0e915359f5a5c78269ddc6) |
| ARK-ASR 3B Q8 | 4,289,201,792 | 4.29 GB | [harshav](https://huggingface.co/harshav/ARK-ASR-3B-GGUF/tree/14239df600b76b1fd697423eb927f9f4622fe739) |
| Qwen3-ASR 1.7B Q8 | 2,185,030,624 | 2.19 GB | [handy-computer](https://huggingface.co/handy-computer/Qwen3-ASR-1.7B-gguf/tree/3555bd238a8572bbace3ebf60d23b036dc0a5dbe) |
| Cohere Transcribe 2B Q8 | 2,410,655,232 | 2.41 GB | [handy-computer](https://huggingface.co/handy-computer/cohere-transcribe-03-2026-gguf/tree/ab667acedcb5d8d56837ef3ed3568c3ad50dde47) |
| Voxtral Realtime 4B Q8 | 4,731,791,648 | 4.73 GB | [handy-computer](https://huggingface.co/handy-computer/Voxtral-Mini-4B-Realtime-2602-gguf/tree/65d1c9408859a0ca0f1c11b025ee951486af21d6) |
| **All five** | **14,356,188,000** | **14.36 GB / 13.37 GiB** | |

No OpenGlaido weight hosting is required. A mirror of this selection needs 14.36 GB plus storage for retained versions, and bandwidth for each download. Disk size is not a RAM requirement: inference also allocates activations, caches and audio buffers. Memory and latency depend on the model, clip length and Mac; no universal RAM or speed rating is claimed.

## Languages and dictionary

Language lists describe model support, not an accuracy guarantee for every accent or language. The native engine accepts 16 kHz mono audio.

| Model | Supported language codes | Language selection |
| --- | --- | --- |
| Parakeet Ultra | bg, hr, cs, da, nl, en, et, fi, fr, de, el, hu, it, lv, lt, mt, pl, pt, ro, ru, sk, **sl**, es, sv, uk | Automatic detection |
| ARK-ASR | zh, en, de, ja, fr, ko, es, pl, it, ro, hu, cs, nl, fi, hr, sk, **sl**, et, lt | Automatic detection |
| Qwen3-ASR | zh, en, yue, ar, de, fr, es, pt, id, it, ko, ru, th, vi, ja, tr, hi, ms, nl, sv, da, fi, pl, cs, fil, fa, el, ro, hu, mk | Automatic detection or a language hint |
| Cohere Transcribe | en, fr, de, es, it, pt, nl, pl, el, ar, ja, zh, vi, ko | An explicit supported language is required |
| Voxtral Realtime | en, fr, es, de, ru, zh, ja, it, pt, nl, ar, hi, ko | Automatic detection |

ARK's original model card lists 19 languages, including Slovenian, while its conversion's metadata lists only seven. The conversion publisher's regression checks used English recordings; its broader language quality has not been independently established by those checks. [Original ARK model](https://huggingface.co/Edge0/ARK-ASR-3B), [conversion validation](https://huggingface.co/harshav/ARK-ASR-3B-GGUF).

Dictionary replacements and snippets still apply after transcription. These five integrations do not pass dictionary hints to the speech model. Qwen's upstream runtime supports prompting, but that capability is not wired through this integration. Existing Whisper prompt handling is unchanged.

## Runtime and attribution

The GGUFs use the bundled native `transcribe.cpp` engine; they are not interchangeable with Whisper's GGML files or a general text-only GGUF runtime. ARK requires the accompanying ARK-enabled fork. No Python installation, model server or model-specific API key is needed. The engine's license and pinned source information belong with the bundled runtime.

- **Parakeet Ultra:** © Moondream, a post-training of NVIDIA's Parakeet TDT 0.6B v3. Weights are [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). Nairod785 converted the weights to GGUF and quantized their precision; the conversion omits the upstream voice-activity head. [Original model](https://huggingface.co/moondream/parakeet-ultra), [conversion and attribution](https://huggingface.co/Nairod785/parakeet-ultra-gguf).
- **ARK-ASR:** Edge0 / AutoArk; [Apache 2.0 original model](https://huggingface.co/Edge0/ARK-ASR-3B). The selected GGUF is harshav's conversion, using source checkpoint `1e28271b79edc97635783bea65abc89195a09ed3`.
- **Qwen3-ASR:** Qwen / Alibaba; [Apache 2.0 original model](https://huggingface.co/Qwen/Qwen3-ASR-1.7B). GGUF conversion by handy-computer.
- **Cohere Transcribe:** Cohere Labs; [Apache 2.0 original model](https://huggingface.co/CohereLabs/cohere-transcribe-03-2026). GGUF conversion by handy-computer.
- **Voxtral Realtime:** Mistral AI; [Apache 2.0 original model](https://huggingface.co/mistralai/Voxtral-Mini-4B-Realtime-2602). GGUF conversion by handy-computer.

Exact file names, immutable download URLs and SHA-256 values are maintained in [`catalog.rs`](../src-tauri/src/models/catalog.rs). A model being downloadable does not establish that it is more accurate or faster than the existing Whisper recommendation; compare real recordings on the target Mac.

## Real-model verification

Run `python3 scripts/check-native-stt.py --helper <openglaido-stt> --wav <whisper.cpp/samples/jfk.wav> <model1.gguf> <model2.gguf> ...` with all five downloaded files. Missing models and failed transcription fail the check. It verifies two sequential requests reuse each loaded process, a recording crossing the chunk boundary is not truncated, and EOF shuts the helper down. This smoke test uses public test audio and does not establish accuracy for every supported language.

On 2026-10-02 all five pinned Q8 artifacts passed this check on the development Mac: first and warm 11-second JFK requests, a 44-second repeated recording crossing a chunk boundary, and clean helper exit. All five downloads matched the catalog SHA-256. These are integration smoke tests, not a multilingual accuracy benchmark.
