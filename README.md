# OpenGlaido

**OpenGlaido** is a free, open-source, cross-platform voice layer for macOS and Windows, bringing together the best features of **Glaido** and **VoiceInk**.

Press your shortcut, speak naturally, and messy speech is converted to clean, formatted text pasted directly at your cursor—powered by Whisper and custom LLM post-processing.

---

## Features

- 🎙️ **Cross-Platform**: Built with **Tauri 2** and **Rust** to run natively on both **macOS** and **Windows 11**.
- 🔔 **Audio Feedback Chimes**: Subtle acoustic chimes when dictation starts and stops (powered by `rodio`).
- ⚡ **Bring Your Own Voice Model**: Connect to any OpenAI-compatible speech-to-text endpoint (`POST /v1/audio/transcriptions`):
  - Groq Whisper (ultra-fast ~200ms latency)
  - Self-hosted [faster-whisper-server](https://github.com/fedirz/faster-whisper-server)
  - Speaches, vLLM, RunPod, Modal, or OpenAI
- 🎯 **App Styles & Writing Modes** (VoiceInk Power Modes / Glaido Styles):
  - **Standard Clean-up:** Removes filler words (*um*, *uh*), fixes punctuation and capitalization.
  - **Email Mode:** Formats professional email prose with polite phrasing.
  - **Chat Mode:** Fast, concise messaging for Slack, Discord, and Teams.
  - **Code Mode:** Understands programming syntax and developer jargon.
  - **Raw Mode:** Raw verbatim output directly from Whisper without AI edits.
- ⚡ **Voice Snippets**: Short spoken trigger phrases (e.g., *"my email"*) expand into long canned templates.
- 🎯 **Floating HUD Pill**: Non-intrusive transparent overlay indicator with animated waveform during dictation.
- 📝 **Custom Dictionary**: Teach OpenGlaido proper nouns, tech jargon, and custom spelling replacements.
- 📜 **Activity History with Audio Playback**: Listen back to recorded audio clips directly in the app and re-transcribe anytime.
- 💻 **System Tray Menu**: Quick access to start/stop dictation, open dashboard, or quit from the menu bar / system tray.

---

## Tech Stack

- **Framework:** [Tauri v2](https://v2.tauri.app)
- **Backend:** Rust (`cpal` for microphone input, `hound` for WAV, `rodio` for audio tones, `arboard` for clipboard, `enigo` for keystrokes, `rusqlite` for database)
- **Frontend:** React 19, TypeScript, Tailwind CSS v4, Vite, Lucide Icons

---

## Development Setup

### Prerequisites
- [Rust](https://rustup.rs/) (1.78+)
- [Bun](https://bun.sh/) (or Node.js / pnpm)

### Running Locally
```bash
# 1. Install frontend dependencies
bun install

# 2. Start the Tauri development environment
bun run tauri dev
```

### Production Build
```bash
bun run tauri build
```

---

## License

MIT License
