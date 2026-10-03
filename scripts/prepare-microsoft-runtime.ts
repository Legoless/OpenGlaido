// Windows cloud builds do not need Python or the macOS-only local runtime.
if (process.platform === "darwin") {
  const result = Bun.spawnSync(["python3", "scripts/build-microsoft-runtime.py"], { stdout: "inherit", stderr: "inherit" });
  process.exit(result.exitCode);
}
