import { expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { assertDraft, assertNewRelease, assertSource, readBuilds, releaseManifest, releaseVersion, repositoryApiPath, type BuildReceipt } from "./release";
import { buildPlan } from "./release-build";

test("GitHub repository lookup has no trailing slash while resource paths stay intact", () => {
  expect(repositoryApiPath("")).toBe("repos/Legoless/OpenGlaido");
  expect(repositoryApiPath("git/ref/heads/main")).toBe("repos/Legoless/OpenGlaido/git/ref/heads/main");
});

const version = "0.1.1";
const repository = "Legoless/OpenGlaido";
const commit = "a".repeat(40);
const macAssets = ["OpenGlaido.app.tar.gz", "OpenGlaido.app.tar.gz.sig", `OpenGlaido_${version}_aarch64.dmg`];
const windowsAssets = [`OpenGlaido_${version}_x64-setup.exe`, `OpenGlaido_${version}_x64-setup.exe.sig`, `OpenGlaido_${version}_x64_en-US.msi`, `OpenGlaido_${version}_x64_en-US.msi.sig`];

function withDirectory(check: (directory: string) => void) {
  const directory = mkdtempSync(join(tmpdir(), "openglaido-release-test-"));
  try { check(directory); } finally { rmSync(directory, { recursive: true, force: true }); }
}

function writeAssets(directory: string, names: string[]) {
  mkdirSync(directory, { recursive: true });
  return names.map((name) => {
    const path = join(directory, name);
    writeFileSync(path, name.endsWith(".sig") ? "test-signature\n" : "test-artifact");
    return path;
  });
}

function writeReceipt(directory: string, platform: BuildReceipt["platform"], changes: Partial<BuildReceipt> = {}) {
  const names = platform === "darwin-aarch64" ? macAssets : windowsAssets;
  writeAssets(directory, names);
  const assets = Object.fromEntries(names.map((name) => [name, createHash("sha256").update(name.endsWith(".sig") ? "test-signature\n" : "test-artifact").digest("hex")]));
  const receipt: BuildReceipt = { schema: 1, repository, version, commit, platform, assets, ...changes };
  writeFileSync(join(directory, "release-build.json"), JSON.stringify(receipt));
  return receipt;
}

test("releases require matching stable versions and never reuse a release or tag", () => {
  expect(releaseVersion("0.1.1", "0.1.1", "0.1.1")).toBe("0.1.1");
  for (const invalid of ["0.1.1-beta.1", "v0.1.1", "01.1.1", "0.1"]) {
    expect(() => releaseVersion(invalid, invalid, invalid)).toThrow();
  }
  expect(() => releaseVersion("0.1.1", "0.1.0", "0.1.1")).toThrow();
  expect(() => releaseVersion("0.1.1", "0.1.1", "0.1.0")).toThrow();
  assertNewRelease([], "v0.1.1", false);
  expect(() => assertNewRelease([{ tag_name: "v0.1.1" }], "v0.1.1", false)).toThrow();
  expect(() => assertNewRelease([], "v0.1.1", true)).toThrow();
  const draft = { id: 1, draft: true, tag_name: "v0.1.1", target_commitish: "sha", html_url: "" };
  assertDraft(draft, "0.1.1", "sha");
  for (const changed of [{ draft: false }, { tag_name: "v0.1.0" }, { target_commitish: "other" }]) {
    expect(() => assertDraft({ ...draft, ...changed }, "0.1.1", "sha")).toThrow();
  }
});

test("manifest accepts complete single-platform or combined releases and preserves Windows installer type", () => {
  withDirectory((directory) => {
    const mac = writeAssets(directory, macAssets);
    const windows = writeAssets(directory, windowsAssets);
    const macManifest = releaseManifest(mac, repository, version);
    expect(Object.keys(macManifest.platforms)).toEqual(["darwin-aarch64", "darwin-aarch64-app"]);
    const windowsManifest = releaseManifest(windows, repository, version);
    expect(Object.keys(windowsManifest.platforms)).toEqual(["windows-x86_64", "windows-x86_64-nsis", "windows-x86_64-msi"]);
    const manifest = releaseManifest([...mac, ...windows], repository, version);
    expect(Object.keys(manifest.platforms)).toEqual(["darwin-aarch64", "darwin-aarch64-app", "windows-x86_64", "windows-x86_64-nsis", "windows-x86_64-msi"]);
    expect(manifest.version).toBe(version);
    expect(manifest.platforms["darwin-aarch64"].signature).toBe("test-signature");
    expect(manifest.platforms["windows-x86_64-msi"].url).toBe("https://github.com/Legoless/OpenGlaido/releases/download/v0.1.1/OpenGlaido_0.1.1_x64_en-US.msi");
    expect(manifest.platforms["windows-x86_64"].url).toEndWith("-setup.exe");
  });
});

test("manifest rejects missing packages, duplicate names, unsupported assets and empty signatures", () => {
  withDirectory((directory) => {
    const files = writeAssets(directory, [...macAssets, ...windowsAssets]);
    for (let missing = 0; missing < files.length; missing++) {
      expect(() => releaseManifest(files.filter((_, index) => index !== missing), repository, version)).toThrow();
    }
    expect(() => releaseManifest([], repository, version)).toThrow();
    expect(() => releaseManifest([...files, files[0]], repository, version)).toThrow();
    expect(() => releaseManifest([...files, join(directory, "other", macAssets[0])], repository, version)).toThrow();
    expect(() => releaseManifest(files, repository, "0.1.2")).toThrow();
    expect(() => releaseManifest(files, "invalid/repo/path", version)).toThrow();
    for (const unexpected of ["OpenGlaido_0.1.1_x64.dmg", "OpenGlaido_0.1.1_arm64-setup.exe", "extra.txt"]) {
      expect(() => releaseManifest([...files, join(directory, unexpected)], repository, version)).toThrow();
    }
    for (const signature of files.filter((file) => file.endsWith(".sig"))) {
      writeFileSync(signature, " \n");
      expect(() => releaseManifest(files, repository, version)).toThrow();
      writeFileSync(signature, "test-signature\n");
    }
  });
});

test("receipts verify complete macOS, Windows and combined local builds", () => {
  withDirectory((directory) => {
    const macDirectory = join(directory, "darwin-aarch64");
    const mac = writeReceipt(macDirectory, "darwin-aarch64");
    const build = readBuilds(directory, version);
    expect(build.commit).toBe(commit);
    expect(build.platforms).toEqual(["darwin-aarch64"]);
    expect(Object.fromEntries([...build.assets].map(([name, asset]) => [name, asset.hash]))).toEqual(mac.assets);
    expect(build.assets.get(macAssets[0])?.path).toBe(join(macDirectory, macAssets[0]));

    const windowsDirectory = join(directory, "windows-x86_64");
    writeReceipt(windowsDirectory, "windows-x86_64");
    expect(readBuilds(windowsDirectory, version).platforms).toEqual(["windows-x86_64"]);
    const combined = readBuilds(directory, version);
    expect(combined.commit).toBe(commit);
    expect(combined.platforms.sort()).toEqual(["darwin-aarch64", "windows-x86_64"]);
    expect([...combined.assets.keys()].sort()).toEqual([...macAssets, ...windowsAssets].sort());
  });
});

test("receipts reject changed, missing or extra build files", () => {
  withDirectory((directory) => {
    expect(() => readBuilds(directory, version)).toThrow();
    writeReceipt(directory, "darwin-aarch64");
    writeFileSync(join(directory, macAssets[0]), "tampered");
    expect(() => readBuilds(directory, version)).toThrow();
    writeAssets(directory, macAssets);
    rmSync(join(directory, macAssets[0]));
    expect(() => readBuilds(directory, version)).toThrow();
    writeAssets(directory, macAssets);
    writeFileSync(join(directory, "extra.txt"), "extra");
    expect(() => readBuilds(directory, version)).toThrow();
  });
});

test("receipts reject malformed metadata and invalid asset maps", () => {
  withDirectory((directory) => {
    const receipt = writeReceipt(directory, "darwin-aarch64");
    const path = join(directory, "release-build.json");
    for (const changed of [
      { schema: 2 }, { repository: "Other/OpenGlaido" }, { version: "0.1.2" },
      { commit: "short" }, { platform: "linux-x86_64" }, { platform: "windows-x86_64" },
      { assets: null }, { assets: [] }, { assets: {} },
      { assets: { ...receipt.assets, [macAssets[0]]: "invalid-hash" } },
      { assets: { "../escape": "a".repeat(64) } },
    ]) {
      writeFileSync(path, JSON.stringify({ ...receipt, ...changed }));
      expect(() => readBuilds(directory, version)).toThrow();
    }
    for (const malformed of ["{", "null", "[]", "{}"]) {
      writeFileSync(path, malformed);
      expect(() => readBuilds(directory, version)).toThrow();
    }
  });
});

test("receipts reject mixed source commits and duplicate platforms", () => {
  withDirectory((directory) => {
    writeReceipt(join(directory, "mac"), "darwin-aarch64");
    const other = join(directory, "other");
    writeReceipt(other, "windows-x86_64", { commit: "b".repeat(40) });
    expect(() => readBuilds(directory, version)).toThrow();
    rmSync(other, { recursive: true });
    writeReceipt(other, "darwin-aarch64");
    expect(() => readBuilds(directory, version)).toThrow();
  });
});

test("upload requires clean source and identical full local, remote and build commits", () => {
  assertSource(true, commit, commit, commit);
  expect(() => assertSource(false, commit, commit, commit)).toThrow();
  expect(() => assertSource(true, commit, "b".repeat(40), commit)).toThrow();
  expect(() => assertSource(true, commit, commit, "b".repeat(40))).toThrow();
  for (const invalid of ["", "a".repeat(39), "a".repeat(41), "g".repeat(40), "A".repeat(40)]) {
    expect(() => assertSource(true, invalid, invalid, invalid)).toThrow();
  }
});

test("build plans use complete native Apple Silicon and Windows x64 packages", () => {
  expect(buildPlan("darwin", "arm64", version)).toEqual({
    platform: "darwin-aarch64", target: "aarch64-apple-darwin", bundles: "app,dmg",
    assets: ["macos/OpenGlaido.app.tar.gz", "macos/OpenGlaido.app.tar.gz.sig", `dmg/OpenGlaido_${version}_aarch64.dmg`],
  });
  expect(buildPlan("win32", "x64", version)).toEqual({
    platform: "windows-x86_64", target: "x86_64-pc-windows-msvc", bundles: "nsis,msi",
    assets: windowsAssets.map((name) => `${name.includes("-setup.exe") ? "nsis" : "msi"}/${name}`),
  });
  for (const [platform, architecture] of [["darwin", "x64"], ["win32", "arm64"], ["linux", "x64"], ["linux", "arm64"]]) {
    expect(() => buildPlan(platform, architecture, version)).toThrow();
  }
  expect(() => buildPlan("darwin", "arm64", "0.1.1-beta.1")).toThrow();
});
