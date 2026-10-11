import { t } from "./i18n";
import type { WorkflowEntry, WorkflowReport, WorkflowRun } from "./types";
import { workflowCurrentJudgements, workflowFailed, workflowSetupGroups } from "./workflow-insights";

export function workflowCategoryLabel(category: string): string {
  switch (category) {
    case "correct": return t("Correct");
    case "transcription": return t("Transcription");
    case "formatting": return t("Formatting");
    case "omission": return t("Missing content");
    case "invention": return t("Invented content");
    case "negation": return t("Negation");
    case "name":
    case "names": return t("Names");
    case "number":
    case "numbers": return t("Numbers");
    case "meaning": return t("Meaning changed");
    default: return t("Other");
  }
}

export function workflowOutcomeLabel(outcome: string): string {
  switch (outcome) {
    case "completed": return t("Processing complete");
    case "formatting_fallback": return t("Raw text used after formatting failed");
    case "snippet": return t("Snippet expansion");
    case "clipboard_only": return t("Copied to clipboard");
    case "paste_failed": return t("Delivery failed");
    case "stt_failed": return t("Transcription failed. Your audio is safe.");
    case "cancelled": return t("Cancelled");
    case "interrupted": return t("Interrupted");
    case "timed_out": return t("Analysis timed out");
    case "no_speech": return t("No speech detected");
    default: return t("Unknown");
  }
}

function workflowStyleLabel(style: string): string {
  return style === "casual" ? t("Casual") : style === "lowercase" ? t("Lowercase") : t("Standard");
}

export function workflowRunLabel(run: WorkflowRun): string {
  if (run.kind === "recording") return t("Original recording");
  if (run.label === "current") return t("Current setup");
  if (run.label === "raw_output") return t("Original raw text (no requests)");
  const backup = /^backup_([1-4])$/.exec(run.label);
  return backup ? t("Backup {n}", { n: backup[1] }) : t("Comparison");
}

export function workflowTiming(ms: number | null): string {
  return ms === null ? t("Unknown") : `${(ms / 1000).toFixed(2)} s`;
}

