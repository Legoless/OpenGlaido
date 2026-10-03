# Microsoft transcription

Microsoft models are selected in **Settings › Model › Transcription**. Cloud
models use Azure; local models are optional downloads managed by OpenGlaido.
These choices transcribe speech in its spoken language. They do not enable
translation or change the separate language-model setting. Select **Language
model: Off** to use their transcript without a cleanup-model request.

## Cloud

| Choice | Model ID | Recording behavior |
| --- | --- | --- |
| MAI Transcribe 2 | `mai-transcribe-2` | Uploads a complete WAV after recording stops. |
| MAI Transcribe 2 Streaming | `mai-transcribe-2-streaming` | Streams microphone audio while recording; commits and pastes the final transcript after stop. |

Select **Cloud → Microsoft**, enter the Azure resource root URL and API key,
then choose a model. Supported roots are
`https://RESOURCE.cognitiveservices.azure.com` and
`https://RESOURCE.services.ai.azure.com`. OpenGlaido derives the appropriate
service hostname for each mode while retaining the saved resource/key scope.
Do not enter a project URL or append an API path. Keys remain in the OS
keychain and are sent in request headers. Redirects are rejected.

The completed-recording choice uses Azure Speech Fast Transcription with
`enhancedMode.model=MAI-Transcribe-2` and verbatim output. It needs an Azure
resource with access to that service. The live choice additionally needs a
Foundry deployment of **MAI-Transcribe-2-Streaming**. Enter its deployment name
in the live model's deployment field; a blank field uses
`MAI-Transcribe-2-Streaming`. Access and region availability depend on Azure.
Streaming is a public-preview service. See Microsoft's [completed-recording
guide](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/mai-transcribe)
and [live configuration and protocol](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/mai-transcribe-2-streaming-realtime).

Live audio is mono PCM16 at 24 kHz over a secure WebSocket. OpenGlaido disables
server turn detection, sends an explicit commit on stop and waits for a final
completed event. Partial text is not pasted while speaking. One selected
dictation language is sent as a hint; zero or multiple selections use automatic
language detection. A live session is limited to one hour.

Dictionary phrases are sent as recognition hints with completed recordings.
The dedicated MAI live schema has no documented glossary field, so live mode
uses dictionary replacements locally after receiving the final transcript.
See the Azure [phrase-list reference](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/fast-transcription-create).

## Local

Sizes below are complete downloads in decimal GB, including required encoders,
tokenizers and configuration files. They are disk sizes, not RAM estimates.

| Choice | Model ID | Download | Languages | Recording behavior |
| --- | --- | --- | --- | --- |
| VibeVoice ASR BitNet | `vibevoice-bitnet` | 1.70 GB | 7 | After recording; CPU helper, macOS 12+. |
| VibeVoice ASR | `vibevoice-asr` | 17.36 GB | 51, including Slovenian | After recording; Apple Silicon, macOS 14+. |
| VibeVoice ASR Streaming 7B | `vibevoice-asr-streaming-7b` | 17.36 GB | 10 | Live local audio; Apple Silicon, macOS 14+. |
| Phi-4 Multimodal | `phi-4-multimodal` | 11.17 GB | 8 speech languages | After recording; Apple Silicon, macOS 14+. |

Download models individually from **On this Mac**. All four require about
47.59 GB together. A model becomes available only after all its companion
files are complete. Every downloaded artifact has a pinned revision, byte size
and SHA-256 digest in [the download manifest](../src-tauri/src/models/microsoft-files.json).
Cancellation retains partial downloads for resumption; deletion removes the
whole model group. The three larger models have substantial memory and
inference costs; disk sizes do not establish their latency on a particular Mac.

BitNet supports English, Chinese, French, Italian, Korean, Portuguese and
Vietnamese. Streaming supports English, Chinese, Spanish, Portuguese, German,
Japanese, Korean, French, Russian and Italian. Phi's speech support is English,
German, Spanish, French, Italian, Japanese, Portuguese and Chinese; its wider
text-language metadata is not advertised as speech support. Unsupported
selected languages produce an error instead of silently switching to English.

