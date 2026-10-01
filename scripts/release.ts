import { copyFileSync, lstatSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";

const REPOSITORY = "Legoless/OpenGlaido";

export function releaseVersion(tauri: string, cargo: string, npm: string): string {
  if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(tauri)) {
    throw new Error("Release version must be a stable major.minor.patch version");
  }
  if (cargo !== tauri || npm !== tauri) throw new Error("Tauri, Cargo and package.json versions must match");
  return tauri;
}

type Release = { id: number; tag_name: string; target_commitish: string; draft: boolean; html_url: string };

export function assertNewRelease(releases: Pick<Release, "tag_name">[], tag: string, tagExists: boolean) {
  if (releases.some((release) => release.tag_name === tag) || tagExists) {
    throw new Error(`${tag} already has a release or tag; never replace an existing release`);
  }
}

export function assertDraft(release: Release, version: string, sha: string) {
  if (!release.draft || release.tag_name !== `v${version}` || release.target_commitish !== sha) {
    throw new Error("Release is no longer the expected draft; refusing to upload");
  }
}

export function releaseManifest(files: string[], repository: string, version: string) {
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository)) throw new Error("Invalid repository");
  const byName = new Map(files.map((file) => [basename(file), file]));
  if (byName.size !== files.length) throw new Error("Duplicate release asset names");
  const groups = [
    { names: ["OpenGlaido.app.tar.gz", "OpenGlaido.app.tar.gz.sig", `OpenGlaido_${version}_aarch64.dmg`], updates: [["OpenGlaido.app.tar.gz", ["darwin-aarch64", "darwin-aarch64-app"]]] },
    { names: [`OpenGlaido_${version}_x64-setup.exe`, `OpenGlaido_${version}_x64-setup.exe.sig`, `OpenGlaido_${version}_x64_en-US.msi`, `OpenGlaido_${version}_x64_en-US.msi.sig`], updates: [[`OpenGlaido_${version}_x64-setup.exe`, ["windows-x86_64", "windows-x86_64-nsis"]], [`OpenGlaido_${version}_x64_en-US.msi`, ["windows-x86_64-msi"]]] },
  ] as const;
  const platforms: Record<string, { url: string; signature: string }> = {};
  const accepted = new Set<string>();
  for (const group of groups) {
    if (!group.names.some((name) => byName.has(name))) continue;
    if (!group.names.every((name) => byName.has(name))) throw new Error("Incomplete platform packages");
    group.names.forEach((name) => accepted.add(name));
    for (const [name, targets] of group.updates) {
      const signature = readFileSync(byName.get(`${name}.sig`)!, "utf8").trim();
      if (!signature) throw new Error(`Empty signature for ${name}`);
      const value = { url: `https://github.com/${repository}/releases/download/v${version}/${encodeURIComponent(name)}`, signature };
      for (const target of targets) platforms[target] = value;
    }
  }
  if (!accepted.size || files.some((file) => !accepted.has(basename(file)))) throw new Error("Unexpected or unsupported release assets");
  return { version, notes: `OpenGlaido ${version}`, pub_date: new Date().toISOString(), platforms };
}

export type BuildReceipt = {
  schema: 1;
  repository: string;
  version: string;
  commit: string;
  platform: "darwin-aarch64" | "windows-x86_64";
  assets: Record<string, string>;
};

export function assertSource(clean: boolean, localCommit: string, remoteCommit: string, buildCommit: string) {
  if (!clean) throw new Error("Commit the release source first; local test builds must not be uploaded as another commit");
  if (!/^[a-f0-9]{40}$/.test(localCommit) || localCommit !== remoteCommit || localCommit !== buildCommit) {
    throw new Error("Build receipts, local HEAD, and the pushed default branch must be the same commit");
  }
}

