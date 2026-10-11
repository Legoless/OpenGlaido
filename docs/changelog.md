# Changelog

OpenGlaido keeps its user-facing release history in the root `changelog.json`.
That file supplies the bundled Settings › General viewer, updater manifest notes,
and GitHub draft release notes. Reading the history requires no network access.

## Content and display

- `unreleased` contains plain-English bullets for internal release authoring and
  is never displayed in the app.
- `releases` contains a version, UTC publication date (`YYYY-MM-DD`), and nonempty
  change bullets for each published or prepared release, newest version first.
- Settings › General opens a scrollable changelog dialog showing only the
  installed version and earlier versioned entries, newest first. The installed
  version's entry is marked as current. Draft changes and entries newer than the
  installed version are never shown, including in local and development builds.
- Interface labels and dates follow the app language. Authored release notes stay
  in English, matching the published release notes.
- The initial history summarizes the published 0.1.0, 0.2.0, and 2.1.0 notes.
  Changes after 2.1.0, including the recording-start fix, belong under Unreleased.

## Authoring and release rules

Add a concise Unreleased bullet for every user-visible feature or fix. Internal
refactoring and test-only changes do not need a bullet. Keep published entries
unchanged except for factual corrections.

Keep draft changes internal to release authoring. Never render `unreleased` or
notes for versions newer than the installed app in the changelog viewer.

Before a release, move all draft bullets into an entry for the new version, set
its publication date, clear `unreleased`, and update the existing package, Cargo,
and Tauri version fields together. Do not introduce a separate build counter.

`bun run check:changelog` validates the schema, real calendar dates, unique stable
versions, newest-first ordering, and meaningful nonempty bullets. It also requires
an entry matching the application version and runs with the frontend build.

The official release check, build, and upload additionally require an empty draft
list. They fail before publishing when the version's entry is missing or draft
changes remain. Verification of already built packages can run with draft changes
present. Release notes are generated from the selected version's entry; package
provenance is appended separately to GitHub draft notes.

## Validation

Tests cover malformed or placeholder entries, impossible dates, duplicate and
out-of-order versions, missing release entries, uncleared drafts, note generation,
and the viewer's exclusion of drafts and newer versions, ordering,
current-version marking, and safe text rendering.
The viewer also needs a browser or built-app check for opening, scrolling, closing,
and fitting long entries. Existing release-asset and signature checks stay active.

For the browser check, run `bun run dev`, open `/scripts/changelog-check.html`,
and execute `await runChangelogChecks()`. Run it at both the default app size and
760 × 540, the smallest main-window size. It exercises the actual dialog inside
Settings, focus restoration, Escape and close actions, long text, and scrolling.