export function WorkflowReportContent({ report }: { report: WorkflowReport }) {
  const reviewed = report.entries.filter((entry) => entry.feedback.length > 0).length;
  const groups = workflowSetupGroups(report.entries);
  const failed = report.entries.filter((entry) => entry.runs.some((run) => run.kind === "recording" && workflowFailed(run.outcome))).length;
  const categories = new Map<string, number>();
  for (const entry of report.entries) for (const reference of entry.feedback) categories.set(reference.category, (categories.get(reference.category) ?? 0) + 1);
  return <>
    <p className="gs-text-body-sm-regular mb-3 text-text-subdued">{t("Summary covers up to the latest 200 Insights recordings.")}</p>
    <div className="grid grid-cols-4 gap-3 max-[600px]:grid-cols-2">
      {[[t("Reviewed"), reviewed], [t("Unknown"), report.entries.length - reviewed], [t("Failed"), failed], [t("Recorded setups"), groups.length]].map(([label, value]) =>
        <div key={label} className="rounded-[6px] bg-transparent-primary p-3"><p className="gs-text-body-sm-regular text-text-subdued">{label}</p><p className="mt-1 text-xl">{value}</p></div>)}
    </div>
    {groups.length === 0 && <p className="gs-text-body-sm-regular mt-3 text-text-subdued">{t("Opt in, dictate, then review a recording in History to build evidence.")}</p>}
    {groups.length > 0 && <div className="mt-4 flex flex-col gap-3">
      <h3 className="gs-text-body-md-medium">{t("Evidence by setup")}</h3>
      <p className="gs-text-body-sm-regular text-text-subdued">{t("Compare recordings from the same workflow. Word edits measure transcription against what you confirmed, including harmless wording differences. Cost is unknown in this beta.")}</p>
      {groups.map((group, index) => <div key={index} className="rounded-[6px] border border-border-default p-3">
        <p className="gs-text-body-md-medium break-words">{group.run.snapshot.stt.provider} · {group.run.snapshot.stt.model_name}</p>
        <p className="gs-text-body-sm-regular mt-1 break-words text-text-subdued">{group.run.snapshot.actual_microphone ?? t("Unknown microphone")} · {group.run.snapshot.app_name ?? t("Unknown app")} · {group.run.snapshot.llm.model_name || t("Formatting off")}</p>
        <p className="gs-text-body-sm-regular mt-1 break-words text-text-subdued">{t("Style")}: {workflowStyleLabel(group.run.snapshot.style)} · {t("Formatting rule")}: {group.run.snapshot.rule_id || t("All apps")} · {t("Languages")}: {group.run.snapshot.languages.join(", ") || t("Automatic")} · {t("Dictionary revision")}: {group.run.snapshot.dictionary_revision.slice(0, 12)}</p>
        {group.run.snapshot.dictionary_changed && <p className="gs-text-body-sm-regular mt-1 text-text-subdued">{t("Dictionary changed during recording")}</p>}
        {group.run.snapshot.configured_backups.length > 0 && <p className="gs-text-body-sm-regular mt-1 break-words text-text-subdued">{t("Configured backups")}: {group.run.snapshot.configured_backups.map((model) => `${model.provider} / ${model.model_name}`).join(", ")} · {t("Winning provider")}: {group.run.snapshot.winning_provider ?? t("Unknown")}</p>}
        <div className="gs-text-body-sm-regular mt-2 flex flex-wrap gap-x-4 gap-y-1 text-text-subdued">
          <span>{t("Recordings: {n}", { n: group.recordings })}</span><span>{t("Comparisons: {n}", { n: group.comparisons })}</span>
          <span>{t("Spoken reviewed: {n}", { n: group.spoken_reviewed })}</span><span>{t("Output reviewed: {n}", { n: group.output_reviewed })}</span>
          <span>{t("Unknown: {n}", { n: group.unknown })}</span><span>{t("Failures: {n}", { n: group.failed })}</span>
        </div>
        <p className="gs-text-body-sm-regular mt-2">{t("Word edits / reference words")}: {group.reference_words ? `${group.edits} / ${group.reference_words} (${Math.round(group.edits / group.reference_words * 100)}%)` : t("Unknown")}</p>
        <p className="gs-text-body-sm-regular mt-1 text-text-subdued">{t("Average measured processing")}: {group.measured_ms.length ? workflowTiming(group.measured_ms.reduce((a, b) => a + b, 0) / group.measured_ms.length) : t("Unknown")} · {t("Samples: {n}", { n: group.measured_ms.length })}</p>
      </div>)}
      <p className="gs-text-body-sm-regular text-text-subdued">{t("Live timing measures finalization after stop; replay timing measures complete processing.")}</p>
    </div>}
    {categories.size > 0 && <div className="mt-4"><h3 className="gs-text-body-md-medium">{t("Confirmed categories")}</h3><p className="gs-text-body-sm-regular mt-1 text-text-subdued">{[...categories].map(([category, count]) => `${workflowCategoryLabel(category)}: ${count}`).join(" · ")}</p></div>}
    {report.events.length > 0 && <details className="gs-text-body-sm-regular mt-4 text-text-subdued"><summary>{t("Operational events")}</summary><ul className="mt-2">{report.events.map((event) => <li key={event.category} className="break-words">{event.category}: {event.count}</li>)}</ul></details>}
  </>;
}

