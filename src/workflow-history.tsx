import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { X } from "lucide-react";
import { t } from "./i18n";
import type { HistoryItem, TranscriptionConfig, WorkflowAnalysisStatus, WorkflowEntry, WorkflowComparePreview } from "./types";
import { Dropdown, IconButton, Modal, NoteText, OutlineButton } from "./ui";
import { EMPTY_WORKFLOW_CONFIG, workflowComparisonAllowed, workflowCorrectReference, workflowEnabled,
  workflowJudgeAllowed, workflowOriginal, type WorkflowCandidate, type WorkflowReferenceKind } from "./workflow-insights";
import { WorkflowRunContent } from "./workflow-report";

export function WorkflowHistory({ item, config }: { item: HistoryItem; config?: TranscriptionConfig | null }) {
  const [entry, setEntry] = useState<WorkflowEntry | null>(null);
  const [open, setOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    const refresh = () => invoke<WorkflowEntry>("workflow_get_entry", { historyId: item.id }).then((value) => { if (alive) setEntry(value); }).catch((reason) => { if (alive) setError(String(reason)); });
    refresh();
    const unlisten = listen("workflow-updated", refresh);
    return () => { alive = false; unlisten.then((stop) => stop()).catch(() => {}); };
  }, [item.id]);
  if (item.kind !== "dictation" || (!workflowEnabled(config) && !entry?.runs.length && !entry?.feedback.length && !entry?.edit_observation)) return null;
  return <>
    <div className="mt-1 flex flex-wrap items-center justify-between gap-3 border-t border-border-default pt-3">
      <div className="min-w-0"><p className="gs-text-body-sm-regular">{t("Workflow Insights (Beta)")}</p><p className="gs-text-body-sm-regular mt-1 text-text-subdued">{entry?.feedback.length ? t("Reviewed") : t("Unknown")}</p></div>
      <OutlineButton onClick={() => setOpen(true)}>{t("Review insights")}</OutlineButton>
    </div>
    {open && <WorkflowEntryModal item={item} config={config} initialEntry={entry} initialError={error} onEntry={setEntry} onClose={() => setOpen(false)} />}
  </>;
}

