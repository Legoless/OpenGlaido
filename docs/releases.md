# Local releases and automatic updates

Build, sign, and notarize on your own computer. GitHub Releases stores only the finished
installers and update manifest. There is no release-build workflow and no signing key needs
to be uploaded to GitHub Actions. The ordinary CI workflow remains for verification.

OpenGlaido reads `https://github.com/Legoless/OpenGlaido/releases/latest/download/latest.json`.
It verifies each update's signature and signed version before installation. Updates install
when recording, transcription, paste, commands, settings saves, and model downloads are idle
and the main/command windows are closed. The first check is after one minute, then every six
hours; Settings → General also has a manual check. Downloaded updates wait up to 30 minutes
for idle. Development builds and unbundled macOS binaries cannot install updates.

## One-time local setup

Install Bun, Rust, Git, GitHub CLI (`gh`), and Minisign on the build/upload computer. macOS
also needs Xcode command-line tools and CMake. Windows builds require native Windows x64,
Visual Studio's C++/Windows SDK tools, WebView2, and PowerShell. Cross-compilation is not used.

1. Sign into GitHub with `gh auth login` for `Legoless/OpenGlaido`.
2. Copy `.release.env.example` to `.release.env.local` and set your local signing paths.
   The `.local` file is gitignored. The current updater key is normally at
   `~/.tauri/openglaido-updater.key`; its public key must match `plugins.updater.pubkey`.
3. On Mac, use the **Developer ID Application** identity already in your Keychain, plus
   `APPLE_API_KEY`, `APPLE_API_ISSUER`, and the local `APPLE_API_KEY_PATH` for notarization.
   No certificate export or `.p12` is needed.
4. Keep a secure offline backup of the updater key. Losing or rotating it breaks updates for
   existing installations. Never commit private keys or local signing settings.

Windows packages need the same updater signing key (on the trusted local build computer).
Windows Authenticode is not configured, so initial installation may show an
unrecognized-publisher/SmartScreen warning. Tauri updater signatures are still mandatory.

## Build and upload

Set the same stable version in `package.json`, `src-tauri/Cargo.toml`, and
`src-tauri/tauri.conf.json`, and update `src-tauri/Cargo.lock` with Cargo. Commit and push the
reviewed source first. The tools never commit, push, or switch branches for you.

```sh
bun run release:check
bun run release:build
bun run release:verify release-artifacts/0.1.0
bun run release:upload release-artifacts/0.1.0
```

Replace `0.1.0` with the version being released. `release:build` runs the checks, builds the
native platform, signs its update package, and verifies it. Mac builds are also Developer ID
signed and notarized. Existing output folders are never overwritten. To retry a local build,
move its old staged folder aside first.

Finished files are staged under the gitignored `release-artifacts/<version>/<platform>/`.
Each build writes `release-build.json` with its source commit, version, platform, and artifact
hashes. Keep that receipt with the packages when copying them between build computers.
Upload verifies those hashes and signatures, requires a clean checkout at the same pushed
commit, and uploads only installer files, signatures, and `latest.json`. It never uploads
signing credentials or invokes CI.

The first draft can contain **macOS Apple Silicon only**. To include Windows, run the local
build command on a Windows x64 computer at the same commit, then copy its
`windows-x86_64/` folder alongside `darwin-aarch64/` before uploading. A complete Windows set
contains both NSIS and MSI installers and their signatures. Once both platforms have public
users, assemble both before publishing a new version so neither disappears from the feed.
Intel Mac, Windows ARM, and Linux releases are not prepared by these commands.

## Publish when ready

Upload creates one **unpublished draft release**. Test the packages and edit its release notes,
including a real upgrade from an older installed version with dictation/commands in progress.
When release is authorized, publish the draft as a stable release and mark it as the latest.
That is the point at which installed clients can receive it automatically.

The repository can remain private during testing. Public clients cannot fetch private assets,
and draft releases never enter the update feed. Make the repository public before launching;
never embed a GitHub token in the app.

An existing release or tag is never overwritten. If an upload fails, its incomplete draft stays
unpublished; inspect and remove only that draft before retrying. Do not publish while an
upload is still running. Corrective releases need a higher version, since automatic downgrades
are refused. Older builds without this updater require one manual installation first.

References: [Tauri updater](https://v2.tauri.app/plugin/updater/),
[macOS signing](https://v2.tauri.app/distribute/sign/macos/).