export function WorkflowRunContent({ run, entry }: { run: WorkflowRun; entry: WorkflowEntry }) {
  const snapshot = run.snapshot;
  const score = entry.scores.find((value) => value.run_id === run.id)?.word_error;
  const judgements = workflowCurrentJudgements(entry).filter((judgement) => judgement.run_id === run.id);
  const latestAnalysis = judgements[judgements.length - 1]?.analysis_id;
  const currentJury = judgements.filter((judgement) => judgement.analysis_id === latestAnalysis);
  const disagreement = new Set(currentJury.map((judge) => judge.verdict)).size > 1;
  return <div className="rounded-[6px] border border-border-default p-3">
    <div className="flex flex-wrap items-center justify-between gap-2"><h4 className="gs-text-body-md-medium">{workflowRunLabel(run)}</h4><span className="gs-text-body-sm-regular text-text-subdued">{workflowOutcomeLabel(run.outcome)}</span></div>
    <p className="gs-text-body-sm-regular mt-2 break-words text-text-subdued">{snapshot.stt.provider} · {snapshot.stt.model_name} · {snapshot.actual_microphone ?? t("Unknown microphone")}</p>
    {snapshot.configured_backups.length > 0 && <p className="gs-text-body-sm-regular mt-1 text-text-subdued">{t("Configured backups")}: {snapshot.configured_backups.map((model) => `${model.provider} / ${model.model_name}`).join(", ")} · {t("Winning provider")}: {snapshot.winning_provider ?? t("Unknown")}</p>}
    <div className="mt-3 grid gap-3">
      {[[t("Raw transcription"), run.raw_text], [t("Formatted text"), run.formatted_text], [t("Final output"), run.final_text]].map(([label, text]) => <div key={label}><p className="gs-text-body-sm-regular text-text-subdued">{label}</p><p className="gs-text-body-sm-regular mt-1 whitespace-pre-wrap break-words select-text">{text || "—"}</p></div>)}
    </div>
    {score && <p className="gs-text-body-sm-regular mt-3">{t("Word edits / reference words")}: {score.edits} / {score.reference_words} ({Math.round(score.rate * 100)}%)</p>}
    <details className="gs-text-body-sm-regular mt-3 text-text-subdued"><summary>{t("Recording details")}</summary>
      <dl className="mt-2 grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)] gap-x-3 gap-y-1">
        {[[t("Capture ready"), workflowTiming(run.timings.capture_ready_ms)], [t("First audio sample"), workflowTiming(run.timings.first_sample_ms)],
          [run.kind === "recording" && snapshot.live ? t("Live finalization") : t("Transcription"), workflowTiming(run.timings.stt_ms)], [t("Formatting"), workflowTiming(run.timings.formatting_ms)],
          [t("Delivery requested"), workflowTiming(run.timings.delivery_requested_ms)], [t("Formatting rule"), snapshot.rule_id || t("All apps")],
          [t("Dictionary revision"), snapshot.dictionary_revision.slice(0, 12)], [t("Languages"), snapshot.languages.join(", ") || t("Automatic")],
          [t("Audio preprocessing"), snapshot.preprocessing], [t("Sample rate"), snapshot.sample_rate ? `${snapshot.sample_rate} Hz` : t("Unknown")],
          [t("Clipped samples"), run.timings.clipping_fraction === null ? t("Unknown") : `${(run.timings.clipping_fraction * 100).toFixed(1)}%`],
          [t("App version"), snapshot.app_version],
          [t("Audio duration"), workflowTiming(run.timings.audio_duration_ms)],
          [t("Input"), snapshot.input_kind === "saved_audio_replay" ? t("Saved audio replay") : snapshot.input_kind === "saved_raw_text" ? t("Saved raw text") : t("Microphone")],
          [t("Transcription mode"), snapshot.live ? t("Live") : t("After recording")]].map(([label, value]) => <div key={label} className="contents"><dt>{label}</dt><dd className="break-words">{value}</dd></div>)}
      </dl>
      {snapshot.dictionary_changed && <p className="mt-2">{t("Dictionary changed during recording")}</p>}
      {snapshot.system_prompt && <details className="mt-2"><summary>{t("Formatting")}</summary><p className="mt-1 whitespace-pre-wrap break-words select-text">{snapshot.system_prompt}</p></details>}
      <p className="mt-2">{t("Live timing measures finalization after stop; replay timing measures complete processing.")}</p>
      <p className="mt-2">{t("Delivery timing records a paste request; insertion into another app is not verified.")}</p>
    </details>
    {currentJury.length > 0 && <div className="mt-3 rounded-[4px] bg-transparent-primary p-3">
      <p className="gs-text-body-md-medium">{disagreement ? t("Judges disagree") : t("Judge findings")}</p>
      <p className="gs-text-body-sm-regular mt-1 text-text-subdued">{t("Judge agreement is not proof of accuracy. Review the passages and your reference.")}</p>
      <p className="gs-text-body-sm-regular mt-1 text-text-subdued">{currentJury[0]?.reference_kind === "spoken" ? t("What I said") : t("What I wanted pasted")}</p>
      {currentJury.map((judge) => <div key={judge.id} className="mt-3 gs-text-body-sm-regular">
        <p className="break-words">{t("Judge {n}", { n: judge.judge.slot })} · {judge.judge.provider} · {judge.judge.model_name} · {judge.verdict === "pass" ? t("Pass") : judge.verdict === "fail" ? t("Fail") : t("Uncertain")}</p>
        <p className="mt-1 whitespace-pre-wrap break-words text-text-subdued">{judge.explanation}</p>
        {judge.issues.map((issue, index) => <p key={index} className="mt-1 whitespace-pre-wrap break-words select-text">{workflowCategoryLabel(issue.category)}: “{issue.excerpt}” — {issue.explanation}</p>)}
      </div>)}
    </div>}
  </div>;
}
