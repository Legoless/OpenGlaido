import { expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { WorkflowEditTrackingSetting, WorkflowSettings, WorkflowSettingsModal } from "../src/workflow-settings";
import { WorkflowEditSuggestion, WorkflowEntryModal } from "../src/workflow-history";
import { WorkflowReportContent, WorkflowRunContent } from "../src/workflow-report";
import { workflowConfig, workflowFixture } from "./workflow-insights.test";
import { setLocale } from "../src/i18n";

const item = { id: "history", text: "History retry text", raw_text: "History retry raw", duration_ms: 4000, audio_filename: "history.wav", created_at: "2026-10-11", kind: "dictation" as const, status: "ok" as const, sources: [] };

test("Settings exposes explicit local opt-in and preserves review access when beta is off", () => {
  const config = workflowConfig();
  const on = renderToStaticMarkup(<WorkflowSettings config={config} save={async () => null} />);
  expect(on).toContain("Workflow Insights (Beta)");
  expect(on).toContain('role="switch"');
  expect(on).toContain("Nothing is analyzed automatically.");
  const off = renderToStaticMarkup(<WorkflowSettings config={{ ...config, beta_features: false }} save={async () => null} />);
  expect(off).toContain("View insights");
  expect(off).not.toContain('role="switch"');
});

test("Insights dialog separates cloud consent and discloses destinations, requests and key handling", () => {
  const html = renderToStaticMarkup(<WorkflowSettingsModal config={workflowConfig()} save={async () => null} onClose={() => {}} />);
  expect(html).toContain('role="dialog"');
  expect(html).toContain("Allow cloud analysis");
  expect(html).toContain("at most 3 requests");
  expect(html).toContain("never your audio");
  expect(html).toContain("API keys already configured in Model settings");
  expect(html).not.toContain('type="password"');
  expect(html).toContain("Turning it off stops new collection");
});

test("summary keeps unknown samples separate and does not convert missing reference into perfect accuracy", () => {
  const html = renderToStaticMarkup(<WorkflowReportContent report={{ entries: [workflowFixture()], events: [] }} />);
  expect(html).toContain("latest 200");
  expect(html).toContain("Unknown: 1");
  expect(html).toContain("Word edits / reference words: Unknown");
  expect(html).not.toContain("0%");
  expect(html).toContain("Cost is unknown in this beta");
});

test("History actions are hidden on opt-out while original evidence and references remain reviewable", () => {
  const entry = workflowFixture();
  entry.feedback = [{ reference_kind: "spoken", reference_text: "Confirmed spoken text", category: "names", updated_at: "now" }];
  const config = workflowConfig(); config.workflow_insights.enabled = false;
  const html = renderToStaticMarkup(<WorkflowEntryModal item={item} config={config} initialEntry={entry} onEntry={() => {}} onClose={() => {}} />);
  expect(html).toContain("Collection is off");
  expect(html).toContain("Confirmed spoken text");
  expect(html).toContain("Call MXL at four");
  expect(html).not.toContain("History retry text");
  expect(html).not.toContain("Save confirmed reference");
  expect(html).not.toContain("Run 3 judges");
  expect(html).not.toContain("Mark original correct");
});

test("recording evidence distinguishes raw, formatted and final text and labels delivery as unverified", () => {
  const entry = workflowFixture();
  const html = renderToStaticMarkup(<WorkflowRunContent run={entry.runs[0]} entry={entry} />);
  expect(html).toContain("Raw transcription");
  expect(html).toContain("Formatted text");
  expect(html).toContain("Final output");
  expect(html).toContain("Delivery requested");
  expect(html).toContain("insertion into another app is not verified");
  expect(html).toContain("MXL");
  expect(html).toContain("Dictionary revision");
});

test("jury disagreement and uncertainty are shown as review evidence rather than measured accuracy", () => {
  const entry = workflowFixture();
  entry.feedback = [{ reference_kind: "spoken", reference_text: "Call MXL at four", category: "correct", updated_at: "now" }];
  entry.judgements = ["pass", "fail", "uncertain"].map((verdict, index) => ({ id: String(index), analysis_id: "analysis", run_id: "original", reference_kind: "spoken", reference_updated_at: "now", judge: { slot: 1, provider: "custom", model_name: String(index) }, verdict: verdict as "pass" | "fail" | "uncertain", issues: [{ category: "names", excerpt: "<script>bad()</script>", explanation: "Name issue" }], explanation: "Evidence only", created_at: "now", elapsed_ms: 10 }));
  const html = renderToStaticMarkup(<WorkflowRunContent run={entry.runs[0]} entry={entry} />);
  expect(html).toContain("Judges disagree");
  expect(html).toContain("Uncertain");
  expect(html).toContain("not proof of accuracy");
  expect(html).not.toContain("<script>");
  expect(html).toContain("&lt;script&gt;");
});

test("reference editing and comparison buttons remain disabled until their prerequisites are explicit", () => {
  const html = renderToStaticMarkup(<WorkflowEntryModal item={item} config={workflowConfig()} initialEntry={workflowFixture()} onEntry={() => {}} onClose={() => {}} />);
  expect(html).toContain("What I said");
  expect(html).toContain("Confirm what you said separately from what you wanted pasted");
  expect(html).toMatch(/<button[^>]*disabled=""[^>]*>Compare<\/button>/);
  expect(html).toMatch(/<button[^>]*disabled=""[^>]*>Run 3 judges<\/button>/);
});

test("translated Insights labels follow the app language", () => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "document");
  Object.defineProperty(globalThis, "document", { configurable: true, value: { documentElement: { lang: "en" } } });
  try {
    setLocale("de");
    const html = renderToStaticMarkup(<WorkflowSettings config={workflowConfig()} save={async () => null} />);
    expect(html).toContain("Workflow-Einblicke (Beta)");
  } finally { setLocale("en"); if (previous) Object.defineProperty(globalThis, "document", previous); else Reflect.deleteProperty(globalThis, "document"); }
});