export function WorkflowEntryModal({ item, config, initialEntry = null, initialError = null, onEntry, onClose }: {
  item: HistoryItem;
  config?: TranscriptionConfig | null;
  initialEntry?: WorkflowEntry | null;
  initialError?: string | null;
  onEntry: (entry: WorkflowEntry) => void;
  onClose: () => void;
}) {
  const [entry, setEntry] = useState(initialEntry);
  const [error, setError] = useState(initialError);
  const [referenceKind, setReferenceKind] = useState<WorkflowReferenceKind>("spoken");
  const [referenceText, setReferenceText] = useState("");
  const [category, setCategory] = useState("transcription");
  const [candidate, setCandidate] = useState<WorkflowCandidate>("current");
  const [previewResult, setPreviewResult] = useState<{ config: TranscriptionConfig | null | undefined; candidate: WorkflowCandidate; history_id: string; value: WorkflowComparePreview | null; error: string | null } | null>(null);
  const [status, setStatus] = useState<WorkflowAnalysisStatus>({ running: false, history_id: null, kind: null });
  const [saving, setSaving] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const mounted = useRef(true);
  const previousRunning = useRef(false);
  const enabled = workflowEnabled(config);
  const insight = config?.workflow_insights ?? EMPTY_WORKFLOW_CONFIG;
  const accept = (value: WorkflowEntry) => { if (mounted.current) { setEntry(value); onEntry(value); } };
  useEffect(() => {
    mounted.current = true;
    if (!initialEntry) invoke<WorkflowEntry>("workflow_get_entry", { historyId: item.id }).then(accept).catch((reason) => { if (mounted.current) setError(String(reason)); });
    let busy = false;
    const refresh = async () => {
      if (busy) return;
      busy = true;
      try {
        const value = await invoke<WorkflowAnalysisStatus>("workflow_analysis_status");
        if (mounted.current) {
          if (previousRunning.current && !value.running) invoke<WorkflowEntry>("workflow_get_entry", { historyId: item.id }).then(accept).catch((reason) => { if (mounted.current) setError(String(reason)); });
          previousRunning.current = value.running;
          setStatus(value);
        }
      } catch (reason) { if (mounted.current) setError(String(reason)); }
      finally { busy = false; }
    };
    refresh();
    const timer = window.setInterval(refresh, 1000);
    const unlisten = listen("workflow-updated", () => invoke<WorkflowEntry>("workflow_get_entry", { historyId: item.id }).then(accept).catch((reason) => { if (mounted.current) setError(String(reason)); }));
    return () => { mounted.current = false; window.clearInterval(timer); unlisten.then((stop) => stop()).catch(() => {}); };
  }, [item.id]);
  const reference = entry?.feedback.find((feedback) => feedback.reference_kind === referenceKind);
  const original = entry ? workflowOriginal(entry) : null;
  const originalText = referenceKind === "spoken" ? original?.raw_text ?? "" : original?.final_text ?? "";
  useEffect(() => {
    setReferenceText(reference?.reference_text ?? originalText);
    setCategory(reference?.category ?? (referenceKind === "spoken" ? "transcription" : "formatting"));
  }, [referenceKind, reference?.updated_at, originalText]);
  useEffect(() => {
    let alive = true;
    if (enabled) invoke<WorkflowComparePreview>("workflow_compare_preview", { historyId: item.id, candidate }).then((value) => {
      if (alive) setPreviewResult({ config, candidate, history_id: item.id, value, error: null });
    }).catch((reason) => {
      if (alive) setPreviewResult({ config, candidate, history_id: item.id, value: null, error: String(reason) });
    });
    return () => { alive = false; };
  }, [config, item.id, candidate, enabled]);
  const previewCurrent = previewResult?.config === config && previewResult?.candidate === candidate && previewResult?.history_id === item.id;
  const preview = previewCurrent ? previewResult?.value ?? null : null;
  const previewError = previewCurrent ? previewResult?.error : null;
  const destinationLabel = (destination: string | null) => destination?.startsWith("Local model: ")
    ? `${t("Local model")}: ${destination.slice(13)}` : destination;
  const persist = async (text: string, feedbackCategory: string) => {
    if (!enabled || saving || !text.trim()) return;
    setSaving(true);
    setError(null);
    try { accept(await invoke<WorkflowEntry>("workflow_save_feedback", { historyId: item.id, feedback: { reference_kind: referenceKind, reference_text: text, category: feedbackCategory } })); }
    catch (reason) { if (mounted.current) setError(String(reason)); }
    finally { if (mounted.current) setSaving(false); }
  };
  const correct = entry ? workflowCorrectReference(entry, referenceKind) : null;
  const runAnalysis = async (kind: "compare" | "judge") => {
    if (!enabled || status.running || submitting) return;
    setSubmitting(true);
    setError(null);
    setStatus({ running: true, history_id: item.id, kind });
    try {
      accept(await invoke<WorkflowEntry>(kind === "compare" ? "workflow_compare" : "workflow_judge", kind === "compare" ? { historyId: item.id, candidate } : { historyId: item.id, referenceKind }));
    } catch (reason) { if (mounted.current) setError(String(reason)); }
    finally {
      if (mounted.current) {
        setSubmitting(false);
        invoke<WorkflowAnalysisStatus>("workflow_analysis_status").then(setStatus).catch((reason) => setError(String(reason)));
      }
    }
  };
  const cancel = async () => {
    setError(null);
    try { await invoke("workflow_cancel"); }
    catch (reason) { if (mounted.current) setError(String(reason)); }
  };
  const reviewEdit = async (action: "confirm" | "dismiss") => {
    const observation = entry?.edit_observation;
    if (!observation || observation.status !== "pending" || saving || (action === "confirm" && !enabled)) return;
    setSaving(true);
    setError(null);
    try { accept(await invoke<WorkflowEntry>(`workflow_${action}_edit`, { historyId: item.id, expectedObservedAt: observation.observed_at })); }
    catch (reason) { if (mounted.current) setError(String(reason)); }
    finally { if (mounted.current) setSaving(false); }
  };
  const compareAllowed = workflowComparisonAllowed(config, preview, !!item.audio_filename, !!original?.raw_text.trim());
  const judgeAllowed = enabled && !!config && workflowJudgeAllowed(config)
    && !!entry?.feedback.some((feedback) => feedback.reference_kind === referenceKind) && !!entry?.runs.length;
  const candidates: { value: string; label: string }[] = [{ value: "current", label: t("Current setup") }, { value: "raw_output", label: t("Original raw text (no requests)") }];
  for (const n of [1, 2, 3, 4]) if ((config as unknown as Record<string, unknown> | null | undefined)?.[`stt_backup_${n}_enabled`]) candidates.push({ value: `backup_${n}`, label: t("Backup {n}", { n }) });
  return <Modal onClose={onClose} width="w-[780px] max-w-[94vw]">
    <div className="flex max-h-[86vh] flex-col text-text-default">
      <div className="flex shrink-0 items-center justify-between gap-3 px-6 pt-5 pb-4"><h2 className="text-[20px] leading-7 font-medium">{t("Review insights")}</h2><IconButton title={t("Close")} onClick={onClose}><X className="size-4" /></IconButton></div>
      <div role="region" tabIndex={0} aria-label={t("Review insights")} className="min-h-0 overflow-y-auto px-6 pb-6 [scrollbar-gutter:stable]">
        {!enabled && <p className="gs-text-body-sm-regular mb-3 text-text-subdued">{t("Collection is off. Saved insights remain available; enable the opt-in beta to add feedback or analysis.")}</p>}
        {!entry?.runs.length && <p className="gs-text-body-sm-regular mb-3 text-text-subdued">{t("No original setup snapshot is available for this recording.")}</p>}
        <p className="gs-text-body-sm-regular mb-3 text-text-subdued">{t("Confirm what you said separately from what you wanted pasted. Formatting edits are not automatically transcription errors.")}</p>
        {entry?.edit_observation && <WorkflowEditSuggestion observation={entry.edit_observation} enabled={enabled} saving={saving}
          replacesOutput={entry.feedback.some((feedback) => feedback.reference_kind === "output")}
          onConfirm={() => reviewEdit("confirm")} onDismiss={() => reviewEdit("dismiss")} />}
        {enabled && <div className="rounded-[6px] border border-border-default p-3">
          <div className="flex flex-wrap items-center justify-between gap-3"><Dropdown value={referenceKind} options={[{ value: "spoken", label: t("What I said") }, { value: "output", label: t("What I wanted pasted") }]} onChange={(kind) => setReferenceKind(kind as WorkflowReferenceKind)} /><OutlineButton disabled={saving || !correct} onClick={() => correct && persist(correct.reference_text, correct.category)}>{t("Mark original correct")}</OutlineButton></div>
          <label className="mt-3 block"><span className="gs-text-body-sm-regular text-text-subdued">{referenceKind === "spoken" ? t("What I said") : t("What I wanted pasted")}</span><textarea value={referenceText} onChange={(event) => setReferenceText(event.target.value)} rows={4} maxLength={64000} className="gs-text-body-sm-regular mt-1 w-full resize-y rounded-[4px] border border-border-default bg-background-input p-2 focus:outline-none" /></label>
          <div className="mt-3 flex flex-wrap items-center justify-between gap-3"><Dropdown value={category} options={[
            { value: "correct", label: t("Correct") }, { value: "transcription", label: t("Transcription") }, { value: "formatting", label: t("Formatting") },
            { value: "names", label: t("Names") }, { value: "numbers", label: t("Numbers") }, { value: "meaning", label: t("Meaning changed") }, { value: "other", label: t("Other") },
          ]} onChange={(value) => setCategory(value ?? "other")} /><OutlineButton disabled={saving || !referenceText.trim()} onClick={() => persist(referenceText, category)}>{t("Save confirmed reference")}</OutlineButton></div>
        </div>}
        {!enabled && entry?.feedback.map((feedback) => <div key={feedback.reference_kind} className="mb-3 rounded-[6px] border border-border-default p-3"><p className="gs-text-body-sm-regular text-text-subdued">{feedback.reference_kind === "spoken" ? t("What I said") : t("What I wanted pasted")}</p><p className="gs-text-body-sm-regular mt-1 whitespace-pre-wrap break-words select-text">{feedback.reference_text}</p></div>)}
        {enabled && config && <div className="mt-4 rounded-[6px] border border-border-default p-3">
          <div className="flex flex-wrap items-center justify-between gap-3"><Dropdown value={candidate} options={candidates} onChange={(value) => setCandidate(value as WorkflowCandidate)} /><OutlineButton disabled={!compareAllowed || status.running || submitting} onClick={() => runAnalysis("compare")}>{t("Compare")}</OutlineButton></div>
          <p className="gs-text-body-sm-regular mt-2 break-words text-text-subdued">{candidate === "raw_output" ? t("Uses the original raw text without network requests. Your History result stays unchanged.") : t("Replays this audio with the selected setup. Your History result stays unchanged; microphone comparisons need a fresh recording.")}</p>
          {candidate !== "raw_output" && <p className="gs-text-body-sm-regular mt-2 break-words select-text text-text-subdued">{t("Audio destination")}: {destinationLabel(preview?.audio_destination ?? null) || t("Unknown")}<br />{t("Text destination")}: {preview ? destinationLabel(preview.text_destination) || t("Formatting off") : t("Unknown")}</p>}
          {candidate !== "raw_output" && preview?.cloud && !insight.cloud_analysis && <p className="gs-text-body-sm-regular mt-2 text-text-subdued">{t("Allow cloud analysis in Insights settings to use remote endpoints.")}</p>}
          {previewError && <p role="alert" className="gs-text-body-sm-regular mt-2 text-text-subdued">{previewError}</p>}
          <div className="mt-4 flex flex-wrap items-center justify-between gap-3"><p className="gs-text-body-sm-regular text-text-subdued">{t("Uses the saved reference selected above.")}</p><OutlineButton disabled={!judgeAllowed || status.running || submitting} onClick={() => runAnalysis("judge")}>{t("Run 3 judges")}</OutlineButton></div>
          <p className="gs-text-body-sm-regular mt-2 break-words select-text text-text-subdued">{t("Judge destinations")}: {insight.judges.map((judge) => `${judge.endpoint_url} (${judge.model_name})`).join(" · ") || t("Configure judges in Insights settings")}</p>
        </div>}
        {status.running && <div role="status" className="mt-3 flex flex-wrap items-center justify-between gap-3"><p className="gs-text-body-sm-regular text-text-subdued">{status.history_id === item.id ? t("Analysis running…") : t("Another recording is being analyzed…")}</p><OutlineButton onClick={cancel}>{t("Cancel analysis")}</OutlineButton></div>}
        {error && <div role="alert" className="mt-3"><NoteText tone="error">{error}</NoteText></div>}
        <p className="gs-text-body-sm-regular mt-4 text-text-subdued">{t("Keeps the original result and latest 8 comparisons. Each result keeps the latest judge findings for each reference.")}</p>
        <div className="mt-4 flex flex-col gap-3">{entry?.runs.map((run) => <WorkflowRunContent key={run.id} run={run} entry={entry} />)}</div>
      </div>
    </div>
  </Modal>;
}

