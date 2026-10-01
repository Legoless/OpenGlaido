import { useEffect, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { AudioLines, Box, Check, Cloud, Download, FolderOpen, Globe, KeyRound, RotateCw, Sparkles, Trash2, X } from "lucide-react";
import { locale, t } from "./i18n";
import { PROVIDERS, baseFromUrl, endpointUrl, providerModelIds } from "./providers";
import type { DownloadProgress, LocalModel, SaveConfig, TranscriptionConfig } from "./types";
import { Dropdown, IS_MAC, IconButton, NoteText, OutlineButton, SettingsRow, SettingsSection, TextInput } from "./ui";

type Kind = LocalModel["kind"];
type ErrorNote = (key: keyof TranscriptionConfig) => ReactNode;

// Config fields behind each section (the original top-level fields are the cloud transcription ones).
const FIELDS = {
  stt: { provider: "stt_provider", url: "endpoint_url", model: "model_name", key: "api_key", local: "local_stt_model" },
  llm: { provider: "llm_provider", url: "llm_endpoint_url", model: "llm_model_name", key: "llm_api_key", local: "local_llm_model" },
} as const;

/** "547 MB", "3.1 GB" (decimal units, like Finder). */
function formatSize(bytes: number): string {
  const gb = bytes >= 1e9;
  const n = new Intl.NumberFormat(locale(), { maximumFractionDigits: gb ? 1 : 0 }).format(bytes / (gb ? 1e9 : 1e6));
  return `${n} ${gb ? "GB" : "MB"}`;
}

const busy = (d?: DownloadProgress | null) => d?.state === "downloading" || d?.state === "verifying";

/** Settings › Model: where transcription and the language model run, on this Mac or with a cloud provider. */
export function ModelSettings({ config, save, errorNote }: { config: TranscriptionConfig; save: SaveConfig; errorNote: ErrorNote }) {
  const [supported, setSupported] = useState(IS_MAC);
  const [models, setModels] = useState<LocalModel[]>([]);
  // "model-download" events and failed actions, on top of what list_local_models reported.
  const [downloads, setDownloads] = useState<Record<string, DownloadProgress>>({});
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  const load = () => invoke<LocalModel[]>("list_local_models").then(setModels).catch(console.error);
  useEffect(() => {
    invoke<boolean>("local_models_supported").then(setSupported).catch(console.error);
    load();
    const unlisten = listen<DownloadProgress>("model-download", (e) => {
      setDownloads((d) => ({ ...d, [e.payload.id]: e.payload }));
      if (!busy(e.payload)) load();
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  const setDownload = (p: DownloadProgress) => setDownloads((d) => ({ ...d, [p.id]: p }));
  // A cancelled download also rejects; that isn't an error.
  const fail = (id: string, e: unknown) =>
    setDownloads((d) =>
      d[id]?.state === "cancelled" ? d : { ...d, [id]: { id, received: 0, total: 0, state: "failed", error: String(e) } },
    );
  const download = (m: LocalModel) => {
    setDownload({ id: m.id, received: 0, total: m.size_bytes, state: "downloading", error: null });
    invoke("download_model", { id: m.id }).catch((e) => fail(m.id, e));
  };
  const cancel = (p: DownloadProgress) => {
    setDownload({ ...p, state: "cancelled" });
    invoke("cancel_model_download", { id: p.id }).catch(console.error);
  };
  const remove = (id: string) => {
    setConfirmDelete(null);
    invoke("delete_model", { id })
      .then(() =>
        setDownloads((d) => {
          const next = { ...d };
          delete next[id];
          return next;
        }),
      )
      .catch((e) => fail(id, e))
      .finally(load);
  };

  const modelList = (kind: Kind) => {
    const field = FIELDS[kind].local;
    const list = models.filter((m) => m.kind === kind);
    if (list.length === 0) return null;
    return (
      <>
        <div
          role="radiogroup"
          aria-label={kind === "stt" ? t("Transcription") : t("Language model")}
          className="mx-3 flex shrink-0 flex-col divide-y divide-border-default overflow-hidden rounded-[8px] border border-border-default"
        >
          {list.map((m) => {
            const dl = downloads[m.id] ?? m.download;
            const downloaded = m.downloaded || dl?.state === "done";
            const failed = dl?.state === "failed";
            const selected = config[field] === m.id;
            const select = () => {
              if (!selected) save({ [field]: m.id });
              if (!downloaded && !busy(dl)) download(m);
            };
            const pct = dl?.total ? Math.floor((dl.received / dl.total) * 100) : 0;
            return (
              <div
                key={m.id}
                onClick={select}
                className={`flex cursor-default items-center gap-3 px-3 py-3 transition-colors hover:bg-transparent-secondary ${
                  selected ? "bg-transparent-primary" : ""
                }`}
              >
                {/* The radio is only the check and text: its children are hidden from screen readers, the buttons aren't. */}
                <div
                  role="radio"
                  aria-checked={selected}
                  tabIndex={0}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      select();
                    }
                  }}
                  className="flex min-w-0 flex-1 items-center gap-3 rounded-[4px] focus-visible:outline-1 focus-visible:outline-border-default"
                >
                <span
                  className={`mt-0.5 flex size-4 shrink-0 items-center justify-center self-start rounded-full ${
                    selected ? "bg-button-primary" : "border border-border-default"
                  }`}
                >
                  {selected && <Check className="size-3 text-text-dark" strokeWidth={3} />}
                </span>
                <div className="flex min-w-0 flex-1 flex-col gap-1">
                  <div className="flex flex-wrap items-center gap-1.5">
                    <span className="gs-text-body-md-medium text-text-default">{t(m.name)}</span>
                    {m.recommended && <Badge accent>{t("Recommended")}</Badge>}
                    {m.english_only && <Badge>{t("English only")}</Badge>}
                  </div>
                  <span className="gs-text-body-sm-regular text-text-subdued">{t(m.notes)}</span>
                  <div className="mt-0.5 flex flex-wrap items-center gap-x-4 gap-y-1">
                    <Meter label={t("Speed")} value={m.speed} />
                    <Meter label={t("Accuracy")} value={m.accuracy} />
                    <span className="gs-text-body-xs-regular text-text-disabled tabular-nums">{formatSize(m.size_bytes)}</span>
                  </div>
                  {failed && dl.error ? (
                    <NoteText tone="error">{t(dl.error)}</NoteText>
                  ) : (
                    selected &&
                    !downloaded &&
                    !busy(dl) && <NoteText tone="warning">{t("Download this model to use it.")}</NoteText>
                  )}
                </div>
                </div>
                <div className="flex shrink-0 items-center gap-1" onClick={(e) => e.stopPropagation()}>
                  {dl && busy(dl) ? (
                    <div className="flex w-[168px] items-center gap-1">
                      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
                        <div className="h-1 overflow-hidden rounded-full bg-transparent-tertiary">
                          <div
                            className="h-full rounded-full bg-text-accent transition-[width] duration-300"
                            style={{ width: `${dl.state === "verifying" ? 100 : pct}%` }}
                          />
                        </div>
                        <span className="gs-text-body-xs-regular truncate text-text-subdued tabular-nums">
                          {dl.state === "verifying"
                            ? t("Verifying…")
                            : `${pct}% · ${formatSize(dl.received)} / ${formatSize(dl.total)}`}
                        </span>
                      </div>
                      <IconButton onClick={() => cancel(dl)} title={t("Cancel download")}>
                        <X className="size-3.5" />
                      </IconButton>
                    </div>
                  ) : downloaded ? (
                    confirmDelete === m.id ? (
                      <OutlineButton onClick={() => remove(m.id)} autoFocus>
                        <Trash2 className="size-3.5 text-text-error" />
                        {t("Delete from this Mac?")}
                      </OutlineButton>
                    ) : (
                      <IconButton onClick={() => setConfirmDelete(m.id)} title={t("Delete model")}>
                        <Trash2 className="size-4" />
                      </IconButton>
                    )
                  ) : (
                    <>
                      {/* An unfinished download (kept for resuming) can take gigabytes: let it be removed. */}
                      {m.partial_bytes > 0 &&
                        (confirmDelete === m.id ? (
                          <OutlineButton onClick={() => remove(m.id)} autoFocus>
                            <Trash2 className="size-3.5 text-text-error" />
                            {t("Delete from this Mac?")}
                          </OutlineButton>
                        ) : (
                          <IconButton onClick={() => setConfirmDelete(m.id)} title={t("Delete model")}>
                            <Trash2 className="size-4" />
                          </IconButton>
                        ))}
                      {confirmDelete !== m.id && (
                        <OutlineButton onClick={() => download(m)}>
                          {failed ? <RotateCw className="size-3.5" /> : <Download className="size-3.5" />}
                          {failed ? t("Retry") : t("Download")}
                        </OutlineButton>
                      )}
                    </>
                  )}
                </div>
              </div>
            );
          })}
        </div>
        {errorNote(field) && <div className="px-3 pt-2">{errorNote(field)}</div>}
        {/* Once per tab: under the transcription list when that one is shown. */}
        {(kind === "stt" || config.stt_source !== "local") && (
          <div className="flex flex-wrap items-center justify-between gap-2 px-3 pt-3">
            <span className="gs-text-body-sm-regular text-text-disabled">{t("Models are stored on this Mac.")}</span>
            <OutlineButton onClick={() => invoke("reveal_models_folder").catch(console.error)}>
              <FolderOpen className="size-3.5" />
              {t("Show in Finder")}
            </OutlineButton>
          </div>
        )}
      </>
    );
  };

  const onMac = supported ? [{ value: "local", label: t("On this Mac") } as const] : [];
  const sttModel = models.find((m) => m.id === config.local_stt_model);
  const englishOnly = config.stt_source === "local" && sttModel?.english_only && config.languages.some((l) => l !== "en");

  return (
    <>
      <SettingsSection label={t("Transcription")} />
      <SettingsRow
        icon={AudioLines}
        title={t("Transcription")}
        description={t("Turns your voice into text.")}
        note={
          <>
            {errorNote("stt_source")}
            {englishOnly && (
              <NoteText tone="warning">
                {t("This model only understands English. Pick a multilingual model for your other dictation languages.")}
              </NoteText>
            )}
          </>
        }
      >
        <Segmented
          label={t("Transcription")}
          value={config.stt_source}
          options={[...onMac, { value: "cloud", label: t("Cloud") }]}
          onChange={(v) => save({ stt_source: v })}
        />
      </SettingsRow>
      {supported && config.stt_source === "local" ? (
        modelList("stt")
      ) : (
        <CloudSettings kind="stt" config={config} save={save} errorNote={errorNote} />
      )}

      <SettingsSection label={t("Language model")} />
      <SettingsRow
        icon={Sparkles}
        title={t("Language model")}
        description={t("Cleans up your dictation and answers commands.")}
        note={
          <>
            {errorNote("llm_source")}
            {config.llm_source === "local" && (
              <NoteText tone="hint">
                {t("Commands answer without tools on a local model. Built-in commands like web search need a cloud model.")}
              </NoteText>
            )}
            {config.llm_source === "off" && (
              <NoteText tone="hint">{t("Without a language model, dictations skip AI cleanup and commands don't work.")}</NoteText>
            )}
          </>
        }
      >
        <Segmented
          label={t("Language model")}
          value={config.llm_source}
          options={[{ value: "off", label: t("Off") }, ...onMac, { value: "cloud", label: t("Cloud") }]}
          onChange={(v) => save({ llm_source: v, ...(v === "cloud" ? sharedKey(config) : {}) })}
        />
      </SettingsRow>
      {config.llm_source === "cloud" ? (
        <CloudSettings kind="llm" config={config} save={save} errorNote={errorNote} />
      ) : (
        supported && config.llm_source === "local" && modelList("llm")
      )}
    </>
  );
}

/** Transcription's key for a language model without one on the same preset provider: it is pasted once. */
function sharedKey(config: TranscriptionConfig): Partial<TranscriptionConfig> {
  const samePreset = config.llm_provider === config.stt_provider && PROVIDERS.some((p) => p.id === config.llm_provider);
  return samePreset && !config.llm_api_key && config.api_key ? { llm_api_key: config.api_key } : {};
}

/** Provider preset (or Custom URL), API key and model for one cloud section. */
function CloudSettings({ kind, config, save, errorNote }: { kind: Kind; config: TranscriptionConfig; save: SaveConfig; errorNote: ErrorNote }) {
  const f = FIELDS[kind];
  const url = config[f.url] ?? "";
  // A preset only when the saved URL is its endpoint; anything else shows as Custom so the URL can be fixed.
  const preset = PROVIDERS.find((p) => p.id === config[f.provider] && endpointUrl(p.baseUrl, kind) === url.trim());
  const apiKey = config[f.key];
  const model = config[f.model] ?? "";
  const base = preset?.baseUrl ?? baseFromUrl(url);
  // Scribe uses presets rather than OpenAI-compatible model discovery.
  const canList = !!base && base !== "https://api.elevenlabs.io/v1" && !(preset?.keyUrl && !apiKey);
  const [listed, setListed] = useState<{ ids: string[]; error: string | null } | null>(null);
  const [other, setOther] = useState(false);

  useEffect(() => {
    setListed(null);
    if (!canList) return;
    let alive = true;
    invoke<string[]>("list_provider_models", { baseUrl: base, apiKey, kind })
      .then((ids) => alive && setListed({ ids, error: null }))
      .catch((e) => alive && setListed({ ids: [], error: String(e) }));
    return () => {
      alive = false;
    };
  }, [kind, base, apiKey, canList]);

  const openaiStt = kind === "stt" && preset?.id === "openai";
  const elevenlabsStt = kind === "stt" && preset?.id === "elevenlabs";
  const ids = providerModelIds(preset, kind, listed?.ids, model);
  const modelOptions = [
    ...(model ? [] : [{ value: "", label: t("Choose a model") }]),
    ...ids.map((id) => ({
      value: id,
      label: openaiStt && id === "gpt-transcribe"
        ? `GPT Transcribe · ${t("After recording")}`
        : openaiStt && id === "gpt-live-transcribe"
          ? `GPT Live Transcribe · ${t("Real time")}`
          : elevenlabsStt && id === "scribe_v2"
            ? `Scribe v2 · ${t("After recording")}`
            : elevenlabsStt && id === "scribe_v2_realtime"
              ? `Scribe v2 · ${t("Real time")}`
              : id,
    })),
    { value: null, label: t("Other…") },
  ];

  // One key serves both sections when they use the same provider: it is pasted once.
  const otherSection = FIELDS[kind === "stt" ? "llm" : "stt"];
  const pickProvider = (id: string) => {
    const p = PROVIDERS.find((x) => x.id === id);
    const shared = p && config[otherSection.provider] === id ? config[otherSection.key] : "";
    save(
      p
        ? {
            [f.provider]: id,
            [f.url]: endpointUrl(p.baseUrl, kind),
            [f.model]: kind === "stt" ? p.defaultStt : p.defaultChat,
            // Never send one provider's key to another: the other section's key for the same provider, else none.
            [f.key]: shared,
          }
        : { [f.provider]: "custom", [f.key]: "" },
    );
  };
  const saveKey = (v: string) => {
    if (v === apiKey) return;
    const share = preset && config[otherSection.provider] === preset.id && !config[otherSection.key];
    save({ [f.key]: v, ...(share ? { [otherSection.key]: v } : {}) });
  };

  return (
    <>
      <SettingsRow
        icon={Cloud}
        title={t("Provider")}
        description={kind === "stt" ? t("Where your audio is sent") : t("Where your text is sent")}
        note={errorNote(f.provider)}
      >
        <Dropdown
          value={preset?.id ?? "custom"}
          options={[
            ...PROVIDERS.filter((p) => (kind === "stt" ? p.stt : p.chat)).map((p) => ({ value: p.id, label: p.name })),
            { value: "custom", label: t("Custom") },
          ]}
          onChange={(v) => v && pickProvider(v)}
          className="w-[200px] max-w-full"
        />
      </SettingsRow>
      {!preset && (
        <SettingsRow icon={Globe} title={t("Endpoint URL")} description={t("OpenAI-compatible endpoint")} note={errorNote(f.url)}>
          <CommitInput
            value={url}
            onCommit={(v) => v !== url && save({ [f.url]: v })}
            placeholder={endpointUrl("https://example.com/v1", kind)}
          />
        </SettingsRow>
      )}
      {(!preset || preset.keyUrl) && (
        <SettingsRow
          icon={KeyRound}
          title={t("API key")}
          description={t("Stored in your system keychain")}
          note={
            <>
              {preset?.keyUrl && (
                <button
                  type="button"
                  onClick={() => openUrl(preset.keyUrl).catch(console.error)}
                  className="gs-text-body-sm-regular self-start text-text-accent hover:underline"
                >
                  {t("Get an API key")}
                </button>
              )}
              {errorNote(f.key)}
            </>
          }
        >
          <CommitInput
            type="password"
            value={apiKey}
            onCommit={saveKey}
            placeholder={t("Paste your API key")}
          />
        </SettingsRow>
      )}
      <SettingsRow
        icon={Box}
        title={t("Model")}
        description={(openaiStt && model === "gpt-live-transcribe") || (elevenlabsStt && model === "scribe_v2_realtime")
          ? t("Streams while you speak; pastes when you stop.")
          : (openaiStt && model === "gpt-transcribe") || (elevenlabsStt && model === "scribe_v2")
            ? t("Transcribes after you stop recording.")
            : t("Pick one from the list or type its name")}
        note={
          <>
            {elevenlabsStt && <NoteText tone="hint">{t("Dictionary hints add 20% to ElevenLabs costs (up to 50 in real time or 100 after recording).")}</NoteText>}
            {canList && !listed && <NoteText tone="hint">{t("Loading models…")}</NoteText>}
            {listed?.error && <NoteText tone="error">{t(listed.error)}</NoteText>}
            {errorNote(f.model)}
          </>
        }
      >
        <div className="flex max-w-full flex-col items-end gap-2">
          <Dropdown
            value={other ? null : model}
            options={modelOptions}
            search={t("Search models...")}
            onChange={(v) => {
              setOther(v === null);
              if (v && v !== model) save({ [f.model]: v });
            }}
            className={`${openaiStt || elevenlabsStt ? "w-[300px]" : "w-[200px]"} max-w-full`}
          />
          {other && (
            <CommitInput
              value={model}
              autoFocus
              onCommit={(v) => {
                setOther(false);
                if (v && v !== model) save({ [f.model]: v });
              }}
              placeholder={t("Model name")}
            />
          )}
        </div>
      </SettingsRow>
    </>
  );
}

/** TextInput with a local draft that follows the saved value; commits the trimmed text on blur or Enter. */
function CommitInput({
  value,
  onCommit,
  placeholder,
  type,
  autoFocus,
}: {
  value: string;
  onCommit: (v: string) => void;
  placeholder?: string;
  type?: string;
  autoFocus?: boolean;
}) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  return (
    <TextInput
      type={type}
      value={draft}
      onChange={setDraft}
      onCommit={() => onCommit(draft.trim())}
      placeholder={placeholder}
      autoFocus={autoFocus}
      className="w-[200px] max-w-full"
    />
  );
}

function Segmented<T extends string>({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
}) {
  if (options.length < 2) return null; // nothing to choose
  return (
    <div role="radiogroup" aria-label={label} className="flex items-center gap-1 rounded-[6px] bg-transparent-primary p-1">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={value === o.value}
          onClick={() => value !== o.value && onChange(o.value)}
          className={`gs-text-body-md-regular rounded-[4px] px-3 py-1 whitespace-nowrap transition-colors ${
            value === o.value ? "bg-transparent-tertiary text-text-default" : "text-text-subdued hover:text-text-default"
          }`}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

function Badge({ accent, children }: { accent?: boolean; children: ReactNode }) {
  return (
    <span
      className={`gs-text-tag rounded-[2px] px-1.5 py-[3px] ${
        accent ? "bg-badge-surface text-badge-text" : "bg-transparent-tertiary text-text-subdued"
      }`}
    >
      {children}
    </span>
  );
}

/** 1–5 dots. */
function Meter({ label, value }: { label: string; value: number }) {
  return (
    <span role="img" aria-label={`${label} ${value}/5`} className="flex items-center gap-1.5">
      <span className="gs-text-body-xs-regular text-text-disabled">{label}</span>
      <span className="flex gap-[3px]">
        {[1, 2, 3, 4, 5].map((i) => (
          <span key={i} className={`size-[5px] rounded-full ${i <= value ? "bg-text-subdued" : "bg-transparent-tertiary"}`} />
        ))}
      </span>
    </span>
  );
}
