<p align="center"><img src="website/assets/icon.png" width="96" alt="OpenGlaido icon"></p>

# OpenGlaido

**Talk instead of type.** Hold a shortcut, speak, let go. Your words appear at your cursor.

Open-source dictation for macOS, Windows 11 and Linux, inspired by [Glaido](https://glaido.com) and [VoiceInk](https://tryvoiceink.com). No app subscription: use your own cloud account or, on a Mac, a downloaded model.

**[Download the latest release](https://github.com/Legoless/OpenGlaido/releases/latest)** · [Website](https://openglaido.com) · [Report a problem](https://github.com/Legoless/OpenGlaido/issues)

<p align="center"><img src="docs/screenshots/home.png" width="880" alt="OpenGlaido Home: time saved, dictation speed, day streak and today's dictations in Slack, Mail, Safari, Notes and Messages"><br><sub>Screenshots show demo data.</sub></p>

## Get started

### 1. Install

| Computer | Download | Requirements |
| --- | --- | --- |
| Mac | `.dmg` | Apple Silicon, macOS 11 (Big Sur) or later |
| Windows | `-setup.exe` or `.msi` | Windows 11 x64 |
| Linux | `.AppImage` or `.deb` | x64, an X11 session, glibc 2.39+ (Ubuntu 24.04+, Debian 13+, Fedora 40+, current Arch) |

- **Mac:** drag OpenGlaido to Applications. Signed and notarized.
- **Windows:** not Authenticode-signed yet. If SmartScreen says "Windows protected your PC", choose **More info › Run anyway**.
- **Linux:** `chmod +x OpenGlaido_*.AppImage` and run it, or `sudo apt install ./OpenGlaido_*_amd64.deb`.
- **Updates:** signed; installed only when the app is idle with its windows closed.

### 2. Finish setting up

Home's **Finish setting up OpenGlaido** card lists what's missing.

- **API key:** new installs use Groq. Add your key in **Settings › Model** before recording, or [choose another model](#choose-a-model).
- **Mac:** allow **Accessibility** (for shortcuts and pasting) and the **microphone** when asked. Windows and Linux ask for no permissions.
- **Mac tip:** in **System Settings › Keyboard**, set "Press globe key to" **Do Nothing**, so fn doesn't open the emoji picker.

### 3. Dictate

| Action | Mac | Windows | Linux |
| --- | --- | --- | --- |
| **Dictate** (hold) | fn | Ctrl+Win | Ctrl+Super+G |
| **Hands-free** (press to start and stop) | fn+Space | Ctrl+Win+Shift | Ctrl+Shift+Super+G |
| **Cancel** | Esc | Esc | Esc |
| **Commands** (beta, hold) | Right Option | Left Ctrl+Left Alt | Ctrl+Super+A |
| **Commands** (beta, hands-free) | Right Option+Space | Left Ctrl+Left Alt+Shift | Ctrl+Shift+Super+A |

- Change shortcuts, or turn on **Enter to stop and paste**, in **Settings › Hotkeys**.
- Shortcut taken? Linux leaves it inactive and warns on Home; a Mac warns and takes it over.

## What happens when you dictate

<p align="center"><img src="docs/screenshots/dictation-bar.webp" width="520" alt="The dictation bar while recording: bars that move with your voice"></p>

- The **dictation bar** shows while recording and processing, without taking focus.
- On a Mac, its **×** cancels hands-free recordings and processing.
- On stop: noise removal, transcription, snippets, AI cleanup, style and dictionary, then paste.
- Recordings with no speech are dropped.
- Your clipboard text returns after pasting, unless **Copy to clipboard** is on.
- Switching windows doesn't stop dictation, unless **Cancel when focus changes** is on (Mac and Windows).
- Recordings with speech go to History first, so you can **Retry** failures.

## Choose a model

- **Transcription:** voice to text.
- **Language model** (on by default): fixes capitals and punctuation, removes "um", answers Commands. **Off** skips cleanup and Commands.

<p align="center"><img src="docs/screenshots/model.png" width="720" alt="Settings › Model: transcription in the cloud with OpenAI GPT Live Transcribe and a saved API key, and the language model set to On this Mac"></p>

| | Cloud service or your own server | On this Mac (downloaded model) |
| --- | --- | --- |
| **Transcription** | Groq (default), OpenAI, ElevenLabs, Microsoft Azure, OpenRouter, Mistral, Custom | Whisper (Large v3 Turbo (quantized) recommended), Parakeet Ultra, ARK-ASR, Qwen3-ASR, Cohere Transcribe, Voxtral Realtime, Microsoft VibeVoice ASR (BitNet, full and Streaming 7B), Phi-4 Multimodal |
| **Language model** | Groq (default), OpenAI, OpenRouter, Mistral, Google Gemini, Ollama, LM Studio, Custom | Gemma 4 E2B Instruct (recommended), Qwen3 4B Instruct 2507, Ministral 3 3B Instruct |

- Downloaded models are Mac only (some need macOS 12+ or 14+). On Windows and Linux, use the cloud, or Ollama, LM Studio or Custom on your computer.
- **Custom** takes any OpenAI-compatible endpoint URL.
- **Live models** stream your raw audio while you speak, before noise removal; only the final text is pasted.

Details: [local transcription models](docs/local-transcription-models.md), [Microsoft models, including Azure setup](docs/microsoft-transcription-models.md).

## Features

<p align="center"><img src="docs/screenshots/formatting.png" width="880" alt="The Formatting page: the All apps style, the built-in Email rule and custom rules for Slack, github.com, Terminal and Notes"></p>

- **Formatting:** a Standard, Casual or Lowercase style, an optional custom prompt (up to 500 characters), and **Raw text** to skip AI cleanup.
- **Rules:** the built-in **Email** rule (on by default for Gmail and Outlook.com on the web), plus your own style and prompt per app or website.
- **Dictionary:** your terms become speech-model hints where supported. **Replace with** fixes spellings locally.
- **Snippets:** a trigger like "my email", spoken alone, pastes saved text.
- **History:** play, copy, delete or **Retry** dictations. **Cmd+K** or Ctrl+K searches.
- **Commands (beta):** enable beta features in **Settings › General**, then ask aloud or start a dictation with "Glaido, …". Web search and deep research need Brave, Tavily or SearXNG.
- **Custom tools:** local MCP servers for Commands. Tools, built-in or custom, need a cloud language model; with a model on this Mac, Commands answer without tools.
- **More:** remembered microphones, 14 app languages, sounds, themes, launch at login.

## Privacy and cost

- **Cost:** pay cloud providers directly. Mac models have no per-request fees. Dictionary hints add 20% to ElevenLabs costs.
- **On your computer:** settings, history (kept until deleted), dictionary, snippets and recordings. API keys go in the system keychain.
- **Sent to the cloud:** transcription gets your recording and dictionary hints. A cloud language model gets the transcript, style and prompt; Commands add your request, app name, selected text (Mac) and what tools read.
- Web searches go only to your search provider; reading a page or YouTube video fetches it from that site. Models come from Hugging Face; update checks go to GitHub.
- **Fully offline on a Mac:** local transcription, with the language model **On this Mac** or **Off**.
- No analytics or telemetry code.

## Known limitations

- No Intel Mac, Windows ARM or Linux ARM builds.
- On Windows, rules list only apps you recently dictated into, and Gmail and Outlook are the only websites recognized.
- Linux can't see the front app: All apps style only, no app names in History.
- Linux is X11 only. On Wayland, shortcuts, pasting into Wayland apps and dictation-bar placement don't work.
- On Linux, shortcuts need a non-modifier key, **Mute system audio** does nothing yet, and API keys need a running Secret Service.
- Only text is restored to the clipboard; a copied image or file is lost.

## Development Setup

- **All:** Git, [Bun](https://bun.sh/) (not Node.js or pnpm), latest stable [Rust](https://rustup.rs/).
- **macOS:** Xcode Command Line Tools, [CMake](https://cmake.org/), python3, plus [uv](https://docs.astral.sh/uv/) on Apple Silicon.
- **Windows:** native x64, Visual Studio C++ build tools, Windows SDK, WebView2, PowerShell.
- **Linux (Ubuntu 24.04):**

```bash
sudo apt-get install -y build-essential curl wget file libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev patchelf libssl-dev libasound2-dev libxkbcommon-dev libdbus-1-dev pkg-config
```

```bash
git clone https://github.com/Legoless/OpenGlaido.git && cd OpenGlaido
bun install
bun run tauri dev     # development
bun run tauri build   # local, unsigned build
```

Before a pull request, run the CI checks (on Windows, also `$env:RUNNER_TEMP = $env:TEMP; ./scripts/check-windows-tests.ps1` in PowerShell from the repository root):

```bash
bun install --frozen-lockfile && bun test scripts && bun run build && bun run check:locales
cd src-tauri && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked
```

- Releases are built and signed locally, never in CI: [docs/releases.md](docs/releases.md).
- Architecture and rules: [AGENTS.md](AGENTS.md). Model helpers: [native-stt](native-stt/README.md), [native-vibe](native-vibe/README.md), [native-microsoft](native-microsoft/README.md).

## Contributing

- Report bugs and ideas in [GitHub Issues](https://github.com/Legoless/OpenGlaido/issues).
- Focused pull requests for code, docs or translations (`src/locales/`) are welcome.
- The app's "Search documentation" command reads this README and AGENTS.md, so keep both accurate.

## License

[MIT License](LICENSE).
