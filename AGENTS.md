# OpenGlaido - Developer & Agent Instructions

OpenGlaido is an open-source, cross-platform (macOS & Windows 11) voice-layer application combining the best features of **Glaido** and **VoiceInk**.

## Tech Stack
* **Framework:** Tauri v2 (`@tauri-apps/cli`, `@tauri-apps/api`, `tauri` crate)
* **Backend:** Rust
  * `cpal` – Low-latency audio capture (CoreAudio on macOS, WASAPI on Windows)
  * `hound` – 16-bit PCM WAV encoding
  * `rodio` – Real-time audio feedback chimes (ascending on start, descending on stop)
  * `reqwest` & `tokio` – Async STT API requests (OpenAI-compatible) and LLM formatting
  * `arboard` & `enigo` – Cross-platform clipboard manipulation and virtual key injection (`Cmd+V` on macOS, `Ctrl+V` on Windows)
  * `whisper-rs` + `llama-cpp-2` (Metal) – Local models, macOS only (`src-tauri/src/models/`). llama.cpp is linked dynamically: `build.rs` stages its dylibs in `src-tauri/libs` for `bundle.macOS.frameworks`; the repo-root `.cargo/config.toml` sets its CMake flags. Building needs CMake.
  * `rusqlite` – Embedded SQLite database for local history, audio clips, dictionary, and snippets
  * `tauri-plugin-global-shortcut` – System-wide push-to-talk hotkeys
  * System tray integration via Tauri 2 TrayIconBuilder
* **Frontend:** React 19 + TypeScript + Tailwind CSS v4 + Vite
  * Floating HUD overlay window (`/#hud`) for non-activating recording state with live waveform animation
  * Settings & activity dashboard for custom voice models, formatting rules, commands, snippets, and dictionary
  * Command window overlay (`/#command`) for command answers and tool approvals

## Implemented Feature Set
1. **Hotkeys:** Native macOS keyboard listener (`src-tauri/src/hotkeys/`, CGEventTap, needs Accessibility) with Glaido-style bindings: hold-to-talk (default `fn`) and hands-free (default `fn+Space`), modifier-only and left/right-specific keys, Esc to cancel, optional Enter to stop, plus Commands hotkeys (default Right ⌥ / Right ⌥+Space). Windows uses `tauri-plugin-global-shortcut` (key combos only). Bindings are recorded in Settings › Hotkeys and re-registered on save.
2. **Dictation bar:** Appears while recording (live mic-level waveform) or showing a model failure; hides immediately when recording stops, while transcription continues in the background. No-speech results never reopen the bar; other operational issues stay in the main window. Bottom/Raised/High position; never takes focus.
   Menu bar menu matches Glaido: Microphone ▸ (System Default + input devices), Show OpenGlaido, Quit.
3. **Models (Settings › Model):** Transcription runs on this Mac (a Whisper model downloaded in-app) or with a cloud provider; the language model is Off, on this Mac (a downloaded GGUF model; commands then get no tools) or cloud. Cloud = presets (Groq, OpenAI, OpenRouter, Mistral, Gemini, Ollama, LM Studio) or any OpenAI-compatible endpoint. API keys live in the OS keychain.
4. **Formatting (Glaido model):** *All apps* style (Standard / Casual / Lowercase, Raw text, custom prompt ≤ 500 chars), the built-in *Email* rule (Gmail, Outlook) and custom rules per app or website, resolved from the frontmost app / browser URL at dictation start.
5. **Commands:** Spoken instructions answered in a floating command window (streamed markdown, selection as context, refine, Enter to paste, ⌘C to copy, background runs with notifications), 8 built-in commands (web search, read a page, YouTube, deep research, math & dates, files & apps, history search, docs search) and custom tools as local MCP servers (`mcp.json`, import folder, new-server scaffold, per-tool approval). Voice activation: start a dictation with "Glaido, …".
6. **Snippets & Dictionary:** Spoken triggers expand to canned text; vocabulary and replacements bias and correct transcripts.
7. **History:** Audio kept in `audio/<id>.wav`, target app icon + name per row, playback, retry that replaces the transcript, failed transcriptions kept with Retry, command answers with sources, ⌘K palette search.
8. **App:** Dark / Light / System theme, 14 UI languages (`src/i18n.ts`, `src/locales/*.json`, checked by `bun run check:locales`), launch at login, menu bar / Dock toggles, mute background while recording, noise reduction and silence detection before upload.

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
