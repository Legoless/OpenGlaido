import { expect, test } from "bun:test";
import { EMPTY_WORKFLOW_CONFIG, workflowComparisonAllowed, workflowCorrectReference, workflowCurrentJudgements,
  workflowEditTrackingEnabled, workflowEnabled, workflowJudgeAllowed, workflowJudgesReady, workflowSetupGroups } from "../src/workflow-insights";
import type { TranscriptionConfig, WorkflowEntry, WorkflowRun } from "../src/types";

export function workflowFixture(): WorkflowEntry {
  const run: WorkflowRun = {
    id: "original", history_id: "history", created_at: "2026-10-11", kind: "recording", label: "Original", outcome: "completed",
    raw_text: "Call MXL at four", formatted_text: "Call MXL at 4.", final_text: "Call MXL at 4.",
    snapshot: { stt: { source: "cloud", provider: "openai", model_name: "gpt-transcribe" }, llm: { source: "off", provider: "off", model_name: "" },
      live: false, configured_backups: [], winning_provider: null, style: "standard", raw_text: false, rule_id: null, system_prompt: "Format text",
      languages: ["en"], dictionary_revision: "dictionary-one", dictionary_changed: false, app_version: "2.1.0", app_bundle_id: "com.editor", app_name: "Editor",
      website_host: null, preprocessing: "16kHz", input_kind: "microphone", actual_microphone: "MXL", sample_rate: 48000, fallback: false },
    timings: { capture_ready_ms: 10, first_sample_ms: 20, audio_duration_ms: 4000, stt_ms: 500, formatting_ms: 100, delivery_requested_ms: 800, clipping_fraction: 0.01 },
  };
  return { history_id: "history", runs: [run], feedback: [], judgements: [], scores: [{ run_id: "original", word_error: null }] };
}

export function workflowConfig(): TranscriptionConfig {
  return { beta_features: true, workflow_insights: { ...EMPTY_WORKFLOW_CONFIG, enabled: true }, stt_source: "cloud",
    endpoint_url: "https://api.provider.example/v1/audio/transcriptions", local_stt_model: "whisper", llm_source: "off" } as TranscriptionConfig;
}

test("workflow collection requires both beta and an explicit opt-in", () => {
  const config = workflowConfig();
  expect(workflowEnabled(config)).toBe(true);
  expect(workflowEnabled({ ...config, beta_features: false })).toBe(false);
  expect(workflowEnabled({ ...config, workflow_insights: { ...EMPTY_WORKFLOW_CONFIG } })).toBe(false);
  expect(workflowEnabled(null)).toBe(false);
  expect(workflowEnabled(undefined)).toBe(false);
});

test("textbox tracking requires independent opt-in, local collection and platform support but no cloud consent", () => {
  const config = workflowConfig();
  expect(EMPTY_WORKFLOW_CONFIG.track_edits).toBe(false);
  expect(workflowEditTrackingEnabled(config, true)).toBe(false);
  config.workflow_insights.track_edits = true;
  expect(workflowEditTrackingEnabled(config, true)).toBe(true);
  expect(workflowEditTrackingEnabled(config, false)).toBe(false);
  config.workflow_insights.cloud_analysis = false;
  expect(workflowEditTrackingEnabled(config, true)).toBe(true);
  config.workflow_insights.enabled = false;
  expect(workflowEditTrackingEnabled(config, true)).toBe(false);
  config.workflow_insights.enabled = true; config.beta_features = false;
  expect(workflowEditTrackingEnabled(config, true)).toBe(false);
});

test("unconfirmed textbox edits never count as reviewed output or spoken accuracy", () => {
  const entry = workflowFixture();
  entry.edit_observation = { history_id: entry.history_id, original_text: "Call MXL at 4.", edited_text: "Call MXL at six.", app_bundle_id: "com.editor", observed_at: "v1", status: "pending" };
  const group = workflowSetupGroups([entry])[0];
  expect(group.unknown).toBe(1);
  expect(group.spoken_reviewed).toBe(0);
  expect(group.output_reviewed).toBe(0);
  expect(group.reference_words).toBe(0);
  expect(workflowCurrentJudgements(entry)).toEqual([]);
});

test("correct feedback confirms only the selected immutable original reference", () => {
  const entry = workflowFixture();
  entry.runs.unshift({ ...entry.runs[0], id: "retry", kind: "comparison", raw_text: "Wrong retry", final_text: "Changed retry" });
  expect(workflowCorrectReference(entry, "spoken")).toEqual({ reference_kind: "spoken", reference_text: "Call MXL at four", category: "correct" });
  expect(workflowCorrectReference(entry, "output")).toEqual({ reference_kind: "output", reference_text: "Call MXL at 4.", category: "correct" });
  entry.runs = entry.runs.filter((run) => run.kind === "comparison");
  expect(workflowCorrectReference(entry, "spoken")).toBeNull();
  const failed = workflowFixture();
  failed.runs[0].outcome = "stt_failed";
  expect(workflowCorrectReference(failed, "spoken")).toBeNull();
});

