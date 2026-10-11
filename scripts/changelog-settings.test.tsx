import { expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { version } from "../package.json";
import { ChangelogContent, ChangelogModal, ChangelogSettings } from "../src/changelog-settings";
import { changelog } from "../src/changelog";
import type { Changelog } from "../src/changelog";
import { setLocale } from "../src/i18n";

const data: Changelog = {
  unreleased: ["Keep the beginning of your recording."],
  releases: [
    { version: "2.1.0", date: "2026-10-10", changes: ["Use your preferred microphone."] },
    { version: "0.2.0", date: "2026-09-01", changes: ["Browse your dictation history."] },
    { version: "0.1.0", date: "2026-08-01", changes: ["Dictate into any application."] },
  ],
};

test("the bundled changelog is available from a labeled Settings row without an update status", () => {
  const html = renderToStaticMarkup(<ChangelogSettings />);
  expect(html).toContain("Changelog");
  expect(html).toContain("See what’s new in OpenGlaido");
  expect(html).toContain("View changelog</button>");
  expect(html).not.toContain("disabled");
  expect(html).not.toContain("<dialog");
  const bundled = renderToStaticMarkup(<ChangelogContent />);
  expect(bundled).toContain(`Version ${version}`);
  expect(bundled).toContain("Current version");
  expect(bundled).not.toContain("Unreleased");
  for (const draft of changelog.unreleased) expect(bundled).not.toContain(draft);
});

test("draft changes never appear and release history stays newest first", () => {
  const html = renderToStaticMarkup(<ChangelogContent currentVersion="2.1.0" data={data} />);
  expect(html).not.toContain("Unreleased");
  expect(html).not.toContain(data.unreleased[0]);
  expect(html.indexOf("Version 2.1.0")).toBeLessThan(html.indexOf("Version 0.2.0"));
  expect(html.indexOf("Version 0.2.0")).toBeLessThan(html.indexOf("Version 0.1.0"));
  const current = html.match(/<section>[^]*?<\/section>/g)?.find((section) => section.includes("Version 2.1.0"));
  expect(current).toContain("Current version");
  expect(html.match(/Current version/g)).toHaveLength(1);
});

test("newer prepared versions are hidden and unknown versions fail closed", () => {
  const html = renderToStaticMarkup(<ChangelogContent currentVersion="0.2.0" data={data} />);
  expect(html).not.toContain("Version 2.1.0");
  expect(html).not.toContain(data.releases[0].changes[0]);
  expect(html).toContain("Version 0.2.0");
  expect(html).toContain("Version 0.1.0");
  expect(html.match(/Current version/g)).toHaveLength(1);
  for (const currentVersion of ["0.2.0-beta", "2.2.0", "invalid", ""]) {
    const unknown = renderToStaticMarkup(<ChangelogContent currentVersion={currentVersion} data={data} />);
    expect(unknown).not.toContain("<section");
    expect(unknown).not.toContain("Current version");
    expect(unknown).not.toContain(data.unreleased[0]);
  }
});

test("empty drafts disappear and publication dates display without timezone drift", () => {
  const html = renderToStaticMarkup(<ChangelogContent data={{ ...data, unreleased: [] }} />);
  expect(html).not.toContain("Unreleased");
  expect(html).toContain('dateTime="2026-10-10"');
  expect(html).toContain("October 10, 2026");
  expect(html).toContain("Use your preferred microphone.");
});

test("release notes render as safe readable text, including long and repeated bullets", () => {
  const note = `<script>alert('hello')</script> & ${"Long unbroken note".repeat(80)}`;
  const html = renderToStaticMarkup(<ChangelogContent data={{
    unreleased: [],
    releases: [{ version: "2.1.0", date: "2026-10-10", changes: [note, note] }],
  }} />);
  expect(html).not.toContain("<script>");
  expect(html).toContain("&lt;script&gt;alert(&#x27;hello&#x27;)&lt;/script&gt; &amp;");
  expect(html.match(/<li /g)).toHaveLength(2);
  expect(html).toContain("Long unbroken note".repeat(80));
});

test("the changelog reuses the accessible dialog and provides keyboard-scrollable content", () => {
  const html = renderToStaticMarkup(<ChangelogModal currentVersion="2.1.0" data={data} onClose={() => {}} />);
  expect(html).toContain('role="dialog"');
  expect(html).toContain('aria-modal="true"');
  expect(html).toContain('title="Close"');
  expect(html).toContain('tabindex="0" role="region" aria-label="Changelog"');
  expect(html).toContain("overflow-y-auto");
  expect(html).toContain("Changelog</h2>");
});

test("interface labels and dates follow the app language while authored bullets stay in English", () => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "document");
  Object.defineProperty(globalThis, "document", { configurable: true, value: { documentElement: { lang: "en" } } });
  try {
    setLocale("de");
    const html = renderToStaticMarkup(<ChangelogContent currentVersion="2.1.0" data={data} />);
    expect(html).not.toContain("Noch nicht veröffentlicht");
    expect(html).not.toContain(data.unreleased[0]);
    expect(html).toContain("Aktuelle Version");
    expect(html).toContain("10. Oktober 2026");
    expect(html).toContain("Use your preferred microphone.");
  } finally {
    setLocale("en");
    if (previous) Object.defineProperty(globalThis, "document", previous);
    else Reflect.deleteProperty(globalThis, "document");
  }
});