export function WorkflowEditSuggestion({ observation, enabled, saving, replacesOutput, onConfirm, onDismiss }: {
  observation: NonNullable<WorkflowEntry["edit_observation"]>; enabled: boolean; saving: boolean; replacesOutput: boolean;
  onConfirm: () => void; onDismiss: () => void;
}) {
  return <div className="mb-4 rounded-[6px] border border-border-default p-3">
    <p className="gs-text-body-md-medium">{t("Suggested textbox edit")}</p>
    <p className="gs-text-body-sm-regular mt-2 text-text-subdued">{t("Textbox edits stay unconfirmed until you review them. They can become desired output, never verified spoken text.")}</p>
    <div className="mt-3 grid min-w-0 grid-cols-2 gap-3 max-[600px]:grid-cols-1">
      <div className="min-w-0"><p className="gs-text-body-sm-regular text-text-subdued">{t("Original pasted text")}</p><p className="gs-text-body-sm-regular mt-1 whitespace-pre-wrap break-words select-text">{observation.original_text}</p></div>
      <div className="min-w-0"><p className="gs-text-body-sm-regular text-text-subdued">{t("Detected edit")}</p><p className="gs-text-body-sm-regular mt-1 whitespace-pre-wrap break-words select-text">{observation.edited_text}</p></div>
    </div>
    {observation.status === "pending" ? <>
      {replacesOutput && <p className="gs-text-body-sm-regular mt-3 text-text-subdued">{t("This replaces your saved desired-output reference.")}</p>}
      <div className="mt-3 flex flex-wrap gap-3"><OutlineButton disabled={!enabled || saving} onClick={onConfirm}>{t("Use as desired output")}</OutlineButton><OutlineButton disabled={saving} onClick={onDismiss}>{t("Dismiss")}</OutlineButton></div>
    </> : <p className="gs-text-body-sm-regular mt-3 text-text-subdued">{observation.status === "confirmed" ? t("Saved as desired output") : t("Dismissed")}</p>}
  </div>;
}