export function readBuilds(directory: string, version: string) {
  const files = [...new Bun.Glob("**/*").scanSync({ cwd: directory, onlyFiles: true, absolute: true })].map((file) => resolve(file));
  const receipts = files.filter((file) => basename(file) === "release-build.json");
  if (!receipts.length) throw new Error("No local build receipts; run release:build from clean source first");
  const assets = new Map<string, { path: string; hash: string }>();
  const platforms = new Set<string>();
  let commit = "";
  for (const path of receipts) {
    if (lstatSync(path).isSymbolicLink()) throw new Error("Build receipts cannot be symbolic links");
    const receipt = JSON.parse(readFileSync(path, "utf8")) as BuildReceipt;
    if (receipt.schema !== 1 || receipt.repository !== REPOSITORY || receipt.version !== version
      || !/^[a-f0-9]{40}$/.test(receipt.commit) || !["darwin-aarch64", "windows-x86_64"].includes(receipt.platform)
      || !receipt.assets || Array.isArray(receipt.assets) || typeof receipt.assets !== "object") {
      throw new Error("Invalid or mismatched local build receipt");
    }
    if ((commit && commit !== receipt.commit) || platforms.has(receipt.platform)) throw new Error("Mixed commits or duplicate platform receipts");
    commit = receipt.commit;
    platforms.add(receipt.platform);
    const platformFiles: string[] = [];
    for (const [name, hash] of Object.entries(receipt.assets)) {
      if (!name || /[\\/]/.test(name) || name === "." || name === ".." || !/^[a-f0-9]{64}$/.test(hash) || assets.has(name)) {
        throw new Error("Invalid or duplicate receipt asset");
      }
      const file = join(dirname(path), name);
      if (!files.includes(file) || !lstatSync(file).isFile() || lstatSync(file).isSymbolicLink() || assetHash(file) !== hash) {
        throw new Error(`Build artifact changed or is missing: ${name}`);
      }
      assets.set(name, { path: file, hash });
      platformFiles.push(file);
    }
    const manifest = releaseManifest(platformFiles, REPOSITORY, version);
    if (!(receipt.platform in manifest.platforms) || Object.keys(manifest.platforms).some((key) => !key.startsWith(receipt.platform))) {
      throw new Error("Receipt does not match its platform packages");
    }
  }
  if (files.length !== assets.size + receipts.length) throw new Error("Unexpected files in the release folder; only packages and build receipts are allowed");
  return { commit, assets, platforms: [...platforms] };
}

export function assetHash(path: string) { return createHash("sha256").update(readFileSync(path)).digest("hex"); }

