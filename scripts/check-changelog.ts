import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { requireVersionEntry, validateChangelog } from "../src/changelog";
import { releaseVersion } from "./release";

export function checkChangelog(data: unknown, tauri: string, cargo: string, npm: string, release = false) {
  const version = releaseVersion(tauri, cargo, npm);
  return requireVersionEntry(validateChangelog(data), version, release);
}

function main() {
  const args = process.argv.slice(2);
  if (args.length === 1 && args[0] === "--help") {
    console.log("Usage: bun run check:changelog [--release]\nValidates the changelog and matching app versions. --release also requires an empty Unreleased list.");
    return;
  }
  if (args.length > 1 || (args.length === 1 && args[0] !== "--release")) {
    throw new Error("Usage: bun run check:changelog [--release]");
  }
  const root = resolve(import.meta.dir, "..");
  const config = JSON.parse(readFileSync(join(root, "src-tauri/tauri.conf.json"), "utf8"));
  const cargo = Bun.TOML.parse(readFileSync(join(root, "src-tauri/Cargo.toml"), "utf8")) as { package: { version: string } };
  const npm = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  const entry = checkChangelog(JSON.parse(readFileSync(join(root, "changelog.json"), "utf8")), config.version, cargo.package.version, npm.version, args.includes("--release"));
  console.log(`Changelog valid for OpenGlaido ${entry.version}`);
}

if (import.meta.main) {
  try { main(); }
  catch (error) {
    console.error(error instanceof Error ? error.message : "Changelog validation failed");
    process.exitCode = 1;
  }
}
