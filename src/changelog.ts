import data from "../changelog.json";

export type ChangelogRelease = { version: string; date: string; changes: string[] };
export type Changelog = { unreleased: string[]; releases: ChangelogRelease[] };

function object(value: unknown, keys: string[], name: string): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`${name} must be an object`);
  const result = value as Record<string, unknown>;
  if (Object.keys(result).some((key) => !keys.includes(key))) throw new Error(`${name} has an unknown field`);
  return result;
}

function versionParts(value: unknown): number[] {
  if (typeof value !== "string" || !/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(value)) {
    throw new Error("Changelog versions must be stable major.minor.patch versions");
  }
  const parts = value.split(".").map(Number);
  if (parts.some((part) => !Number.isSafeInteger(part))) throw new Error("Changelog version is too large");
  return parts;
}

function compareVersions(a: string, b: string): number {
  const left = versionParts(a), right = versionParts(b);
  for (let i = 0; i < left.length; i++) {
    if (left[i] !== right[i]) return left[i] > right[i] ? 1 : -1;
  }
  return 0;
}

function changes(value: unknown, name: string, allowEmpty = false): void {
  if (!Array.isArray(value) || (!allowEmpty && value.length === 0)) throw new Error(`${name} must contain change bullets`);
  const seen = new Set<string>();
  for (const change of value) {
    if (typeof change !== "string" || !change.trim() || change.trim() !== change || /[\r\n]/.test(change)) {
      throw new Error(`${name} must contain nonempty, single-line change bullets`);
    }
    if (!/\p{L}/u.test(change)
      || /^(?:todo|tbd|placeholder)\b|^(?:coming soon|release notes|changes go here|no changes)[.!…]*$/i.test(change)
      || /^(?:OpenGlaido\s+)?v?\d+\.\d+\.\d+[.!…]*$/i.test(change)) {
      throw new Error(`${name} contains placeholder text`);
    }
    if (seen.has(change)) throw new Error(`${name} contains a duplicate change`);
    seen.add(change);
  }
}

/** Validates the authored file before it can be bundled or used for release notes. */
export function validateChangelog(value: unknown): Changelog {
  const root = object(value, ["unreleased", "releases"], "Changelog");
  changes(root.unreleased, "Unreleased", true);
  if (!Array.isArray(root.releases) || root.releases.length === 0) throw new Error("Changelog must contain release entries");
  const seen = new Set<string>();
  let previous: ChangelogRelease | undefined;
  for (const value of root.releases) {
    const entry = object(value, ["version", "date", "changes"], "Release entry");
    versionParts(entry.version);
    const version = entry.version as string;
    if (seen.has(version)) throw new Error(`Duplicate changelog version ${version}`);
    seen.add(version);
    const date = entry.date;
    if (typeof date !== "string" || !/^\d{4}-\d{2}-\d{2}$/.test(date)) throw new Error(`Invalid changelog date for ${version}`);
    const parsed = new Date(`${date}T00:00:00Z`);
    if (!Number.isFinite(parsed.getTime()) || parsed.toISOString().slice(0, 10) !== date) {
      throw new Error(`Invalid changelog date for ${version}`);
    }
    changes(entry.changes, `Release ${version}`);
    if (previous && compareVersions(previous.version, version) <= 0) throw new Error("Changelog versions must be newest first");
    if (previous && previous.date < date) throw new Error("Changelog dates must be newest first");
    previous = entry as ChangelogRelease;
  }
  return root as Changelog;
}

/** Development builds can retain internal drafts; publishing must move them into a versioned entry. */
export function requireVersionEntry(data: Changelog, version: string, release = false): ChangelogRelease {
  versionParts(version);
  const entry = data.releases.find((entry) => entry.version === version);
  if (!entry) throw new Error(`Add a changelog entry for OpenGlaido ${version} before building this version`);
  if (release && data.unreleased.length !== 0) {
    throw new Error("Move Unreleased changes into a new versioned entry and clear the draft before releasing");
  }
  return entry;
}

/** The same Markdown is used in the updater manifest and the GitHub draft release. */
export function formatReleaseNotes(entry: ChangelogRelease): string {
  validateChangelog({ unreleased: [], releases: [entry] });
  return `## OpenGlaido ${entry.version}\n\nReleased ${entry.date}\n\n${entry.changes.map((change) => `- ${change}`).join("\n")}`;
}

export const changelog = validateChangelog(data);
