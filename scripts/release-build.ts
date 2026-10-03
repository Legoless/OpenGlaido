import { createHash } from "node:crypto";
import { copyFileSync, existsSync, lstatSync, mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { releaseVersion, verifySignatures } from "./release";

export function buildPlan(platform: string, architecture: string, version: string) {
  releaseVersion(version, version, version);
  if (platform === "darwin" && architecture === "arm64") {
    return {
      platform: "darwin-aarch64", target: "aarch64-apple-darwin", bundles: "app,dmg",
      assets: ["macos/OpenGlaido.app.tar.gz", "macos/OpenGlaido.app.tar.gz.sig", `dmg/OpenGlaido_${version}_aarch64.dmg`],
    };
  }
  if (platform === "win32" && architecture === "x64") {
    const nsis = `nsis/OpenGlaido_${version}_x64-setup.exe`;
    const msi = `msi/OpenGlaido_${version}_x64_en-US.msi`;
    return {
      platform: "windows-x86_64", target: "x86_64-pc-windows-msvc", bundles: "nsis,msi",
      assets: [nsis, `${nsis}.sig`, msi, `${msi}.sig`],
    };
  }
  throw new Error("Build on an Apple Silicon Mac or an x64 Windows PC; cross-compilation is not supported");
}

async function main() {
  if (process.argv.length === 3 && process.argv[2] === "--help") {
    console.log(`Usage: bun scripts/release-build.ts

Builds, tests, signs and stages this computer's release packages locally.
Requires a clean committed checkout, Bun, Rust/Clippy and minisign on PATH.
macOS: Apple Silicon, Xcode tools, CMake, Developer ID identity in Keychain,
  APPLE_SIGNING_IDENTITY, APPLE_API_KEY, APPLE_API_ISSUER, APPLE_API_KEY_PATH.
Windows: x64, Visual Studio C++/Windows SDK, WebView2, PowerShell.
Updater key: TAURI_SIGNING_PRIVATE_KEY, or ~/.tauri/openglaido-updater.key.
Set TAURI_SIGNING_PRIVATE_KEY_PASSWORD if the updater key is encrypted.
Outputs: release-artifacts/<version>/<platform>/, including release-build.json.
Existing output directories are never overwritten. Nothing is uploaded.`);
    return;
  }
  if (process.argv.length !== 2) throw new Error("Usage: bun scripts/release-build.ts [--help]");

  const root = realpathSync(resolve(import.meta.dir, ".."));
  process.chdir(root);
  const env = { ...process.env, CARGO_TARGET_DIR: join(root, "src-tauri", "target") };
  // Native builds reuse the normal local debug/release caches.
  delete env.CARGO_BUILD_TARGET;
  // Signing uses the existing local Keychain identity, never an imported CI certificate.
  delete env.APPLE_CERTIFICATE;
  delete env.APPLE_CERTIFICATE_PASSWORD;
  const capture = (command: string[]) => {
    const result = Bun.spawnSync(command, { cwd: root, env, stdout: "pipe", stderr: "pipe" });
    if (result.exitCode !== 0) throw new Error(`${command[0]} ${command[1] ?? ""} failed`);
    return result.stdout.toString().trim();
  };
  const run = async (command: string[]) => {
    console.log(`Running ${command.join(" ")}`);
    const child = Bun.spawn(command, { cwd: root, env, stdin: "inherit", stdout: "inherit", stderr: "inherit" });
    if (await child.exited !== 0) throw new Error(`${command[0]} failed; no release receipt was issued`);
  };
  for (const executable of ["git", "bun", "cargo", "rustc", "minisign"]) {
    if (!Bun.which(executable)) throw new Error(`Required tool is missing from PATH: ${executable}`);
  }
  if (realpathSync(capture(["git", "rev-parse", "--show-toplevel"])) !== root) {
    throw new Error("The release builder must belong to the repository root");
  }
  const commit = capture(["git", "rev-parse", "HEAD^{commit}"]);
  if (!/^[a-f0-9]{40}$/.test(commit)) throw new Error("Expected a committed Git source revision");
  const assertSource = () => {
    if (capture(["git", "status", "--porcelain=v1", "--untracked-files=all", "--ignore-submodules=none"])) {
      throw new Error("Release builds require a clean committed checkout, including untracked files");
    }
    if (capture(["git", "rev-parse", "HEAD^{commit}"]) !== commit) {
      throw new Error("The source commit changed during the build; discard these packages and build again");
    }
  };
  assertSource();
  capture(["git", "ls-files", "--error-unmatch", "package.json", "bun.lock", "src-tauri/Cargo.toml", "src-tauri/Cargo.lock", "src-tauri/tauri.conf.json", "src-tauri/tauri.release.conf.json", "scripts/release-build.ts", "scripts/release.ts"]);
  const config = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"));
  const cargo = Bun.TOML.parse(readFileSync("src-tauri/Cargo.toml", "utf8")) as { package: { version: string } };
  const version = releaseVersion(config.version, cargo.package.version, JSON.parse(readFileSync("package.json", "utf8")).version);
  const plan = buildPlan(process.platform, process.arch, version);
  const host = capture(["rustc", "-vV"]).match(/^host: (.+)$/m)?.[1].trim();
  if (host !== plan.target) throw new Error(`Use a native Rust toolchain for ${plan.target}; cross-compilation is not supported`);
  const output = join(root, "release-artifacts", version, plan.platform);
  if (existsSync(output)) throw new Error("Release output already exists; move it aside before building again");
  if (!config.plugins?.updater?.pubkey) throw new Error("The application updater public key is missing");
  if (!env.TAURI_SIGNING_PRIVATE_KEY) {
    const key = join(homedir(), ".tauri", "openglaido-updater.key");
    if (!existsSync(key)) throw new Error("Set TAURI_SIGNING_PRIVATE_KEY or provide ~/.tauri/openglaido-updater.key");
    env.TAURI_SIGNING_PRIVATE_KEY = key;
  }
  let powershell: string | null = null;
  if (process.platform === "darwin") {
    for (const executable of ["cmake", "xcrun", "codesign", "spctl", "lipo", "security"]) {
      if (!Bun.which(executable)) throw new Error(`Required tool is missing from PATH: ${executable}`);
    }
    for (const name of ["APPLE_SIGNING_IDENTITY", "APPLE_API_KEY", "APPLE_API_ISSUER", "APPLE_API_KEY_PATH"]) {
      if (!env[name]?.trim()) throw new Error(`Missing signing prerequisite: ${name}`);
    }
    if (!env.APPLE_SIGNING_IDENTITY!.startsWith("Developer ID Application:") ||
      !capture(["security", "find-identity", "-v", "-p", "codesigning"]).includes(`"${env.APPLE_SIGNING_IDENTITY}"`)) {
      throw new Error("APPLE_SIGNING_IDENTITY must name an available Developer ID Application identity in Keychain");
    }
    if (!lstatSync(env.APPLE_API_KEY_PATH!).isFile()) throw new Error("APPLE_API_KEY_PATH must point to the local notarization key file");
  } else {
    powershell = Bun.which("pwsh") ?? Bun.which("powershell");
    if (!powershell) throw new Error("PowerShell is required to verify Windows test manifests");
    env.RUNNER_TEMP ??= tmpdir();
  }

  console.log(`Building OpenGlaido ${version} for ${plan.platform} from ${commit}`);
  await run(["bun", "install", "--frozen-lockfile"]);
  await run(["bun", "test", "scripts"]);
  await run(["bun", "run", "check:locales"]);
  await run(["bun", "run", "build"]);
  await run(["cargo", "clippy", "--manifest-path", "src-tauri/Cargo.toml", "--locked", "--all-targets", "--", "-D", "warnings"]);
  if (powershell) await run([powershell, "-NoProfile", "-File", "scripts/check-windows-tests.ps1"]);
  await run(["cargo", "test", "--manifest-path", "src-tauri/Cargo.toml", "--locked"]);
  assertSource();
  const bundle = join(env.CARGO_TARGET_DIR, "release", "bundle");
  // Keep the running .app in place until Tauri refreshes it after compilation.
  for (const asset of plan.assets) rmSync(join(bundle, asset), { force: true });
  await run(["bun", "run", "tauri", "build", "--ci", "--config", "src-tauri/tauri.release.conf.json", "--bundles", plan.bundles, "--", "--locked"]);

  if (process.platform === "darwin") {
    const app = join(bundle, "macos", "OpenGlaido.app");
    await run(["codesign", "--verify", "--deep", "--strict", app]);
    await run(["xcrun", "stapler", "validate", app]);
    await run(["spctl", "--assess", "--type", "execute", "--verbose=4", app]);
    await run(["lipo", "-verify_arch", "arm64", join(app, "Contents", "MacOS", "openglaido")]);
    const speechHelper = join(app, "Contents", "MacOS", "openglaido-stt");
    await run(["lipo", "-verify_arch", "arm64", speechHelper]);
    await run(["codesign", "--verify", "--strict", speechHelper]);
    await run([speechHelper, "--self-test"]);
    const bitnetHelper = join(app, "Contents", "MacOS", "openglaido-vibe");
    await run(["lipo", "-verify_arch", "arm64", bitnetHelper]);
    await run(["codesign", "--verify", "--strict", bitnetHelper]);
    await run([bitnetHelper, "--self-test"]);
    const microsoftHelper = join(app, "Contents", "Resources", "microsoft-runtime", "openglaido-microsoft");
    await run(["lipo", "-verify_arch", "arm64", microsoftHelper]);
    await run(["codesign", "--verify", "--strict", microsoftHelper]);
    await run([microsoftHelper, "--self-test"]);
    await run([microsoftHelper, "--check-runtime"]);
    for (const library of config.bundle.macOS.frameworks as string[]) {
      await run(["lipo", "-verify_arch", "arm64", join(app, "Contents", "Frameworks", basename(library))]);
    }
  }
  assertSource();
  mkdirSync(dirname(output), { recursive: true });
  mkdirSync(output);
  const assets: Record<string, string> = {};
  for (const asset of plan.assets) {
    const source = join(bundle, asset);
    if (!lstatSync(source).isFile() || lstatSync(source).size === 0) throw new Error(`Missing or invalid release asset: ${basename(asset)}`);
    const staged = join(output, basename(asset));
    copyFileSync(source, staged);
    assets[basename(asset)] = createHash("sha256").update(readFileSync(staged)).digest("hex");
  }
  verifySignatures(Object.keys(assets).map((name) => join(output, name)), config.plugins.updater.pubkey, version);
  assertSource();
  writeFileSync(join(output, "release-build.json"), JSON.stringify({ schema: 1, repository: "Legoless/OpenGlaido", version, commit, platform: plan.platform, assets }, null, 2) + "\n", { flag: "wx" });
  console.log(`Verified local release packages: ${output}\nNothing was uploaded.`);
}

if (import.meta.main) {
  try {
    await main();
  } catch (error) {
    console.error(error instanceof Error ? error.message : "Release build failed");
    process.exitCode = 1;
  }
}