test("judge readiness requires three distinct valid destinations without URL credentials", () => {
  const judges = ["a", "b", "c"].map((model_name) => ({ provider: "custom", model_name, endpoint_url: "http://localhost:1234/v1/chat/completions" }));
  expect(workflowJudgesReady(judges)).toBe(true);
  expect(workflowJudgesReady(judges.slice(0, 2))).toBe(false);
  expect(workflowJudgesReady([judges[0], judges[0], judges[2]])).toBe(false);
  for (const endpoint_url of ["https://key:secret@example.com/v1", "https://example.com/v1?key=secret", "https://example.com/v1#secret", "http://example.com/v1", "file:///tmp/model", "invalid"]) {
    expect(workflowJudgesReady([{ ...judges[0], endpoint_url }, judges[1], judges[2]])).toBe(false);
  }
});

test("remote judges require separate cloud consent while loopback judges do not", () => {
  const config = workflowConfig();
  config.workflow_insights.judges = ["a", "b", "c"].map((model_name) => ({ provider: "custom", model_name, endpoint_url: "http://localhost:1234/v1/chat/completions" }));
  expect(workflowJudgeAllowed(config)).toBe(true);
  config.workflow_insights.judges[0].endpoint_url = "https://example.com/v1/chat/completions";
  expect(workflowJudgeAllowed(config)).toBe(false);
  config.workflow_insights.cloud_analysis = true;
  expect(workflowJudgeAllowed(config)).toBe(true);
  config.beta_features = false;
  expect(workflowJudgeAllowed(config)).toBe(false);
});

test("comparison consent follows the effective backend preview, including raw app rules and legacy audio", () => {
  const config = workflowConfig();
  config.llm_source = "cloud"; config.llm_endpoint_url = "https://formatter.example/chat";
  const rawRule = { audio_destination: "Local model: whisper", text_destination: null, cloud: false, raw_output: false };
  expect(workflowComparisonAllowed(config, rawRule, true, false)).toBe(true);
  expect(workflowComparisonAllowed(config, { ...rawRule, cloud: true }, true, false)).toBe(false);
  expect(workflowComparisonAllowed(config, null, true, true)).toBe(false);
  expect(workflowComparisonAllowed(config, { ...rawRule, raw_output: true }, true, false)).toBe(false);
  expect(workflowComparisonAllowed(config, { ...rawRule, raw_output: true }, false, true)).toBe(true);
});

test("unreviewed setup runs remain unknown and never become zero-error evidence", () => {
  const unknown = workflowFixture();
  const groups = workflowSetupGroups([unknown]);
  expect(groups[0].unknown).toBe(1);
  expect(groups[0].spoken_reviewed).toBe(0);
  expect(groups[0].reference_words).toBe(0);
  expect(groups[0].measured_ms).toEqual([600]); // STT + formatting, not unverified delivery.
  const reviewed = workflowFixture();
  reviewed.feedback = [{ reference_kind: "spoken", reference_text: "Call MXL at five", category: "numbers", updated_at: "now" }];
  reviewed.scores = [{ run_id: "original", word_error: { edits: 1, reference_words: 4, hypothesis_words: 4, rate: 0.25 } }];
  const evidence = workflowSetupGroups([reviewed])[0];
  expect(evidence.unknown).toBe(0);
  expect(evidence.spoken_reviewed).toBe(1);
  expect(evidence.output_reviewed).toBe(0);
  expect(evidence.edits).toBe(1);
  expect(evidence.reference_words).toBe(4);
});

test("setup evidence separates microphones and dictionary revisions instead of attributing all changes to the model", () => {
  const entry = workflowFixture();
  const mic = workflowFixture(); mic.runs[0].snapshot.actual_microphone = "System mic";
  const dictionary = workflowFixture(); dictionary.runs[0].snapshot.dictionary_revision = "dictionary-two";
  expect(workflowSetupGroups([entry, mic, dictionary])).toHaveLength(3);
});

