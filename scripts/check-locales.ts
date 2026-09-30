// Fails when a locale file misses a string the UI translates. `--keys` prints the key list.
// Keys: every t("…") literal in src/ plus src/locales/extra.json (backend messages, tool metadata).
import { readdirSync, readFileSync, statSync } from "fs";
import { join } from "path";

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    return statSync(p).isDirectory() ? walk(p) : [p];
  });
}

const keys = new Set<string>(JSON.parse(readFileSync("src/locales/extra.json", "utf8")));
for (const file of walk("src").filter((f) => /\.tsx?$/.test(f))) {
  for (const m of readFileSync(file, "utf8").matchAll(/\bt\(\s*"((?:[^"\\]|\\.)*)"/g)) keys.add(JSON.parse(`"${m[1]}"`));
}

if (process.argv.includes("--keys")) {
  console.log(JSON.stringify([...keys].sort(), null, 1));
  process.exit(0);
}

let failed = false;
for (const file of readdirSync("src/locales").filter((f) => f.endsWith(".json") && f !== "extra.json")) {
  const dict: Record<string, string> = JSON.parse(readFileSync(join("src/locales", file), "utf8"));
  const missing = [...keys].filter((k) => !dict[k]?.trim());
  const placeholders = [...keys].filter((k) => dict[k] && [...k.matchAll(/\{(\w+)\}/g)].some((m) => !dict[k].includes(m[0])));
  if (missing.length || placeholders.length) {
    failed = true;
    console.error(`${file}: ${missing.length} missing, ${placeholders.length} with broken {placeholders}`);
    for (const k of [...missing, ...placeholders].slice(0, 10)) console.error(`  ${JSON.stringify(k)}`);
  }
}
console.log(`${keys.size} keys checked`);
process.exit(failed ? 1 : 0);
