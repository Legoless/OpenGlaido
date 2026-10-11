import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ChartNoAxesCombined, X } from "lucide-react";
import { t } from "./i18n";
import { endpointUrl, PROVIDERS } from "./providers";
import type { SaveConfig, TranscriptionConfig, WorkflowJudgeConfig, WorkflowReport } from "./types";
import { Dropdown, IconButton, IS_MAC, Modal, NoteText, OutlineButton, SettingsRow, TextInput, Toggle } from "./ui";
import { EMPTY_JUDGE, EMPTY_WORKFLOW_CONFIG, workflowEnabled, workflowJudgesReady } from "./workflow-insights";
import { WorkflowReportContent } from "./workflow-report";

export function WorkflowSettings({ config, save }: { config: TranscriptionConfig; save: SaveConfig }) {
  const [open, setOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const current = config.workflow_insights ?? EMPTY_WORKFLOW_CONFIG;
  const toggle = async (enabled: boolean) => {
    if (saving) return;
    setSaving(true);
    setError(await save({ workflow_insights: { ...current, enabled } }));
    setSaving(false);
  };
  return <>
    <SettingsRow icon={ChartNoAxesCombined} title={t("Workflow Insights (Beta)")}
      description={t("Opt in to local recording measurements and correction feedback. Nothing is analyzed automatically.")}
      note={error ? <NoteText tone="error">{error}</NoteText> : undefined}>
      <div className="flex items-center gap-3">
        <OutlineButton onClick={() => setOpen(true)}>{t("View insights")}</OutlineButton>
        {config.beta_features && <Toggle on={current.enabled} onChange={toggle} label={t("Workflow Insights (Beta)")} />}
      </div>
    </SettingsRow>
    {open && <WorkflowSettingsModal config={config} save={save} onClose={() => setOpen(false)} />}
  </>;
}

export function WorkflowSettingsModal({ config, save, onClose }: { config: TranscriptionConfig; save: SaveConfig; onClose: () => void }) {
  const [report, setReport] = useState<WorkflowReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [confirmClear, setConfirmClear] = useState(false);
  const current = config.workflow_insights ?? EMPTY_WORKFLOW_CONFIG;
  const [judges, setJudges] = useState<WorkflowJudgeConfig[]>(() => Array.from({ length: 3 }, (_, i) => ({ ...(current.judges[i] ?? EMPTY_JUDGE) })));
  useEffect(() => {
    let alive = true;
    const refresh = () => invoke<WorkflowReport>("workflow_report").then((value) => { if (alive) setReport(value); }).catch((reason) => { if (alive) setError(String(reason)); });
    refresh();
    const unlisten = listen("workflow-updated", refresh);
    return () => { alive = false; unlisten.then((stop) => stop()).catch(() => {}); };
  }, []);
  const update = async (patch: Partial<typeof current>) => {
    if (saving) return;
    setSaving(true);
    setError(await save({ workflow_insights: { ...current, ...patch } }));
    setSaving(false);
  };
  const clear = async () => {
    setSaving(true);
    setError(null);
    try {
      await invoke("workflow_clear");
      setReport(await invoke<WorkflowReport>("workflow_report"));
      setConfirmClear(false);
    } catch (reason) { setError(String(reason)); }
    finally { setSaving(false); }
  };
  const setJudge = (index: number, patch: Partial<WorkflowJudgeConfig>) => setJudges((previous) => previous.map((judge, i) => i === index ? { ...judge, ...patch } : judge));
  return <Modal onClose={onClose} width="w-[780px] max-w-[94vw]">
    <div className="flex max-h-[86vh] flex-col text-text-default">
      <div className="flex shrink-0 items-center justify-between gap-3 px-6 pt-5 pb-4">
        <h2 className="text-[20px] leading-7 font-medium">{t("Workflow Insights (Beta)")}</h2>
        <IconButton title={t("Close")} onClick={onClose}><X className="size-4" /></IconButton>
      </div>
      <div role="region" tabIndex={0} aria-label={t("Workflow Insights (Beta)")} className="min-h-0 overflow-y-auto px-6 pb-6 [scrollbar-gutter:stable]">
        <p className="gs-text-body-sm-regular mb-4 text-text-subdued">{t("Accuracy first, then speed and cost. Unreviewed recordings stay unknown; judge agreement is not proof of accuracy.")}</p>
        <div className="mb-4 rounded-[6px] border border-border-default p-3">
          <div className="flex items-center justify-between gap-4">
            <span className="gs-text-body-md-medium">{t("Collect local insights")}</span>
            {config.beta_features ? <Toggle on={current.enabled} onChange={(enabled) => update({ enabled })} label={t("Collect local insights")} /> : <span className="gs-text-body-sm-regular text-text-subdued">{t("Enable beta features first")}</span>}
          </div>
          <p className="gs-text-body-sm-regular mt-2 text-text-subdued">{t("Stores microphone, model settings, timing, original text and your corrections on this computer. Turning it off stops new collection; saved data stays available until cleared.")}</p>
        </div>
        <WorkflowEditTrackingSetting config={config} saving={saving} onChange={(track_edits) => update({ track_edits })} />
        {report ? <WorkflowReportContent report={report} /> : <p className="gs-text-body-sm-regular text-text-subdued">{t("Loading…")}</p>}
        <details className="mt-5 rounded-[6px] border border-border-default p-3">
          <summary className="gs-text-body-md-medium cursor-pointer">{t("Optional comparisons and 3 judges")}</summary>
          <div className="mt-3 flex items-center justify-between gap-4">
            <span className="gs-text-body-md-medium">{t("Allow cloud analysis")}</span>
            <Toggle on={current.cloud_analysis} onChange={(cloud_analysis) => update({ cloud_analysis })} label={t("Allow cloud analysis")} />
          </div>
          <p className="gs-text-body-sm-regular mt-2 text-text-subdued">{t("Only explicit Compare or Run 3 judges actions send selected audio or text to the shown endpoints. Provider charges may apply; each judge action makes at most 3 requests. Local servers can run without cloud consent.")}</p>
          <p className="gs-text-body-sm-regular mt-2 text-text-subdued">{t("Judges use API keys already configured in Model settings. They receive candidate text without model labels and your confirmed reference, never your audio.")}</p>
          <div className="mt-3 flex flex-col gap-3">
            {judges.map((judge, index) => <fieldset key={index} className="min-w-0 rounded-[4px] bg-transparent-primary p-3">
              <legend className="gs-text-body-sm-regular px-1">{t("Judge {n}", { n: index + 1 })}</legend>
              <div className="grid min-w-0 grid-cols-[145px_1fr] gap-2 max-[600px]:grid-cols-1">
                <Dropdown value={judge.provider} className="w-full" options={[...PROVIDERS.filter((provider) => provider.chat).map((provider) => ({ value: provider.id, label: provider.name })), { value: "custom", label: t("Custom") }]}
                  onChange={(value) => {
                    const provider = PROVIDERS.find((candidate) => candidate.id === value);
                    setJudge(index, provider ? { provider: provider.id, endpoint_url: endpointUrl(provider.baseUrl, "llm"), model_name: provider.defaultChat } : { provider: "custom" });
                  }} />
                <label className="min-w-0"><span className="sr-only">{t("Model name")}</span><TextInput value={judge.model_name} onChange={(model_name) => setJudge(index, { model_name })} placeholder={t("Model name")} className="w-full" /></label>
                <label className="col-span-full min-w-0"><span className="sr-only">{t("Endpoint URL")}</span><TextInput value={judge.endpoint_url} onChange={(endpoint_url) => setJudge(index, { endpoint_url })} placeholder={t("Endpoint URL")} className="w-full" /></label>
              </div>
            </fieldset>)}
          </div>
          <p className="gs-text-body-sm-regular mt-3 text-text-subdued">{t("Choose 3 distinct endpoint and model combinations. Disagreement and uncertain findings remain visible for your review.")}</p>
          <div className="mt-3"><OutlineButton disabled={saving || !workflowJudgesReady(judges)} onClick={() => update({ judges })}>{t("Save judges")}</OutlineButton></div>
        </details>
        {error && <div role="alert" className="mt-3"><NoteText tone="error">{error}</NoteText></div>}
        <div className="mt-5 flex flex-wrap items-center gap-3">
          <OutlineButton disabled={saving} onClick={() => confirmClear ? clear() : setConfirmClear(true)}>{confirmClear ? t("Confirm clear insights") : t("Clear insights data")}</OutlineButton>
          {confirmClear && <OutlineButton onClick={() => setConfirmClear(false)}>{t("Cancel")}</OutlineButton>}
        </div>
        {confirmClear && <p className="gs-text-body-sm-regular mt-2 text-text-subdued">{t("Deletes Insights measurements, feedback and analysis. Your History recordings are kept.")}</p>}
      </div>
    </div>
  </Modal>;
}

export function WorkflowEditTrackingSetting({ config, saving = false, supported = IS_MAC, onChange }: {
  config: TranscriptionConfig; saving?: boolean; supported?: boolean; onChange: (enabled: boolean) => void;
}) {
  if (!supported) return null;
  const enabled = workflowEnabled(config);
  return <fieldset disabled={saving || !enabled} className={`mb-4 rounded-[6px] border border-border-default p-3 ${enabled ? "" : "opacity-60"}`}>
    <div className="flex items-center justify-between gap-4">
      <span className="gs-text-body-md-medium">{t("Detect edits after paste")}</span>
      <Toggle on={!!config.workflow_insights?.track_edits} onChange={onChange} label={t("Detect edits after paste")} />
    </div>
    <p className="gs-text-body-sm-regular mt-2 text-text-subdued">{t("Briefly reads the destination textbox locally to identify your pasted passage. Only edits to that passage are saved as suggestions; surrounding text is not saved. Stops when focus changes, a new dictation starts, or after 2 minutes. Requires Accessibility permission and works in supported macOS textboxes.")}</p>
  </fieldset>;
}