test("jury evidence is ignored after the selected confirmed reference changes", () => {
  const entry = workflowFixture();
  entry.feedback = [{ reference_kind: "spoken", reference_text: "Call MXL", category: "correct", updated_at: "current" }];
  const base = { id: "judge", analysis_id: "analysis", run_id: "original", reference_kind: "spoken" as const, reference_updated_at: "old",
    judge: { slot: 1, provider: "custom", model_name: "a" }, verdict: "pass" as const, issues: [], explanation: "Looks good", created_at: "now", elapsed_ms: 30 };
  entry.judgements = [base, { ...base, id: "current", reference_updated_at: "current" }];
  expect(workflowCurrentJudgements(entry).map((judge) => judge.id)).toEqual(["current"]);
});

test("empty comparison output is a failure while an ordinary silent recording is not", () => {
  const silence = workflowFixture(); silence.runs[0].outcome = "no_speech"; silence.runs[0].raw_text = "";
  expect(workflowSetupGroups([silence])[0].failed).toBe(0);
  const comparison = workflowFixture(); comparison.runs[0].kind = "comparison"; comparison.runs[0].outcome = "no_speech"; comparison.runs[0].raw_text = "";
  expect(workflowSetupGroups([comparison])[0].failed).toBe(1);
});

test("a mid-recording dictionary change cannot be grouped with an unchanged configuration", () => {
  const unchanged = workflowFixture();
  const changed = workflowFixture(); changed.runs[0].id = "changed"; changed.runs[0].snapshot.dictionary_changed = true;
  const anotherChanged = workflowFixture(); anotherChanged.runs[0].id = "another-changed"; anotherChanged.runs[0].snapshot.dictionary_changed = true;
  expect(workflowSetupGroups([unchanged, changed, anotherChanged])).toHaveLength(3);
});

test("clipboard, snippet and delivery-failure results can be explicitly confirmed without inferring insertion", () => {
  for (const outcome of ["snippet", "clipboard_only", "paste_failed"]) {
    const entry = workflowFixture(); entry.runs[0].outcome = outcome;
    expect(workflowCorrectReference(entry, "output")?.reference_text).toBe(entry.runs[0].final_text);
  }
});

test("repeated comparisons keep attempt counts but use only the latest evidence per recording and setup", () => {
  const entry = workflowFixture();
  entry.runs[0].kind = "comparison";
  const repeated = { ...entry.runs[0], id: "repeated", created_at: "2026-10-12" };
  entry.runs.push(repeated);
  entry.feedback = [{ reference_kind: "spoken", reference_text: "Call MXL at five", category: "numbers", updated_at: "now" },
    { reference_kind: "output", reference_text: "Call MXL at 5.", category: "numbers", updated_at: "now" }];
  entry.scores = [{ run_id: "original", word_error: { edits: 1, reference_words: 4, hypothesis_words: 4, rate: 0.25 } },
    { run_id: "repeated", word_error: { edits: 2, reference_words: 4, hypothesis_words: 4, rate: 0.5 } }];
  const group = workflowSetupGroups([entry])[0];
  expect(group.comparisons).toBe(2);
  expect(group.measured_ms).toHaveLength(2);
  expect(group.spoken_reviewed).toBe(1);
  expect(group.output_reviewed).toBe(1);
  expect(group.edits).toBe(2);
  expect(group.reference_words).toBe(4);
  expect(group.unknown).toBe(0);
  const anotherRecording = workflowFixture();
  anotherRecording.history_id = "another-history";
  anotherRecording.runs[0].history_id = anotherRecording.history_id;
  anotherRecording.feedback = entry.feedback;
  anotherRecording.scores = [{ run_id: "original", word_error: { edits: 1, reference_words: 4, hypothesis_words: 4, rate: 0.25 } }];
  const independent = workflowSetupGroups([entry, anotherRecording])[0];
  expect(independent.spoken_reviewed).toBe(2);
  expect(independent.output_reviewed).toBe(2);
  expect(independent.edits).toBe(3);
  expect(independent.reference_words).toBe(8);
});

test("repeated unreviewed clips count once as unknown and later evidence wins regardless of array order", () => {
  const entry = workflowFixture();
  entry.runs[0].kind = "comparison";
  entry.runs.unshift({ ...entry.runs[0], id: "latest", created_at: "2026-10-13" });
  entry.runs.push({ ...entry.runs[1], id: "middle", created_at: "2026-10-12" });
  expect(workflowSetupGroups([entry])[0].unknown).toBe(1);
  entry.feedback = [{ reference_kind: "spoken", reference_text: "Call MXL at five", category: "numbers", updated_at: "now" }];
  entry.scores = entry.runs.map((run) => ({ run_id: run.id, word_error: { edits: run.id === "latest" ? 2 : 1, reference_words: 4, hypothesis_words: 4, rate: 0.25 } }));
  const group = workflowSetupGroups([entry])[0];
  expect(group.edits).toBe(2);
  expect(group.reference_words).toBe(4);
  expect(group.spoken_reviewed).toBe(1);
});
