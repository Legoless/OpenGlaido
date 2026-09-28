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
  * `rusqlite` – Embedded SQLite database for local history, audio clips, dictionary, and snippets
  * `tauri-plugin-global-shortcut` – System-wide push-to-talk hotkeys
  * System tray integration via Tauri 2 TrayIconBuilder
* **Frontend:** React 19 + TypeScript + Tailwind CSS v4 + Vite
  * Floating HUD overlay window (`/#hud`) for non-activating recording state with live waveform animation
  * Settings & activity dashboard for custom voice models, writing styles/modes, snippets, and dictionary

## Implemented Feature Set
1. **Push-to-Talk & Hotkeys:** Global shortcut (`CommandOrControl+Shift+Space`) listens system-wide to toggle/push-to-talk dictation.
2. **Audio Feedback:** Subtle harmonic audio tones (via `rodio`) indicating when listening starts and stops.
3. **Custom Voice Model (STT):** Point to any remote or self-hosted OpenAI-compatible endpoint (Groq Whisper, faster-whisper-server, RunPod, Speaches, etc.).
4. **Writing Styles & Modes:**
   - *Default:* Cleans up filler sounds and applies standard capitalization/punctuation.
   - *Email:* Professional tone, paragraphs, polite phrasing.
   - *Chat:* Fast, punchy, concise messaging for Slack/Discord.
   - *Code:* Translates spoken programming terminology.
   - *Raw:* Verbatim output from Whisper without LLM changes.
5. **Voice Snippets:** Spoken triggers (e.g., "my email") automatically expand to canned text.
6. **Custom Dictionary & Jargon:** Terminology and brand replacements stored in local SQLite.
7. **Audio History & Retranscription:** Audio clips are preserved in `audio/<id>.wav` with in-app audio playback and one-click re-transcription.

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
