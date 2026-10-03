# OpenGlaido - Developer & Agent Instructions

OpenGlaido is an open-source, cross-platform (macOS & Windows 11) voice-layer application combining the best features of **Glaido** and **VoiceInk**.

## Tech Stack
* **Framework:** Tauri v2 (`@tauri-apps/cli`, `@tauri-apps/api`, `tauri` crate)
* **Backend:** Rust
  * `cpal` – Low-latency audio capture (CoreAudio on macOS, WASAPI on Windows)
  * `hound` – 16-bit PCM WAV encoding
  * `rodio` – Real-time audio feedback chimes (ascending on start, descending on stop)
  * `reqwest`, `tokio` & `tokio-tungstenite` – Completed-recording STT requests, live OpenAI/ElevenLabs/Microsoft transcription over WebSockets, and LLM formatting
  * `arboard` & `enigo` – Cross-platform clipboard manipulation and virtual key injection (`Cmd+V` on macOS, `Ctrl+V` on Windows)
  * `whisper-rs` + `llama-cpp-2` (Metal) – Local models, macOS only (`src-tauri/src/models/`). llama.cpp is linked dynamically: `build.rs` stages its dylibs in `src-tauri/libs` for `bundle.macOS.frameworks`; the repo-root `.cargo/config.toml` sets its CMake flags. Building needs CMake.
  * Native STT – A bundled persistent `transcribe.cpp` helper isolates the additional GGML runtime from Whisper and llama.cpp. Build source is pinned and checksum-verified; model weights are optional, verified GGUF downloads. See `native-stt/README.md` and `docs/local-transcription-models.md`.
  * Microsoft local STT – A separate CPU `VibeASR.cpp` helper runs BitNet; a frozen offline Python/PyTorch helper runs full VibeVoice ASR, Streaming 7B and Phi-4 Multimodal on Apple Silicon/macOS 14+. Model groups are pinned, checksum-verified optional downloads. Build the frozen runtime with uv; users need no Python/server setup. See `docs/microsoft-transcription-models.md`.
  * `rusqlite` – Embedded SQLite database for local history, audio clips, dictionary, and snippets
  * `tauri-plugin-global-shortcut` – System-wide push-to-talk hotkeys
  * System tray integration via Tauri 2 TrayIconBuilder
* **Frontend:** React 19 + TypeScript + Tailwind CSS v4 + Vite
  * Floating HUD overlay window (`/#hud`) for non-activating recording state with live waveform animation
  * Settings & activity dashboard for custom voice models, formatting rules, commands, snippets, and dictionary
  * Command window overlay (`/#command`) for command answers and tool approvals

## Implemented Feature Set
1. **Hotkeys:** Native macOS keyboard listener (`src-tauri/src/hotkeys/`, CGEventTap, needs Accessibility) with Glaido-style bindings: hold-to-talk (default `fn`) and hands-free (default `fn+Space`), modifier-only and left/right-specific keys, Esc to cancel, optional Enter to stop, plus Commands hotkeys (default Right ⌥ / Right ⌥+Space). Windows uses `tauri-plugin-global-shortcut` (key combos only). Bindings are recorded in Settings › Hotkeys and re-registered on save.
2. **Dictation bar:** Appears while recording: ten bars driven by one auto-gained mic level (`audio.rs` `Loudness`), still at warm-up, a slow ripple in pauses, independent per-bar motion while speaking (`hud-state.ts` `barHeight`). Processing keeps the icon and sweeps a shimmer over rest-height dots until transcription, formatting and delivery finish. Commands show double chevrons; hands-free recordings and pending processing show an × to cancel; processing cancellation keeps audio in History for Retry (macOS: the hotkey tap catches the click, so the bar never takes clicks or focus). Recording takes priority over earlier pending responses. Cancelled recordings and no-speech results close quietly; only model failures show bar warnings, while other operational issues stay in the main window. Bottom/Raised/High position; never takes focus.
   Menu bar menu matches Glaido: Microphone ▸ (System Default + input devices), Show OpenGlaido, Quit.
3. **Models (Settings › Model):** Transcription runs on this Mac (a downloaded Whisper, Parakeet Ultra, ARK-ASR, Qwen3-ASR, Cohere Transcribe, or Voxtral Realtime model; the additional native models require macOS 12+ and run after recording stops) or with a cloud provider. OpenAI offers GPT Transcribe (after recording) and GPT Live Transcribe (streams while recording, pastes the final transcript on stop); ElevenLabs offers Scribe v2 and Scribe v2 Realtime with the same timing choices. ElevenLabs dictionary hints use its paid keyterms feature (up to 50 short hints live or 100 after recording); local replacements still apply. The separate language model is Off, on this Mac (a downloaded GGUF model; commands then get no tools) or cloud. Other presets include Groq, OpenRouter, Mistral, Gemini, Ollama, and LM Studio, alongside custom OpenAI-compatible endpoints. API keys live in the OS keychain.
   Microsoft offers MAI Transcribe 2 (after recording) and MAI Transcribe 2 Streaming (live preview) through Azure, plus local VibeVoice ASR BitNet, VibeVoice ASR, VibeVoice ASR Streaming 7B and Phi-4 Multimodal. The streaming local model feeds audio incrementally; all modes paste only the final transcript, preserve LLM-Off and support cancellation/History Retry. Azure requires a resource URL/key and a live deployment name; cloud authentication is configured by the user in Settings.
4. **Formatting (Glaido model):** *All apps* style (Standard / Casual / Lowercase, Raw text, custom prompt ≤ 500 chars), the built-in *Email* rule (Gmail, Outlook) and custom rules per app or website, resolved from the frontmost app / browser URL at dictation start.
5. **Commands:** Spoken instructions answered in a floating command window (streamed markdown, selection as context, refine, Enter to paste, ⌘C to copy, background runs with notifications), 8 built-in commands (web search, read a page, YouTube, deep research, math & dates, files & apps, history search, docs search) and custom tools as local MCP servers (`mcp.json`, import folder, new-server scaffold, per-tool approval). Voice activation: start a dictation with "Glaido, …".
6. **Snippets & Dictionary:** Spoken triggers expand to canned text; vocabulary and replacements bias and correct transcripts.
7. **History:** Audio kept in `audio/<id>.wav`, target app icon + name per row, playback, retry that replaces the transcript, failed transcriptions kept with Retry, command answers with sources, ⌘K palette search.
8. **App:** Dark / Light / System theme, 14 UI languages (`src/i18n.ts`, `src/locales/*.json`, checked by `bun run check:locales`), launch at login, menu bar / Dock toggles, mute background while recording, noise reduction and silence detection before upload.
9. **Updates:** Tauri-signed updates from GitHub Releases, checked in the background or in Settings › General. Installation waits for recording, transcription, commands, paste, settings saves, and model downloads to finish and for app windows to close. Development builds cannot install updates. Releases are built, signed, and notarized locally, then uploaded as GitHub drafts; see `docs/releases.md`. No release builds or signing credentials belong in GitHub Actions.

## Getting Started

### Development
```bash
bun install
bun run tauri dev
```

### Build
```bash
bun run tauri build
```