test("recording details disclose a dictionary that changed while recording", () => {
  const entry = workflowFixture(); entry.runs[0].snapshot.dictionary_changed = true;
  const html = renderToStaticMarkup(<WorkflowRunContent run={entry.runs[0]} entry={entry} />);
  expect(html).toContain("Dictionary changed during recording");
});

test("textbox edit collection is a separate disabled-by-default supported-platform opt-in", () => {
  const config = workflowConfig();
  const render = (supported = true) => renderToStaticMarkup(<WorkflowEditTrackingSetting config={config} supported={supported} onChange={() => {}} />);
  expect(render(false)).toBe("");
  expect(render()).toContain('aria-checked="false"');
  expect(render()).toContain("surrounding text is not saved");
  expect(render()).toContain("after 2 minutes");
  expect(render()).toContain("Accessibility permission");
  config.workflow_insights.enabled = false;
  expect(render()).toContain('<fieldset disabled=""');
  config.workflow_insights.enabled = true; config.beta_features = false;
  expect(render()).toContain('<fieldset disabled=""');
});

test("detected edits stay safe suggestions requiring explicit desired-output confirmation", () => {
  const entry = workflowFixture();
  entry.edit_observation = { history_id: "history", original_text: "Call MXL at 4.", edited_text: "<script>six</script>", app_bundle_id: "com.editor", observed_at: "revision-one", status: "pending" };
  entry.feedback = [{ reference_kind: "output", reference_text: "Existing desired output", category: "formatting", updated_at: "existing" }];
  const html = renderToStaticMarkup(<WorkflowEntryModal item={item} config={workflowConfig()} initialEntry={entry} onEntry={() => {}} onClose={() => {}} />);
  expect(html).toContain("Suggested textbox edit");
  expect(html).toContain("Original pasted text");
  expect(html).toContain("Detected edit");
  expect(html).toContain("never verified spoken text");
  expect(html).toContain("This replaces your saved desired-output reference.");
  expect(html).toContain("Use as desired output");
  expect(html).toContain("Dismiss");
  expect(html).toContain("&lt;script&gt;six&lt;/script&gt;");
  expect(html).not.toContain("<script>");
});

test("opt-out makes suggestions read-only except dismissal and terminal reviews have no mutation buttons", () => {
  const observation = { history_id: "history", original_text: "old", edited_text: "new", app_bundle_id: null, observed_at: "v1", status: "pending" as const };
  const render = (status: "pending" | "confirmed" | "dismissed") => renderToStaticMarkup(<WorkflowEditSuggestion observation={{ ...observation, status }} enabled={false} saving={false} replacesOutput={false} onConfirm={() => {}} onDismiss={() => {}} />);
  const off = render("pending");
  expect(off).toMatch(/<button[^>]*disabled=""[^>]*>Use as desired output<\/button>/);
  expect(off).toMatch(/<button(?![^>]*disabled)[^>]*>Dismiss<\/button>/);
  expect(render("confirmed")).toContain("Saved as desired output");
  expect(render("dismissed")).toContain("Dismissed");
  expect(render("confirmed")).not.toContain("<button");
  expect(render("dismissed")).not.toContain("<button");
});
