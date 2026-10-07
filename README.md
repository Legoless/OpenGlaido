<p align="center"><img src="website/assets/icon.png" width="96" alt="OpenGlaido icon"></p>

# OpenGlaido

**OpenGlaido lets you talk instead of type.** Hold a shortcut, say what you want to write, and let go. Your words appear as text at your cursor, in an email, a chat or a document. It is an open-source dictation app for macOS, Windows 11 and Linux, inspired by [Glaido](https://glaido.com) and [VoiceInk](https://tryvoiceink.com).

There is no app subscription. You use your own account with a cloud service, or on a Mac you can download a model that runs on your computer.

**[Download the latest release](https://github.com/Legoless/OpenGlaido/releases/latest)** · [Website](https://openglaido.com) · [Report a problem](https://github.com/Legoless/OpenGlaido/issues)

<p align="center"><img src="docs/screenshots/home.png" width="880" alt="OpenGlaido Home: time saved, dictation speed, day streak and today's dictations in Slack, Mail, Safari, Notes and Messages"><br><sub>Screenshots show demo data.</sub></p>

## Get started

### 1. Install

| Computer | File to download | Requirements |
| --- | --- | --- |
| Mac | `.dmg` | Apple Silicon, macOS 11 (Big Sur) or later |
| Windows | `-setup.exe` or `.msi` | Windows 11 x64 |
| Linux | `.AppImage` or `.deb` | x64, an X11 session, glibc 2.39 or newer (Ubuntu 24.04+, Debian 13+, Fedora 40+, current Arch) |

- **Mac:** open the `.dmg` and drag OpenGlaido into Applications. The app is signed and notarized, so macOS opens it normally.
- **Windows:** run either installer. It isn't signed with a Windows publisher certificate (Authenticode) yet, so SmartScreen may show "Windows protected your PC". Choose **More info › Run anyway**.
- **Linux:** run `chmod +x OpenGlaido_*.AppImage` and start the AppImage, or install the package with `sudo apt install ./OpenGlaido_*_amd64.deb`.
- **Updates:** OpenGlaido checks for signed updates 1 minute after launch and then every 6 hours, or use **Check for updates** in **Settings › General**. An update installs only when nothing is in progress and the app's windows are closed. Updating the Linux `.deb` asks for your password.

### 2. Finish setting up

Open OpenGlaido. Home shows a **Finish setting up OpenGlaido** card that lists anything still missing.

- **Add an API key.** New installs use Groq, a cloud service, to turn speech into text and to clean it up, so Home asks you to **Add your Groq API key**. An API key is a password from the provider that lets the app use your account. Paste it in **Settings › Model**, or use the **Get an API key** link. You can also pick another provider or, on a Mac, download a model instead (see [Choose a model](#choose-a-model)). You can't record until transcription is set up.
- **Mac: allow Accessibility** when macOS asks at launch, or click **Open Accessibility settings** on Home. OpenGlaido needs it to notice your shortcut and to paste text.
- **Mac: allow the microphone** the first time you dictate, or click **Allow access** on the card.
- **Mac tip:** in **System Settings › Keyboard**, set the "Press globe key to" option to **Do Nothing**, so the fn shortcut doesn't open the emoji picker.

Windows and Linux ask for no permissions. Closing the main window keeps OpenGlaido running in the menu bar (Mac) or system tray (Windows and Linux).

### 3. Dictate

Click into any text field. Hold the shortcut, speak, and let go. Or press the hands-free shortcut once to start and again to stop.

| Action | Mac | Windows | Linux |
| --- | --- | --- | --- |
| **Dictate:** hold, speak, let go | fn | Ctrl+Win | Ctrl+Super+G |
| **Hands-free:** press to start, again to stop | fn+Space | Ctrl+Win+Shift | Ctrl+Shift+Super+G |
| **Cancel** a recording | Esc | Esc | Esc |
| **Commands** (beta): hold | Right Option | Left Ctrl+Left Alt | Ctrl+Super+A |
| **Commands** (beta): hands-free | Right Option+Space | Left Ctrl+Left Alt+Shift | Ctrl+Shift+Super+A |

Change shortcuts in **Settings › Hotkeys**. There you can also turn on **Enter to stop and paste** for hands-free dictation. On Linux the app shows the Super key as "Win", so Ctrl+Super+G appears as Ctrl Win G. If another app already uses a shortcut on Linux, it stays assigned but inactive, and Home shows a warning so you can pick another. On a Mac, Settings › Hotkeys warns you instead, and OpenGlaido takes the shortcut over from the other app.

## What happens when you dictate

<p align="center"><img src="docs/screenshots/dictation-bar.webp" width="520" alt="The dictation bar while recording: bars that move with your voice"></p>

A small **dictation bar** floats on screen while you speak and while your text is processed. It never takes focus. On a Mac, click its **×** to cancel a hands-free recording or text that is still being processed. Windows and Linux don't show the ×; press Esc while recording. Set its height with **Dictation bar location** in **Settings › Dictation** (Bottom, Raised or High).

When you stop, OpenGlaido first removes background noise and quietly drops recordings with no speech. (Live models receive your raw audio while you speak, before noise removal, but a recording without speech is still dropped.) Then it turns your speech into text (transcription), applies your snippets, AI cleanup, style and dictionary, then pastes the result at your cursor. About a second later your previous clipboard text comes back, unless you turn on **Copy to clipboard** in **Settings › Dictation**.

- **Switching windows** doesn't stop dictation. The text goes into whichever field has focus when it is ready. Turn on **Cancel when focus changes** (Mac and Windows) to cancel instead.
- **Nothing is lost.** Every recording with speech is saved to History before it is transcribed. If something fails, press **Retry**.

## Choose a model

**Settings › Model** has two parts:

- **Transcription** turns your voice into text.
- **Language model** does the *AI cleanup*. It uses a language model, a text AI, to fix capitals and punctuation and remove filler sounds like "um", while keeping your words and meaning. It also answers Commands. It is on by default; set it to **Off** to skip AI cleanup (Commands then don't work).

<p align="center"><img src="docs/screenshots/model.png" width="720" alt="Settings › Model: transcription in the cloud with OpenAI GPT Live Transcribe and a saved API key, and the language model set to On this Mac"></p>

| | Cloud service or your own server | On this Mac (downloaded model) |
| --- | --- | --- |
| **Transcription** | Groq (default), OpenAI, ElevenLabs, Microsoft Azure, OpenRouter, Mistral, Custom | Whisper (10 variants, Large v3 Turbo (quantized) recommended), Parakeet Ultra, ARK-ASR, Qwen3-ASR, Cohere Transcribe, Voxtral Realtime, Microsoft VibeVoice ASR (BitNet, full and Streaming 7B), Phi-4 Multimodal |
| **Language model** | Groq (default), OpenAI, OpenRouter, Mistral, Google Gemini, Ollama, LM Studio, Custom | Gemma 4 E2B Instruct (recommended), Qwen3 4B Instruct 2507, Ministral 3 3B Instruct |

- Downloaded models are Mac only. On Windows and Linux, use a cloud provider, or Ollama, LM Studio or a Custom endpoint running on your own computer. Mac models are optional downloads, verified with a checksum.
- **Custom** works with any OpenAI-compatible endpoint URL. Ollama and LM Studio need no key.
- Most models transcribe after you stop. **Live models** stream audio while you speak, but paste only the final text when you stop: OpenAI GPT Live Transcribe, the real-time versions of ElevenLabs Scribe v2 and Microsoft MAI Transcribe 2, and VibeVoice ASR Streaming 7B on a Mac.
- **Microsoft Azure** (MAI Transcribe 2, public preview) needs an Azure resource URL and key. Real time also needs a Foundry deployment of MAI-Transcribe-2-Streaming; leave the deployment name blank if it uses that name.
- **Mac versions:** Whisper runs on macOS 11. Parakeet Ultra, ARK-ASR, Qwen3-ASR, Cohere Transcribe, Voxtral Realtime and VibeVoice ASR BitNet need macOS 12+. VibeVoice ASR, VibeVoice ASR Streaming 7B and Phi-4 Multimodal need macOS 14+.
- With a local language model, Commands answer without tools.

Details: [local transcription models](docs/local-transcription-models.md) and [Microsoft models, including Azure setup](docs/microsoft-transcription-models.md).

## Features

<p align="center"><img src="docs/screenshots/formatting.png" width="880" alt="The Formatting page: the All apps style, the built-in Email rule and custom rules for Slack, github.com, Terminal and Notes"></p>

- **Formatting.** **All apps** sets one style: Standard (full capitals and punctuation, the default), Casual (capitals stay, fewer full stops) or Lowercase (no capitals, minimal punctuation). **Custom prompt** adds up to 500 characters of your own instructions, like "Use British English spelling." **Raw text** gives you exactly what you said with no AI cleanup; your style still applies.
- **Email rule.** On by default. In Gmail and Outlook.com on the web (add outlook.office.com for work or school Outlook), it writes your words as an email: greeting on its own line, short paragraphs, a polite tone and nothing made up. Add your mail apps to it, or switch it off.
- **Custom rules.** Give an app or website (like github.com) its own style, Raw text setting and prompt. For a browser, add the website.
- **Dictionary.** Teach it names and jargon. Words are passed to the speech model as hints where it supports them. **Replace with** fixes spellings on your computer, even with AI cleanup off.
- **Snippets.** Say a short trigger such as "my email" as the whole dictation, and OpenGlaido pastes your saved text instead. Snippets can include `{date}`, `{time}` and `{clipboard}`.
- **History.** **Home › Activity** lists every dictation and the app you started it in. Play the audio, copy, search, delete, or **Retry** with your current settings. **Cmd+K** (Ctrl+K on Windows and Linux) searches your transcriptions and offers quick actions. Home also shows time saved, dictation speed and your day streak.
- **Commands (beta).** Off by default; turn on **Settings › General › Enable beta features**. Ask a question out loud with the Commands shortcut, or start any dictation with "Glaido, …". The answer appears in a floating window where you can refine, paste or copy it. There are 8 built-in commands, such as web search, reading a page, deep research and searching your history. Web search and deep research need a search provider (Brave, Tavily or SearXNG).
- **Custom tools.** Add your own tools for Commands as local MCP servers: small programs on your computer that give Commands new abilities. Set each tool to Auto, Ask or Deny. Tools need a cloud language model.
- **Microphone menu.** The menu bar or tray menu has **Microphone**, **Show OpenGlaido** and **Quit**. Every mic you pick is remembered and used again when you plug it back in. **System Default** forgets them all.
- **Languages.** The app comes in 14 languages. Dictate in one language, several, or any (auto-detect) in **Settings › Dictation › Language**.
- **Comfort.** Start and stop sounds, muting your speakers while you record (Mac and Windows), Dark, Light or System theme, and launch at login.

## Privacy and cost

- **Cost.** There is no app subscription. You pay cloud providers directly for what you use. Models on your Mac have no per-request fees. Dictionary hints add 20% to ElevenLabs costs.
- **Stored on your computer.** Settings, history, dictionary, snippets and a copy of every saved recording live in the app's data folder (`~/Library/Application Support/com.openglaido.app` on a Mac, `%APPDATA%\com.openglaido.app` on Windows, `~/.local/share/com.openglaido.app` on Linux). History is kept until you delete it. API keys go in the system keychain (macOS Keychain, Windows Credential Manager or Linux Secret Service), never in the settings file.
- **Sent to the cloud.** A cloud transcription provider gets your recording and your dictionary words as hints. A cloud language model gets the transcript, your style and your custom prompt. Commands send your request, the name of the app you're in, selected text (Mac) and whatever their tools read (web pages, files you ask about, matching history) to the language model. Web searches go only to a search provider you set up; reading a page or a YouTube video fetches it directly from that site. Models download from Hugging Face, and update checks go to GitHub.
- **Fully offline on a Mac.** Use a local transcription model and set the language model to **On this Mac** or **Off**. A local speech model alone keeps your audio on the Mac, but a cloud language model still receives the text.
- OpenGlaido contains no analytics or telemetry code.

## Known limitations

- On-device models are Mac only. There are no Intel Mac, Windows ARM or Linux ARM builds.
- Per-app and per-website rules work fully on macOS. On Windows, the app list shows only apps you have dictated into recently, and Gmail and Outlook are the only websites recognized.
- On Linux, the app can't tell which app is in front. Only the All apps style applies, History shows no app names, and **Cancel when focus changes** doesn't work.
- Linux works on X11 only: on Wayland, shortcuts, pasting into Wayland apps and dictation-bar placement don't work. Shortcuts need one normal key, not just modifiers, and left and right modifiers count as the same key. **Mute system audio** does nothing yet. Saving API keys needs a running Secret Service.
- Selected text reaches Commands only on a Mac.
- Only text is restored to the clipboard after pasting. An image or file you had copied is lost.

## Development Setup

**All platforms:** Git, [Bun](https://bun.sh/) (required; Node.js or pnpm can't replace it) and the latest stable [Rust](https://rustup.rs/).

- **macOS:** Xcode Command Line Tools, [CMake](https://cmake.org/) and python3. On Apple Silicon, also [uv](https://docs.astral.sh/uv/), which builds the bundled Microsoft speech runtime (app users need no Python). The first `dev` or `build` downloads pinned, checksum-verified sources and builds whisper.cpp, llama.cpp and two native speech helpers.
- **Windows:** a native Windows x64 machine with Visual Studio's C++ build tools and Windows SDK, WebView2 and PowerShell.
- **Linux (Ubuntu 24.04):** `sudo apt-get install -y build-essential curl wget file libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev patchelf libssl-dev libasound2-dev libxkbcommon-dev libdbus-1-dev pkg-config`

```bash
git clone https://github.com/Legoless/OpenGlaido.git && cd OpenGlaido
bun install
bun run tauri dev     # run the app in development (Vite on localhost:1420)
bun run tauri build   # local, unsigned build
```

Run the same checks as CI before opening a pull request (on Windows, also run `$env:RUNNER_TEMP = $env:TEMP; ./scripts/check-windows-tests.ps1` in PowerShell from the repository root):

```bash
bun install --frozen-lockfile && bun test scripts && bun run build && bun run check:locales
cd src-tauri && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked
```

Signed, self-updating releases come from `bun run release:build` and are built and signed locally, never in CI. See [docs/releases.md](docs/releases.md). For architecture and project rules, read [AGENTS.md](AGENTS.md). Local model helpers are documented in [native-stt](native-stt/README.md), [native-vibe](native-vibe/README.md) and [native-microsoft](native-microsoft/README.md).

## Contributing

Report bugs and ideas in [GitHub Issues](https://github.com/Legoless/OpenGlaido/issues). Pull requests for code, docs or translations (`src/locales/`) are welcome; keep them focused and run the checks above. The app's "Search documentation" command answers from this README and AGENTS.md, so please keep both accurate.

## License

OpenGlaido is released under the [MIT License](LICENSE).