// Verify the actual packages with the public key shipped in the app, before creating any release.
export function verifySignatures(files: string[], pubkey: string, version: string) {
  if (!files.some((file) => file.endsWith(".sig"))) throw new Error("No updater signatures found");
  const directory = mkdtempSync(join(tmpdir(), "openglaido-signatures-"));
  try {
    const key = join(directory, "public.key");
    writeFileSync(key, Buffer.from(pubkey, "base64"));
    for (const file of files.filter((file) => file.endsWith(".sig"))) {
      const signature = join(directory, "signature");
      const decoded = Buffer.from(readFileSync(file, "utf8").trim(), "base64").toString("utf8");
      writeFileSync(signature, decoded);
      const result = Bun.spawnSync(["minisign", "-Vm", file.slice(0, -4), "-p", key, "-x", signature]);
      if (result.exitCode !== 0) throw new Error(`Signature verification failed for ${basename(file)}`);
      // minisign has authenticated this trusted comment as well as the package bytes.
      if (!decoded.split(/\r?\n/)[2]?.split("\t").includes(`version:${version}`)) {
        throw new Error(`Signed version does not match ${version} for ${basename(file)}`);
      }
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

function command(args: string[], input?: unknown, allowMissing = false): string | null {
  const result = Bun.spawnSync(args, { stdin: input === undefined ? undefined : Buffer.from(JSON.stringify(input)), stdout: "pipe", stderr: "pipe" });
  const output = result.stdout.toString();
  if (result.exitCode !== 0) {
    if (allowMissing && result.stderr.toString().includes("HTTP 404")) return null;
    throw new Error(`${args[0]} failed: ${result.stderr.toString().trim()}`);
  }
  return output.trim();
}

export function repositoryApiPath(path: string) { return `repos/${REPOSITORY}${path ? `/${path}` : ""}`; }

function github(path: string, method = "GET", body?: unknown, allowMissing = false) {
  const args = ["gh", "api", "--hostname", "github.com", "--method", method, repositoryApiPath(path)];
  if (body !== undefined) args.push("--input", "-");
  const result = command(args, body, allowMissing);
  return result === null ? null : JSON.parse(result);
}

function sourcePreflight(buildCommit?: string) {
  const sha = command(["git", "rev-parse", "HEAD"])!;
  assertSource(command(["git", "status", "--porcelain"]) === "", sha, sha, buildCommit ?? sha);
  const info = github("");
  const branch = command(["git", "branch", "--show-current"]);
  if (branch !== info.default_branch) throw new Error("Release from the repository's default branch");
  const remote = github(`git/ref/heads/${info.default_branch}`).object.sha;
  assertSource(command(["git", "status", "--porcelain"]) === "", sha, remote, buildCommit ?? sha);
  return sha;
}

function assertUnusedVersion(version: string) {
  const tag = `v${version}`;
  for (let page = 1; ; page++) {
    const releases = github(`releases?per_page=100&page=${page}`);
    assertNewRelease(releases, tag, false);
    if (releases.length < 100) break;
  }
  assertNewRelease([], tag, github(`git/ref/tags/${tag}`, "GET", undefined, true) !== null);
}

async function main() {
  const action = process.argv[2];
  if (!action || action === "--help") {
    console.log("Local releases: check | verify <release-folder> | upload <release-folder>\nBuild with bun run release:build. Upload creates an unpublished GitHub draft using gh auth; it never builds, signs, commits, or publishes.");
    return;
  }
  if (!["check", "verify", "upload"].includes(action)) throw new Error("Unknown release action");
  if (action !== "check" && !process.argv[3]) throw new Error("Specify the locally built release folder");
  const config = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"));
  const cargo = Bun.TOML.parse(readFileSync("src-tauri/Cargo.toml", "utf8")) as { package: { version: string } };
  const version = releaseVersion(config.version, cargo.package.version, JSON.parse(readFileSync("package.json", "utf8")).version);
  if (action === "check") {
    const sha = sourcePreflight();
    assertUnusedVersion(version);
    console.log(`Ready for local release v${version} from ${sha}`);
    return;
  }
  const build = readBuilds(resolve(process.argv[3]), version);
  const directory = mkdtempSync(join(tmpdir(), "openglaido-upload-"));
  try {
    // Freeze finished packages before verification/upload; never read mutable build outputs later.
    const files: string[] = [];
    for (const [name, asset] of build.assets) {
      const copy = join(directory, name);
      copyFileSync(asset.path, copy);
      if (assetHash(copy) !== asset.hash) throw new Error(`Artifact changed while preparing upload: ${name}`);
      files.push(copy);
    }
    const manifest = releaseManifest(files, REPOSITORY, version);
    verifySignatures(files, config.plugins.updater.pubkey, version);
    if (action === "verify") {
      console.log(`Verified v${version}, ${build.commit}, platforms: ${build.platforms.join(", ")}`);
      return;
    }
    const sha = sourcePreflight(build.commit);
    assertUnusedVersion(version);
    const manifestPath = join(directory, "latest.json");
    writeFileSync(manifestPath, JSON.stringify(manifest, null, 2) + "\n");
    const release: Release = github("releases", "POST", {
      tag_name: `v${version}`, target_commitish: sha, name: `OpenGlaido ${version}`,
      body: `Locally built from ${sha}.\n\nIncluded platforms: ${build.platforms.join(", ")}. Test these packages before publishing. macOS packages are Developer ID signed and notarized; Windows packages, when included, have updater signatures but no Authenticode certificate configured.`,
      draft: true, prerelease: false, make_latest: "false",
    });
    for (const file of [...files, manifestPath]) {
      assertDraft(github(`releases/${release.id}`), version, sha);
      command(["gh", "release", "upload", `v${version}`, file, "--repo", `https://github.com/${REPOSITORY}`]);
    }
    assertDraft(github(`releases/${release.id}`), version, sha);
    console.log(`Unpublished draft uploaded: ${release.html_url}`);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  try { await main(); }
  catch (error) {
    console.error(error instanceof Error ? error.message : "Release preparation failed");
    process.exitCode = 1;
  }
}
