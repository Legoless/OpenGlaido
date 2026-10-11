import { expect, test } from "bun:test";
import { changelog, formatReleaseNotes, requireVersionEntry, validateChangelog, type Changelog } from "../src/changelog";
import packageInfo from "../package.json";

function fixture(): Changelog {
  return {
    unreleased: [],
    releases: [
      { version: "2.1.0", date: "2026-10-07", changes: ["Keep speech at the beginning of recordings."] },
      { version: "0.2.0", date: "2026-10-07", changes: ["Add Windows and Linux builds."] },
      { version: "0.1.0", date: "2026-10-06", changes: ["Introduce voice dictation on macOS."] },
    ],
  };
}

test("bundled history is valid and covers the application version", () => {
  expect(validateChangelog(changelog)).toEqual(changelog);
  expect(requireVersionEntry(changelog, packageInfo.version).version).toBe(packageInfo.version);
});

test("versions use numeric ordering, allow release gaps, and can share a publication date", () => {
  const value = fixture();
  value.releases.unshift({ version: "10.0.0", date: "2026-10-08", changes: ["Improve microphone startup."] });
  expect(validateChangelog(value)).toEqual(value);
  value.releases.reverse();
  expect(() => validateChangelog(value)).toThrow("newest first");
});

test("malformed shapes, unknown fields, missing history and unstable versions fail", () => {
  for (const value of [null, [], {}, { ...fixture(), typo: [] }, { ...fixture(), releases: [] }]) {
    expect(() => validateChangelog(value)).toThrow();
  }
  for (const version of ["v2.1.0", "2.1", "2.1.0-beta.1", "01.0.0", "2.01.0", "9007199254740992.0.0"]) {
    const value = fixture();
    value.releases[0].version = version;
    expect(() => validateChangelog(value)).toThrow(/version/i);
  }
  expect(() => validateChangelog({ ...fixture(), releases: [null] })).toThrow();
  expect(() => validateChangelog({ ...fixture(), releases: [{ ...fixture().releases[0], notes: "unused" }] })).toThrow("unknown field");
});

test("duplicate versions and backward publication dates fail", () => {
  const duplicate = fixture();
  duplicate.releases.push({ ...duplicate.releases[0] });
  expect(() => validateChangelog(duplicate)).toThrow("Duplicate changelog version");
  const dates = fixture();
  dates.releases[1].date = "2026-10-08";
  expect(() => validateChangelog(dates)).toThrow("dates must be newest first");
});

test("real calendar dates are required, including leap-day validation", () => {
  for (const date of ["", "2026-2-07", "2026-02-29", "2026-04-31", "2026-13-01", "2026-00-01"]) {
    const value = fixture();
    value.releases[0].date = date;
    expect(() => validateChangelog(value)).toThrow("Invalid changelog date");
  }
  const leap = fixture();
  leap.releases.forEach((entry) => { entry.date = "2024-02-29"; });
  expect(validateChangelog(leap)).toEqual(leap);
});

test("release bullets must be nonempty meaningful lines without duplicates", () => {
  const empty = fixture();
  empty.releases[0].changes = [];
  expect(() => validateChangelog(empty)).toThrow();
  for (const changes of [[""], ["  "], [" Improve capture."], ["Improve capture.\nAnother change."], ["TODO: write notes"], ["TODO write release notes"], ["TBD write release notes"], ["TBD"], ["Coming soon."], ["placeholder"], ["OpenGlaido 2.1.0"], ["2.1.0"], ["-"], ["..."], ["Fix capture.", "Fix capture."]]) {
    const value = fixture();
    value.releases[0].changes = changes;
    expect(() => validateChangelog(value)).toThrow();
    expect(() => validateChangelog({ ...fixture(), unreleased: changes })).toThrow();
  }
  expect(() => validateChangelog({ ...fixture(), unreleased: "Fix capture." })).toThrow();
});

test("development permits drafts but release gates require matching entries and cleared drafts", () => {
  const value = fixture();
  expect(requireVersionEntry(value, "2.1.0", true)).toEqual(value.releases[0]);
  value.unreleased.push("Preserve early speech during shortcut confirmation.");
  expect(requireVersionEntry(value, "2.1.0")).toEqual(value.releases[0]);
  expect(() => requireVersionEntry(value, "2.1.0", true)).toThrow("Unreleased");
  expect(() => requireVersionEntry(value, "2.1.1")).toThrow("Add a changelog entry");
  expect(() => requireVersionEntry(value, "2.1.1", true)).toThrow("Add a changelog entry");
});

test("release Markdown uses only the selected published entry, never draft changes", () => {
  const value = fixture();
  value.unreleased.push("Preserve early speech during shortcut confirmation.");
  const notes = formatReleaseNotes(requireVersionEntry(value, "0.2.0"));
  expect(notes).toBe("## OpenGlaido 0.2.0\n\nReleased 2026-10-07\n\n- Add Windows and Linux builds.");
  expect(notes).not.toContain(value.unreleased[0]);
  expect(() => formatReleaseNotes({ ...value.releases[0], changes: [] })).toThrow();
});