The streaming checkpoint requires an initial audio window of approximately
3.5 seconds before its first inference. This comes from its pinned chunk and
lookahead configuration, and is not a promise of a 3.5-second response. Audio
is consumed incrementally using the model's streaming state and KV cache.
Releasing the recording shortcut sends the remaining samples for finalization;
the final transcript follows the same formatting and paste flow as cloud models.

VibeVoice models receive Dictionary phrases as hotword context. BitNet context
is capped at 3,800 UTF-8 bytes; the packaged helper bounds metadata at 64 KiB.
Phi uses local spelling replacements after transcription. All model outputs
are reduced to transcript content, so speaker labels, timestamps and structured
metadata are not pasted. The model's separate multimodal/chat abilities are
not exposed by this transcription integration.

## Runtime and recovery

BitNet uses the bundled `openglaido-vibe` C++ helper. The other three use a
self-contained `openglaido-microsoft` runtime with pinned Microsoft source and
dependencies. Users do not install Python, run a server or download executable
model code. The portable runtime is roughly 1.2 GiB; model
weights remain separate downloads. Inference loads local files with Hugging
Face offline settings. Runtime and model-source details are in
[native-vibe](../native-vibe/README.md) and
[native-microsoft](../native-microsoft/README.md).

The bar stays in progress until transcription, optional formatting and delivery
finish. Its × cancels pending work. Local cancellation terminates and reaps the
helper; another recording starts a fresh helper. Processing cancellation keeps
the audio in History for Retry. Retry uses the currently selected model and
dictionary, including the live adapters for an already completed recording.
Cancellation and model switching preserve the existing recording controls and
never change the user's language-model selection.

## Validation

Protocol tests cover Azure multipart and WebSocket requests, finalization,
credential handling and cancellation. Local checks cover framed requests,
buffer bounds, incremental streaming, retained final audio, helper reuse,
process shutdown and multi-file download readiness. Helper self-tests run
without model weights; runtime import checks verify the bundled model classes.
These checks do not establish speech accuracy or inference speed.

On 2026-10-03, all four pinned checkpoints transcribed the public Whisper JFK
sample correctly on the development M5 Max Mac (128 GB), including a second
request in the same process. Streaming returned partial text before finalization
and retained the final 500 ms of audio. In the source runtime, the three large
models took about 40–55 seconds to load cold; warm 11-second transcriptions took
about 5 seconds.
These are smoke checks, not a multilingual accuracy benchmark.

All four models also passed first and warm requests using the signed app's
packaged helpers. The frozen runtime remained byte-identical after the final
Rust capture fix. A delayed-start regression preserved 9,000 microphone
callbacks while the model loaded, including the first samples and final tail;
the final checks passed 200 Rust tests, 49 Bun tests and all-targets Clippy.

Authenticated Azure testing is deferred, as requested, until the user configures
a resource and deployment in Settings.

The real-inference regression can be reproduced with a public/synthetic WAV:

```sh
python3 scripts/check-microsoft-stt.py --wav /path/to/jfk.wav --expect country --expect country
```

## Pinned model sources

- [VibeVoice ASR BitNet](https://huggingface.co/microsoft/VibeVoice-ASR-BitNet):
  `66e78021ab8f5f06133d1ab421ba4d348bda97c9`.
- [VibeVoice ASR](https://huggingface.co/microsoft/VibeVoice-ASR):
  `d0c9efdb8d614685062c04425d91e01b6f37d944`.
- [VibeVoice ASR Streaming 7B](https://huggingface.co/microsoft/VibeVoice-ASR-Streaming-7B):
  `60d858b518b4e19d404af3737f848fc185b30177`.
- [Phi-4 Multimodal](https://huggingface.co/microsoft/Phi-4-multimodal-instruct):
  `93f923e1a7727d1c4f446756212d9d3e8fcc5d81`.

The full VibeVoice model also uses a pinned Qwen2.5 tokenizer. Exact companion
files and digests are recorded in the download manifest; language metadata
comes from the pinned model cards, with Phi's speech-language override in
`models/python.rs`.
