import type { TranscriptionConfig, WorkflowEntry, WorkflowFeedback, WorkflowInsightsConfig, WorkflowJudgeConfig, WorkflowRun, WorkflowComparePreview } from "./types";

export type WorkflowCandidate = "current" | "backup_1" | "backup_2" | "backup_3" | "backup_4" | "raw_output";
export type WorkflowReferenceKind = "spoken" | "output";

export const EMPTY_WORKFLOW_CONFIG: WorkflowInsightsConfig = { enabled: false, cloud_analysis: false, track_edits: false, judges: [] };
export const EMPTY_JUDGE: WorkflowJudgeConfig = { provider: "custom", endpoint_url: "", model_name: "" };

export function workflowEnabled(config: TranscriptionConfig | null | undefined): boolean {
  return !!(config?.beta_features && config.workflow_insights?.enabled);
}

export function workflowEditTrackingEnabled(config: TranscriptionConfig | null | undefined, supported: boolean): boolean {
  return supported && workflowEnabled(config) && !!config?.workflow_insights?.track_edits;
}

export function workflowJudgesReady(judges: WorkflowJudgeConfig[]): boolean {
  if (judges.length !== 3 || judges.some((judge) => !judge.provider.trim() || !judge.model_name.trim() || !judge.endpoint_url.trim())) return false;
  try {
    const scopes = judges.map((judge) => {
      const url = new URL(judge.endpoint_url.trim());
      if (url.username || url.password || url.search || url.hash || !url.hostname
        || (url.protocol !== "https:" && !(url.protocol === "http:" && !workflowEndpointRemote(url.href)))) throw Error("Invalid endpoint");
      return `${url.href.replace(/\/+$/, "")}\n${judge.model_name.trim()}`;
    });
    return new Set(scopes).size === 3;
  } catch { return false; }
}

/** Never seed confirmation from a History retry, which can replace the displayed transcript. */
export function workflowOriginal(entry: WorkflowEntry): WorkflowRun | undefined {
  return entry.runs.find((run) => run.kind === "recording");
}

export function workflowCorrectReference(entry: WorkflowEntry, kind: WorkflowReferenceKind): Omit<WorkflowFeedback, "updated_at"> | null {
  const original = workflowOriginal(entry);
  if (!original || !["completed", "formatting_fallback", "snippet", "clipboard_only", "paste_failed"].includes(original.outcome)) return null;
  const text = kind === "spoken" ? original.raw_text : original.final_text;
  return text.trim() ? { reference_kind: kind, reference_text: text, category: "correct" } : null;
}

/** Current reference only: prior jury votes become stale when the user corrects it again. */
export function workflowCurrentJudgements(entry: WorkflowEntry) {
  return entry.judgements.filter((judgement) => entry.feedback.some((reference) =>
    reference.reference_kind === judgement.reference_kind && reference.updated_at === judgement.reference_updated_at));
}

export function workflowEndpointRemote(endpoint: string): boolean {
  try { return !["localhost", "127.0.0.1", "[::1]"].includes(new URL(endpoint).hostname); }
  catch { return true; }
}

export function workflowJudgeAllowed(config: TranscriptionConfig): boolean {
  const insight = config.workflow_insights ?? EMPTY_WORKFLOW_CONFIG;
  return workflowEnabled(config) && workflowJudgesReady(insight.judges)
    && (insight.cloud_analysis || insight.judges.every((judge) => !workflowEndpointRemote(judge.endpoint_url)));
}

export function workflowSetupKey(run: WorkflowRun): string {
  const snapshot = run.snapshot;
  return JSON.stringify([snapshot.stt, snapshot.llm, snapshot.actual_microphone, snapshot.live, snapshot.configured_backups,
    snapshot.winning_provider, snapshot.style, snapshot.raw_text, snapshot.rule_id, snapshot.system_prompt,
    snapshot.languages, snapshot.dictionary_revision, snapshot.preprocessing, snapshot.app_version,
    snapshot.app_bundle_id, snapshot.website_host, snapshot.input_kind, snapshot.dictionary_changed ? run.id : null]);
}

export function workflowSetupGroups(entries: WorkflowEntry[]) {
  type Group = { run: WorkflowRun; recordings: number; comparisons: number; unknown: number;
    spoken_reviewed: number; output_reviewed: number; failed: number; edits: number; reference_words: number; measured_ms: number[] };
  const groups = new Map<string, Group>();
  const latest = new Map<string, { entry: WorkflowEntry; run: WorkflowRun; group: Group }>();
  for (const entry of entries) for (const run of entry.runs) {
    const key = workflowSetupKey(run);
    let group = groups.get(key);
    if (!group) {
      group = { run, recordings: 0, comparisons: 0, unknown: 0, spoken_reviewed: 0, output_reviewed: 0,
        failed: 0, edits: 0, reference_words: 0, measured_ms: [] };
      groups.set(key, group);
    }
    if (run.kind === "recording") group.recordings++;
    else group.comparisons++;
    if (workflowFailed(run.outcome, run.kind)) group.failed++;
    const time = run.timings.stt_ms !== null && run.timings.formatting_ms !== null
      ? run.timings.stt_ms + run.timings.formatting_ms : null;
    if (time !== null) group.measured_ms.push(time);
    // Replaying one clip repeatedly adds attempts, never more independent accuracy evidence.
    const sampleKey = JSON.stringify([entry.history_id, key]);
    const previous = latest.get(sampleKey);
    if (!previous || previous.run.created_at <= run.created_at) latest.set(sampleKey, { entry, run, group });
  }
  for (const { entry, run, group } of latest.values()) {
    if (!entry.feedback.length) group.unknown++;
    if (entry.feedback.some((reference) => reference.reference_kind === "spoken")) group.spoken_reviewed++;
    if (entry.feedback.some((reference) => reference.reference_kind === "output")) group.output_reviewed++;
    const error = entry.scores.find((score) => score.run_id === run.id)?.word_error;
    if (error) { group.edits += error.edits; group.reference_words += error.reference_words; }
  }
  return [...groups.values()];
}

export function workflowFailed(outcome: string, kind: WorkflowRun["kind"] = "recording"): boolean {
  return ["stt_failed", "paste_failed", "timed_out", "interrupted"].includes(outcome) || (outcome === "no_speech" && kind === "comparison");
}

export function workflowComparisonAllowed(config: TranscriptionConfig | null | undefined, preview: WorkflowComparePreview | null,
  audioAvailable: boolean, originalAvailable: boolean): boolean {
  if (!workflowEnabled(config) || !preview) return false;
  if (preview.raw_output ? !originalAvailable : !audioAvailable) return false;
  return !preview.cloud || !!config?.workflow_insights?.cloud_analysis;
}
