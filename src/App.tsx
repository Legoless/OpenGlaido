import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  AudioLines,
  BookOpen,
  Check,
  Command,
  Copy,
  Cpu,
  Flame,
  Globe,
  Home,
  KeyRound,
  Languages,
  Mic,
  Pause,
  Pencil,
  Play,
  Plus,
  RotateCw,
  Search,
  Settings,
  SquareMenu,
  SquareText,
  Timer,
  Trash2,
  Volume2,
  X,
  Zap,
} from "lucide-react";

// ------------------------------------------------------------------
// Types (mirror src-tauri/src/lib.rs)
// ------------------------------------------------------------------
interface TranscriptionConfig {
  endpoint_url: string;
  api_key: string;
  model_name: string;
  language?: string | null;
  temperature?: number | null;
  mode: string;
  enable_llm_formatting: boolean;
  llm_endpoint_url?: string | null;
  llm_model_name?: string | null;
  custom_formatting_prompt?: string | null;
  sound_feedback: boolean;
  hotkey: string;
}

interface HistoryItem {
  id: string;
  text: string;
  raw_text: string;
  duration_ms: number;
  audio_filename?: string | null;
  created_at: string;
}

interface DictionaryItem {
  id: string;
  phrase: string;
  replacement: string;
}

interface SnippetItem {
  id: string;
  trigger: string;
  content: string;
}

type Page = "home" | "formatting" | "dictionary" | "snippets";
type SettingsTab = "dictation" | "hotkeys" | "model";
type IconType = React.ComponentType<{ className?: string; fill?: string }>;

// ------------------------------------------------------------------
// Small shared primitives
// ------------------------------------------------------------------
function Toggle({ on, onChange, label }: { on: boolean; onChange: (v: boolean) => void; label?: string }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      onClick={() => onChange(!on)}
      className={`inline-flex h-5 w-10 shrink-0 items-center rounded-full p-[3px] transition-colors ${
        on ? "bg-toggle-on" : "bg-background-tertiary"
      }`}
    >
      <span
        className={`block h-3.5 w-5 rounded-full bg-white transition-transform ${on ? "translate-x-[14px]" : ""}`}
      />
    </button>
  );
}

function OutlineButton({
  onClick,
  children,
}: {
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="gs-text-body-md-medium inline-flex h-7 shrink-0 items-center gap-2 whitespace-nowrap rounded-[2px] border border-border-default px-2.5 text-text-subdued transition-colors hover:text-text-default"
    >
      {children}
    </button>
  );
}

function LimeButton({ onClick, children }: { onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="gs-text-body-md-medium inline-flex h-8 shrink-0 items-center gap-2 whitespace-nowrap rounded-[4px] bg-button-primary px-3 text-text-dark transition-colors hover:bg-text-accent/85"
    >
      {children}
    </button>
  );
}

function IconButton({
  onClick,
  title,
  subdued,
  children,
}: {
  onClick: () => void;
  title?: string;
  subdued?: boolean;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      className={`flex size-7 items-center justify-center rounded-[2px] p-1 transition-colors hover:text-text-default ${
        subdued ? "text-text-disabled" : "text-text-subdued"
      }`}
    >
      {children}
    </button>
  );
}

function SearchField({
  value,
  onChange,
  placeholder,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder: string;
}) {
  return (
    <div className="flex items-center gap-2 rounded-[4px] bg-transparent-primary px-3 py-1.5">
      <Search className="size-3.5 shrink-0 text-text-disabled" />
      <input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="gs-text-body-sm-regular min-w-[140px] bg-transparent text-text-default placeholder:text-text-disabled focus:outline-none"
      />
    </div>
  );
}

function Modal({
  onClose,
  width,
  children,
}: {
  onClose: () => void;
  width: string;
  children: React.ReactNode;
}) {
  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        className={`${width} rounded-[12px] border border-border-default bg-background-card shadow-2xl`}
      >
        {children}
      </div>
    </div>
  );
}

function TextInput({
  value,
  onChange,
  onCommit,
  placeholder,
  type = "text",
  className = "",
  autoFocus = false,
}: {
  value: string;
  onChange: (v: string) => void;
  onCommit?: () => void;
  placeholder?: string;
  type?: string;
  className?: string;
  autoFocus?: boolean;
}) {
  return (
    <input
      type={type}
      value={value}
      autoFocus={autoFocus}
      onChange={(e) => onChange(e.target.value)}
      onBlur={onCommit}
      onKeyDown={(e) => {
        if (e.key === "Enter") e.currentTarget.blur();
      }}
      placeholder={placeholder}
      className={`gs-text-body-sm-regular h-[28px] rounded-[2px] border border-border-default bg-background-input px-2.5 text-text-default placeholder:text-text-disabled focus:outline-none ${className}`}
    />
  );
}

// ------------------------------------------------------------------
// HUD dictation bar (/#hud)
// ------------------------------------------------------------------
const WAVE_PEAKS = [8, 13, 17, 11, 16, 9, 15, 18, 12, 8];
const WAVE_DURATIONS = [0.85, 0.97, 1.09];

function Hud({ isRecording, isProcessing }: { isRecording: boolean; isProcessing: boolean }) {
  if (!isRecording && !isProcessing) return null;
  const processing = isProcessing && !isRecording;
  return (
    <div className="flex h-screen w-screen items-center justify-center bg-transparent select-none">
      <div
        className="inline-flex h-[38px] items-center overflow-hidden rounded-[12px] text-text-accent"
        style={{
          backgroundColor: "rgba(13, 13, 13, 0.85)",
          boxShadow:
            "inset 0 0 0 1px rgba(255, 255, 255, 0.1), 0 4px 14px rgba(0, 0, 0, 0.18)",
          backdropFilter: "blur(18px) saturate(160%)",
        }}
      >
        <div
          className={`flex h-full items-center gap-2 pr-3 pl-3 ${processing ? "gs-waveform-processing" : ""}`}
        >
          <AudioLines className="size-[18px] shrink-0" />
          <div className="flex h-[18px] items-center gap-1">
            {WAVE_PEAKS.map((h, i) => (
              <span
                key={i}
                className="gs-wave-bar"
                style={
                  {
                    "--gs-wave-h": `${h}px`,
                    animationDelay: `${-0.13 * i}s`,
                    animationDuration: `${WAVE_DURATIONS[i % 3]}s`,
                  } as React.CSSProperties
                }
              />
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}

// ------------------------------------------------------------------
// Stats helpers
// ------------------------------------------------------------------
function wordCount(text: string): number {
  return text.trim().split(/\s+/).filter(Boolean).length;
}

function formatSaved(ms: number): string {
  const totalMin = Math.round(ms / 60000);
  const h = Math.floor(totalMin / 60);
  const m = totalMin % 60;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m`;
}

function dayKey(d: Date): string {
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
}

function computeStreak(history: HistoryItem[]): number {
  const days = new Set(history.map((h) => dayKey(new Date(h.created_at))));
  let streak = 0;
  const cursor = new Date();
  while (days.has(dayKey(cursor))) {
    streak += 1;
    cursor.setDate(cursor.getDate() - 1);
  }
  return streak;
}

function formatGroupDate(d: Date): string {
  return d.toLocaleDateString("en-US", { month: "short", day: "numeric", year: "numeric" });
}

function groupLabel(d: Date): string {
  const today = new Date();
  const yesterday = new Date();
  yesterday.setDate(yesterday.getDate() - 1);
  if (dayKey(d) === dayKey(today)) return "Today";
  if (dayKey(d) === dayKey(yesterday)) return "Yesterday";
  return formatGroupDate(d);
}

function formatTime(d: Date): string {
  return d.toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });
}

function formatDuration(ms: number): string {
  return `${(ms / 1000).toFixed(1)}s`;
}

function formatClock(seconds: number): string {
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}

// ------------------------------------------------------------------
// Record player modal (fig-history-1)
// ------------------------------------------------------------------
function RecordPlayer({
  item,
  onClose,
  onDelete,
  onRetranscribe,
  retranscribing,
  displayText,
}: {
  item: HistoryItem;
  onClose: () => void;
  onDelete: (id: string) => void;
  onRetranscribe: (id: string) => void;
  retranscribing: boolean;
  displayText: string;
}) {
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const [audioSrc, setAudioSrc] = useState<string | null>(null);
  const [playing, setPlaying] = useState(false);
  const [progress, setProgress] = useState(0);
  const [duration, setDuration] = useState(item.duration_ms / 1000);
  const [copied, setCopied] = useState(false);

  const bars = useMemo(() => {
    // Deterministic pseudo-waveform from the entry id (no real peaks stored).
    let hash = 0;
    for (let i = 0; i < item.id.length; i++) hash = (hash * 31 + item.id.charCodeAt(i)) | 0;
    const out: number[] = [];
    for (let i = 0; i < 120; i++) {
      const x = Math.sin(i * 12.9898 + hash) * 43758.5453;
      const frac = x - Math.floor(x);
      out.push(2 + frac * 15.5);
    }
    return out;
  }, [item.id]);

  const ensureAudio = async (): Promise<HTMLAudioElement | null> => {
    if (audioRef.current) return audioRef.current;
    try {
      const base64 = audioSrc ?? (await invoke<string>("get_audio_base64", { id: item.id }));
      if (!audioSrc) setAudioSrc(base64);
      const audio = new Audio(`data:audio/wav;base64,${base64}`);
      audio.onloadedmetadata = () => {
        if (Number.isFinite(audio.duration)) setDuration(audio.duration);
      };
      audio.ontimeupdate = () => {
        if (audio.duration) setProgress(audio.currentTime / audio.duration);
      };
      audio.onended = () => {
        setPlaying(false);
        setProgress(0);
      };
      audioRef.current = audio;
      return audio;
    } catch (e) {
      console.error("Failed to load audio:", e);
      return null;
    }
  };

  const togglePlay = async () => {
    const audio = await ensureAudio();
    if (!audio) return;
    if (playing) {
      audio.pause();
      setPlaying(false);
    } else {
      await audio.play();
      setPlaying(true);
    }
  };

  useEffect(() => {
    return () => {
      audioRef.current?.pause();
      audioRef.current = null;
    };
  }, []);

  const copyText = () => {
    navigator.clipboard.writeText(displayText);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  const elapsed = progress * duration;
  const playedBars = Math.floor(progress * bars.length);

  return (
    <Modal onClose={onClose} width="w-[640px] max-w-[90vw]">
      <div className="flex flex-col pt-4 pb-4 text-text-default">
        {/* Header */}
        <div className="flex w-full shrink-0 items-center justify-between px-5">
          <div className="flex min-w-0 flex-1 items-center gap-3">
            <div className="flex size-10 shrink-0 items-center justify-center rounded-[4px] bg-transparent-tertiary">
              <Mic className="size-5 text-text-subdued" />
            </div>
            <span className="gs-text-body-md-regular truncate text-text-default">
              {formatGroupDate(new Date(item.created_at))}, {formatTime(new Date(item.created_at))}
            </span>
          </div>
          <div className="flex shrink-0 items-center gap-0.5">
            <IconButton onClick={copyText} title="Copy text" subdued={copied}>
              {copied ? <Check className="size-4 text-text-accent" /> : <Copy className="size-4" />}
            </IconButton>
            <IconButton onClick={() => onRetranscribe(item.id)} title="Retry transcription">
              <RotateCw className={`size-4 ${retranscribing ? "animate-spin" : ""}`} />
            </IconButton>
            <IconButton onClick={() => onDelete(item.id)} title="Delete recording">
              <Trash2 className="size-4" />
            </IconButton>
            <IconButton onClick={onClose} title="Close" subdued>
              <X className="size-4" />
            </IconButton>
          </div>
        </div>

        <div className="mt-4 shrink-0 px-5">
          <div className="h-px w-full bg-border-default" />
        </div>

        {/* Transcript */}
        <div className="max-h-[40vh] min-h-0 overflow-y-auto px-5 py-4">
          <p className="gs-text-body-md-regular text-text-default select-text">{displayText}</p>
        </div>

        {/* Player footer */}
        {item.audio_filename && (
          <div className="shrink-0">
            <div className="px-5">
              <div className="h-px w-full bg-border-default" />
            </div>
            <div className="flex w-full items-center gap-2.5 px-5 pt-3 pb-1">
              <IconButton onClick={togglePlay} title={playing ? "Pause" : "Play"}>
                {playing ? <Pause className="size-4" /> : <Play className="size-4" />}
              </IconButton>
              <span className="gs-text-body-sm-regular w-[4ch] shrink-0 text-text-subdued tabular-nums">
                {formatClock(elapsed)}
              </span>
              <div className="min-w-0 flex-1">
                <div className="flex h-6 w-full items-center justify-center gap-[2px] overflow-hidden">
                  {bars.map((h, i) => (
                    <span
                      key={i}
                      className="shrink-0 rounded-full"
                      style={{
                        width: 2,
                        height: h,
                        backgroundColor:
                          i < playedBars ? "var(--color-text-accent)" : "var(--color-text-disabled)",
                      }}
                    />
                  ))}
                </div>
              </div>
              <span className="gs-text-body-sm-regular w-[4ch] shrink-0 text-right text-text-subdued tabular-nums">
                {formatClock(duration)}
              </span>
            </div>
          </div>
        )}
      </div>
    </Modal>
  );
}

// ------------------------------------------------------------------
// Settings modal
// ------------------------------------------------------------------
function SettingsRow({
  icon: Icon,
  title,
  description,
  children,
}: {
  icon: IconType;
  title: string;
  description: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-4 rounded-lg px-3 py-3">
      <div className="flex min-w-0 flex-1 items-center gap-4">
        <div className="flex size-10 shrink-0 items-center justify-center rounded-[4px] bg-transparent-primary p-2.5 text-text-default">
          <Icon className="size-5" />
        </div>
        <div className="flex min-w-0 flex-col gap-0.5">
          <span className="gs-text-body-md-regular text-text-default">{title}</span>
          <span className="gs-text-body-sm-regular text-text-subdued">{description}</span>
        </div>
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}

function SettingsSection({ label, first }: { label: string; first?: boolean }) {
  return (
    <span
      className={`gs-text-tag px-3 pb-1 text-text-subdued select-none ${first ? "pt-0" : "pt-5"}`}
    >
      {label}
    </span>
  );
}

const KEYCAP_LABELS: Record<string, string> = {
  commandorcontrol: navigator.platform.toLowerCase().includes("mac") ? "⌘" : "Ctrl",
  command: "⌘",
  cmd: "⌘",
  control: "Ctrl",
  ctrl: "Ctrl",
  shift: "⇧",
  alt: "⌥",
  option: "⌥",
  space: "Space",
  spacebar: "Space",
  enter: "↵",
  return: "↵",
};

function Keycaps({ hotkey }: { hotkey: string }) {
  const parts = hotkey.split("+").filter(Boolean);
  return (
    <span className="flex items-center gap-1.5">
      {parts.map((p, i) => {
        const label = KEYCAP_LABELS[p.trim().toLowerCase()] ?? p.trim();
        return (
          <kbd
            key={i}
            className="inline-flex min-w-6 items-center justify-center rounded-[2px] bg-transparent-tertiary px-2 py-1 font-mono text-[12px] leading-4 text-text-subdued"
          >
            {label}
          </kbd>
        );
      })}
    </span>
  );
}

function SettingsModal({
  config,
  onSave,
  onClose,
}: {
  config: TranscriptionConfig;
  onSave: (patch: Partial<TranscriptionConfig>) => void;
  onClose: () => void;
}) {
  const [tab, setTab] = useState<SettingsTab>("dictation");
  const [language, setLanguage] = useState(config.language ?? "");
  const [endpointUrl, setEndpointUrl] = useState(config.endpoint_url);
  const [modelName, setModelName] = useState(config.model_name);
  const [apiKey, setApiKey] = useState(config.api_key);
  const [editingHotkey, setEditingHotkey] = useState(false);
  const [hotkeyDraft, setHotkeyDraft] = useState(config.hotkey);

  const tabs: { id: SettingsTab; label: string; icon: IconType }[] = [
    { id: "dictation", label: "Dictation", icon: SquareMenu },
    { id: "hotkeys", label: "Hotkeys", icon: Command },
    { id: "model", label: "Model", icon: Cpu },
  ];

  return (
    <Modal onClose={onClose} width="w-[784px] max-w-[92vw]">
      <div className="flex max-h-[82vh] flex-col">
        {/* Header */}
        <div className="flex items-center justify-between px-6 pt-5">
          <h2 className="gs-text-heading-md text-text-default">Settings</h2>
          <IconButton onClick={onClose} title="Close settings">
            <X className="size-4" />
          </IconButton>
        </div>

        {/* Body */}
        <div className="flex min-h-[420px] flex-1 gap-6 px-6 pt-4">
          {/* Tab list */}
          <div className="flex w-[220px] shrink-0 flex-col gap-1">
            {tabs.map((t) => {
              const Icon = t.icon;
              const active = tab === t.id;
              return (
                <button
                  key={t.id}
                  type="button"
                  onClick={() => setTab(t.id)}
                  className={`flex h-9 items-center gap-3 rounded-[6px] px-3 transition-colors ${
                    active
                      ? "bg-transparent-primary text-text-default"
                      : "text-text-subdued hover:bg-transparent-secondary hover:text-text-default"
                  }`}
                >
                  <Icon className={`size-4 ${active ? "text-text-accent" : ""}`} />
                  <span className="gs-text-body-md-medium">{t.label}</span>
                </button>
              );
            })}
          </div>

          {/* Content */}
          <div className="flex min-w-0 flex-1 flex-col gap-2 overflow-y-auto pb-2">
            <h3 className="gs-text-heading-sm font-medium text-text-default px-3 capitalize">{tab}</h3>

            {tab === "dictation" && (
              <>
                <SettingsSection label="Language" first />
                <SettingsRow
                  icon={Languages}
                  title="Dictation language"
                  description="The language you speak when dictating"
                >
                  <TextInput
                    value={language}
                    onChange={setLanguage}
                    onCommit={() => onSave({ language: language.trim() || null })}
                    placeholder="Auto"
                    className="w-[140px]"
                  />
                </SettingsRow>
                <SettingsSection label="Behavior" />
                <SettingsRow
                  icon={Volume2}
                  title="Interaction sounds"
                  description="Play audio feedback when recording starts and stops"
                >
                  <Toggle
                    on={config.sound_feedback}
                    onChange={(v) => onSave({ sound_feedback: v })}
                    label="Interaction sounds"
                  />
                </SettingsRow>
              </>
            )}

            {tab === "hotkeys" && (
              <>
                <SettingsSection label="Dictation" first />
                <SettingsRow
                  icon={AudioLines}
                  title="Dictation"
                  description="Press to speak, press again to insert what you said."
                >
                  <div className="flex items-center gap-2">
                    <IconButton
                      onClick={() => {
                        setHotkeyDraft(config.hotkey);
                        setEditingHotkey((v) => !v);
                      }}
                      title="Edit hotkey"
                    >
                      <Pencil className="size-4" />
                    </IconButton>
                    {editingHotkey ? (
                      <TextInput
                        value={hotkeyDraft}
                        onChange={setHotkeyDraft}
                        onCommit={() => {
                          if (hotkeyDraft.trim()) onSave({ hotkey: hotkeyDraft.trim() });
                          setEditingHotkey(false);
                        }}
                        placeholder="CommandOrControl+Shift+Space"
                        className="w-[220px] font-mono"
                        autoFocus
                      />
                    ) : (
                      <Keycaps hotkey={config.hotkey} />
                    )}
                  </div>
                </SettingsRow>
              </>
            )}

            {tab === "model" && (
              <>
                <SettingsSection label="Speech to text" first />
                <SettingsRow
                  icon={Globe}
                  title="STT endpoint URL"
                  description="OpenAI-compatible endpoint"
                >
                  <TextInput
                    value={endpointUrl}
                    onChange={setEndpointUrl}
                    onCommit={() => onSave({ endpoint_url: endpointUrl.trim() })}
                    placeholder="https://api.groq.com/openai/v1/audio/transcriptions"
                    className="w-[200px] font-mono"
                  />
                </SettingsRow>
                <SettingsRow
                  icon={Cpu}
                  title="Model name"
                  description="Whisper model to request"
                >
                  <TextInput
                    value={modelName}
                    onChange={setModelName}
                    onCommit={() => onSave({ model_name: modelName.trim() })}
                    placeholder="whisper-large-v3-turbo"
                    className="w-[180px] font-mono"
                  />
                </SettingsRow>
                <SettingsRow
                  icon={KeyRound}
                  title="API key"
                  description="Bearer token for the endpoint"
                >
                  <TextInput
                    type="password"
                    value={apiKey}
                    onChange={setApiKey}
                    onCommit={() => onSave({ api_key: apiKey.trim() })}
                    placeholder="gsk_... or custom key"
                    className="w-[200px] font-mono"
                  />
                </SettingsRow>
              </>
            )}
          </div>
        </div>

        {/* Footer */}
        <div className="px-6 pt-2 pb-5">
          <span className="gs-text-body-xs-regular text-text-disabled">Version 0.1.0</span>
        </div>
      </div>
    </Modal>
  );
}

// ------------------------------------------------------------------
// Add-entry modal (dictionary word / snippet)
// ------------------------------------------------------------------
function AddEntryModal({
  title,
  fields,
  hint,
  submitLabel,
  onSubmit,
  onClose,
}: {
  title: string;
  fields: {
    label: string;
    value: string;
    onChange: (v: string) => void;
    placeholder: string;
    multiline?: boolean;
  }[];
  hint?: string;
  submitLabel: string;
  onSubmit: () => void;
  onClose: () => void;
}) {
  return (
    <Modal onClose={onClose} width="w-[420px] max-w-[90vw]">
      <div className="flex flex-col gap-4 p-6">
        <h3 className="gs-text-heading-md text-text-default">{title}</h3>
        {fields.map((f, i) => (
          <div key={i} className="flex flex-col gap-1.5">
            <label className="gs-text-body-sm-regular text-text-subdued">{f.label}</label>
            {f.multiline ? (
              <textarea
                value={f.value}
                onChange={(e) => f.onChange(e.target.value)}
                placeholder={f.placeholder}
                rows={3}
                autoFocus={i === 0}
                className="gs-text-body-sm-regular w-full resize-none rounded-[2px] border border-border-default bg-background-input px-2.5 py-2 text-text-default placeholder:text-text-disabled focus:outline-none"
              />
            ) : (
              <input
                value={f.value}
                onChange={(e) => f.onChange(e.target.value)}
                placeholder={f.placeholder}
                autoFocus={i === 0}
                onKeyDown={(e) => {
                  if (e.key === "Enter") onSubmit();
                }}
                className="gs-text-body-sm-regular h-[28px] w-full rounded-[2px] border border-border-default bg-background-input px-2.5 text-text-default placeholder:text-text-disabled focus:outline-none"
              />
            )}
          </div>
        ))}
        {hint && <p className="gs-text-body-xs-regular text-text-disabled">{hint}</p>}
        <div className="flex items-center justify-end gap-2 pt-1">
          <OutlineButton onClick={onClose}>Cancel</OutlineButton>
          <LimeButton onClick={onSubmit}>
            <Plus className="size-3.5" />
            {submitLabel}
          </LimeButton>
        </div>
      </div>
    </Modal>
  );
}

// ------------------------------------------------------------------
// Empty state (04-snippets.png)
// ------------------------------------------------------------------
function EmptyState({
  icon: Icon,
  title,
  description,
  actionLabel,
  onAction,
}: {
  icon: IconType;
  title: string;
  description: string;
  actionLabel: string;
  onAction: () => void;
}) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-3 py-8">
      <div className="flex size-12 items-center justify-center rounded-[8px] bg-transparent-primary">
        <Icon className="size-6 text-text-subdued" />
      </div>
      <span className="gs-text-body-md-medium text-text-default">{title}</span>
      <p className="gs-text-body-sm-regular max-w-[280px] text-center text-text-subdued">
        {description}
      </p>
      <LimeButton onClick={onAction}>
        <Plus className="size-3.5" />
        {actionLabel}
      </LimeButton>
    </div>
  );
}

// ------------------------------------------------------------------
// Header card shared by Formatting / Dictionary / Snippets
// ------------------------------------------------------------------
function HeaderCard({ title, description }: { title: string; description: string }) {
  return (
    <div className="shrink-0 rounded-[8px] bg-background-card p-8">
      <h2 className="gs-text-heading-md text-text-default">{title}</h2>
      <p className="gs-text-body-md-regular mt-1 text-text-subdued">{description}</p>
    </div>
  );
}

// ------------------------------------------------------------------
// Main App
// ------------------------------------------------------------------
export default function App() {
  const isHud = window.location.hash === "#hud";

  const [isRecording, setIsRecording] = useState(false);
  const [isProcessing, setIsProcessing] = useState(false);

  const [page, setPage] = useState<Page>("home");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [config, setConfig] = useState<TranscriptionConfig | null>(null);

  const [history, setHistory] = useState<HistoryItem[]>([]);
  const [dictionary, setDictionary] = useState<DictionaryItem[]>([]);
  const [snippets, setSnippets] = useState<SnippetItem[]>([]);

  const [historyQuery, setHistoryQuery] = useState("");
  const [dictQuery, setDictQuery] = useState("");
  const [openRecordId, setOpenRecordId] = useState<string | null>(null);
  const [retranscribingId, setRetranscribingId] = useState<string | null>(null);
  const [retranscribedText, setRetranscribedText] = useState<Record<string, string>>({});

  const [addWordOpen, setAddWordOpen] = useState(false);
  const [newPhrase, setNewPhrase] = useState("");
  const [newReplacement, setNewReplacement] = useState("");

  const [addSnippetOpen, setAddSnippetOpen] = useState(false);
  const [newTrigger, setNewTrigger] = useState("");
  const [newContent, setNewContent] = useState("");

  // ---- data loaders ----
  const loadConfig = async () => {
    try {
      setConfig(await invoke<TranscriptionConfig>("get_config"));
    } catch (e) {
      console.error(e);
    }
  };
  const loadHistory = async () => {
    try {
      setHistory(await invoke<HistoryItem[]>("get_history", { limit: 500 }));
    } catch (e) {
      console.error(e);
    }
  };
  const loadDictionary = async () => {
    try {
      setDictionary(await invoke<DictionaryItem[]>("get_dictionary"));
    } catch (e) {
      console.error(e);
    }
  };
  const loadSnippets = async () => {
    try {
      setSnippets(await invoke<SnippetItem[]>("get_snippets"));
    } catch (e) {
      console.error(e);
    }
  };

  // ---- events ----
  useEffect(() => {
    const unlistenRec = listen<boolean>("recording-status", (e) => setIsRecording(e.payload));
    const unlistenProc = listen<boolean>("processing-status", (e) => setIsProcessing(e.payload));
    const unlistenDone = listen<string>("transcription-completed", () => {
      if (!isHud) loadHistory();
    });
    invoke<boolean>("get_recording_state").then(setIsRecording).catch(console.error);
    if (!isHud) {
      loadConfig();
      loadHistory();
      loadDictionary();
      loadSnippets();
    }
    return () => {
      unlistenRec.then((f) => f());
      unlistenProc.then((f) => f());
      unlistenDone.then((f) => f());
    };
  }, [isHud]);

  // ---- keyboard shortcuts (main window) ----
  useEffect(() => {
    if (isHud) return;
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === ",") {
        e.preventDefault();
        setSettingsOpen((v) => !v);
        return;
      }
      if (e.key === "Escape") {
        if (addWordOpen) setAddWordOpen(false);
        else if (addSnippetOpen) setAddSnippetOpen(false);
        else if (openRecordId) setOpenRecordId(null);
        else if (settingsOpen) setSettingsOpen(false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [isHud, addWordOpen, addSnippetOpen, openRecordId, settingsOpen]);

  const saveConfig = async (patch: Partial<TranscriptionConfig>) => {
    if (!config) return;
    const newConfig = { ...config, ...patch };
    try {
      await invoke("save_config", { newConfig });
      setConfig(newConfig);
    } catch (e) {
      console.error(e);
    }
  };

  // ---- dictionary actions ----
  const addWord = async () => {
    if (!newPhrase.trim()) return;
    try {
      await invoke("add_dictionary_entry", {
        phrase: newPhrase.trim(),
        replacement: newReplacement.trim(),
      });
      setNewPhrase("");
      setNewReplacement("");
      setAddWordOpen(false);
      loadDictionary();
    } catch (e) {
      console.error(e);
    }
  };
  const deleteWord = async (id: string) => {
    try {
      await invoke("delete_dictionary_entry", { id });
      loadDictionary();
    } catch (e) {
      console.error(e);
    }
  };

  // ---- snippet actions ----
  const addSnippet = async () => {
    if (!newTrigger.trim() || !newContent.trim()) return;
    try {
      await invoke("add_snippet", { trigger: newTrigger.trim(), content: newContent.trim() });
      setNewTrigger("");
      setNewContent("");
      setAddSnippetOpen(false);
      loadSnippets();
    } catch (e) {
      console.error(e);
    }
  };
  const deleteSnippet = async (id: string) => {
    try {
      await invoke("delete_snippet", { id });
      loadSnippets();
    } catch (e) {
      console.error(e);
    }
  };

  // ---- history actions ----
  const deleteHistory = async (id: string) => {
    try {
      await invoke("delete_history_entry", { id });
      setOpenRecordId(null);
      loadHistory();
    } catch (e) {
      console.error(e);
    }
  };
  const retranscribe = async (id: string) => {
    setRetranscribingId(id);
    try {
      const newText = await invoke<string>("retranscribe", { id });
      setRetranscribedText((prev) => ({ ...prev, [id]: newText }));
      loadHistory();
    } catch (e) {
      console.error("Retranscribe failed:", e);
    } finally {
      setRetranscribingId(null);
    }
  };

  // ---- derived: stats ----
  const stats = useMemo(() => {
    const words = history.reduce((a, h) => a + wordCount(h.text), 0);
    const ms = history.reduce((a, h) => a + h.duration_ms, 0);
    const typingMs = (words / 40) * 60000;
    const savedMs = Math.max(0, typingMs - ms);
    const wpm = ms > 0 ? Math.round(words / (ms / 60000)) : 0;
    const streak = computeStreak(history);

    const savedSubtitle =
      savedMs <= 0
        ? "No time saved yet"
        : savedMs < 3600000
          ? "Just getting started"
          : savedMs < 8 * 3600000
            ? "Nice progress"
            : "That's a full day";
    const streakSubtitle =
      streak === 0
        ? "Dictate today to start"
        : streak === 1
          ? "Great start!"
          : streak < 7
            ? "Keep it going!"
            : "On fire!";

    return {
      saved: formatSaved(savedMs),
      savedSubtitle,
      wpm: wpm > 0 ? `${wpm} wpm` : "0 wpm",
      wpmSubtitle: `${(wpm / 40).toFixed(1)}x faster than typing`,
      streak: `${streak} day${streak === 1 ? "" : "s"}`,
      streakSubtitle,
    };
  }, [history]);

  // ---- derived: grouped + filtered history ----
  const historyGroups = useMemo(() => {
    const q = historyQuery.trim().toLowerCase();
    const filtered = q ? history.filter((h) => h.text.toLowerCase().includes(q)) : history;
    const groups: { label: string; date: string; items: HistoryItem[] }[] = [];
    for (const item of filtered) {
      const d = new Date(item.created_at);
      const label = groupLabel(d);
      const last = groups[groups.length - 1];
      if (last && last.label === label) {
        last.items.push(item);
      } else {
        groups.push({ label, date: formatGroupDate(d), items: [item] });
      }
    }
    return groups;
  }, [history, historyQuery]);

  const filteredDictionary = useMemo(() => {
    const q = dictQuery.trim().toLowerCase();
    if (!q) return dictionary;
    return dictionary.filter(
      (d) => d.phrase.toLowerCase().includes(q) || d.replacement.toLowerCase().includes(q),
    );
  }, [dictionary, dictQuery]);

  // ------------------------------------------------------------------
  // HUD window
  // ------------------------------------------------------------------
  if (isHud) {
    return <Hud isRecording={isRecording} isProcessing={isProcessing} />;
  }

  const navItems: { id: Page; label: string; icon: IconType }[] = [
    { id: "home", label: "Home", icon: Home },
    { id: "formatting", label: "Formatting", icon: SquareMenu },
    { id: "dictionary", label: "Dictionary", icon: BookOpen },
    { id: "snippets", label: "Snippets", icon: SquareText },
  ];

  const openRecord = openRecordId ? history.find((h) => h.id === openRecordId) : undefined;

  const statCards = [
    { icon: Timer, label: "Time saved", value: stats.saved, subtitle: stats.savedSubtitle },
    { icon: Zap, label: "Dictation speed", value: stats.wpm, subtitle: stats.wpmSubtitle },
    { icon: Flame, label: "Day streak", value: stats.streak, subtitle: stats.streakSubtitle },
  ];

  const modes = [
    {
      id: "default",
      title: "All apps",
      desc: "The style every dictation gets, in every app.",
    },
    { id: "email", title: "Email", desc: "Professional tone, paragraphs, and polite phrasing." },
    { id: "chat", title: "Chat", desc: "Fast, punchy, concise messaging for Slack and Discord." },
    { id: "code", title: "Code", desc: "Translates spoken programming terminology and syntax." },
    { id: "raw", title: "Raw", desc: "Verbatim output from Whisper without any LLM changes." },
  ];

  // ------------------------------------------------------------------
  // Main window
  // ------------------------------------------------------------------
  return (
    <div className="flex h-screen w-screen flex-col bg-background-primary text-text-default select-none">
      {/* Drag strip for the overlay title bar (traffic lights live here) */}
      <div data-tauri-drag-region className="h-10 w-full shrink-0" />

      <div className="flex min-h-0 flex-1 gap-8 px-4 pb-4 pt-5">
        {/* Sidebar */}
        <aside className="flex w-[224px] shrink-0 flex-col">
          <nav className="flex flex-1 flex-col gap-1">
            {navItems.map((item) => {
              const Icon = item.icon;
              const active = page === item.id;
              return (
                <button
                  key={item.id}
                  type="button"
                  onClick={() => setPage(item.id)}
                  className={`flex h-10 w-full items-center gap-3 rounded-[6px] px-3 transition-colors ${
                    active
                      ? "bg-transparent-primary text-text-default"
                      : "text-text-subdued hover:bg-transparent-secondary hover:text-text-default"
                  }`}
                >
                  <Icon
                    className={`size-[18px] shrink-0 ${active ? "text-text-accent stroke-[#151515]" : ""}`}
                    fill={active ? "currentColor" : "none"}
                  />
                  <span className="gs-text-body-md-medium">{item.label}</span>
                </button>
              );
            })}
          </nav>
          <button
            type="button"
            onClick={() => setSettingsOpen(true)}
            className={`flex h-10 w-full items-center gap-3 rounded-[6px] px-3 transition-colors ${
              settingsOpen
                ? "bg-transparent-primary text-text-default"
                : "text-text-subdued hover:bg-transparent-secondary hover:text-text-default"
            }`}
          >
            <Settings
              className={`size-[18px] shrink-0 ${settingsOpen ? "text-text-accent stroke-[#151515]" : ""}`}
              fill={settingsOpen ? "currentColor" : "none"}
            />
            <span className="gs-text-body-md-medium">Settings</span>
          </button>
        </aside>

        {/* Content */}
        <main className="flex min-w-0 flex-1 flex-col gap-4">
          {/* ---------------- HOME ---------------- */}
          {page === "home" && (
            <>
              <div className="grid shrink-0 grid-cols-3 gap-4">
                {statCards.map((card) => {
                  const Icon = card.icon;
                  return (
                    <div
                      key={card.label}
                      className="flex h-[180px] flex-col rounded-[8px] bg-background-card p-8"
                    >
                      <div className="flex items-center gap-2">
                        <Icon className="size-[18px] text-text-accent" />
                        <span className="gs-text-tag text-text-subdued">{card.label}</span>
                      </div>
                      <div className="mt-auto">
                        <div className="text-[34px] leading-[40px] font-medium text-text-default">
                          {card.value}
                        </div>
                        <div className="gs-text-body-sm-regular mt-2 text-text-subdued">
                          {card.subtitle}
                        </div>
                      </div>
                    </div>
                  );
                })}
              </div>

              {/* Activity card */}
              <div className="flex min-h-0 flex-1 flex-col rounded-[8px] bg-background-card pt-8 pb-8 pl-8">
                <div className="flex min-h-7 shrink-0 items-center gap-1.5 pr-8">
                  <span className="gs-text-tag text-text-default">Activity</span>
                  <div className="flex-1" />
                  <SearchField
                    value={historyQuery}
                    onChange={setHistoryQuery}
                    placeholder="Search transcriptions..."
                  />
                </div>
                <div className="mt-5 flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto pr-8">
                  {historyGroups.length === 0 ? (
                    <EmptyState
                      icon={Mic}
                      title="No dictations yet"
                      description="Your transcriptions will appear here once you start dictating."
                      actionLabel="Open settings"
                      onAction={() => setSettingsOpen(true)}
                    />
                  ) : (
                    historyGroups.map((group) => (
                      <div key={group.label} className="flex shrink-0 flex-col gap-2">
                        <div className="flex items-center gap-2 px-3 py-2">
                          <span className="gs-text-heading-sm text-text-default">{group.label}</span>
                          <span className="gs-text-heading-sm text-text-subdued">{group.date}</span>
                        </div>
                        <div className="flex flex-col gap-2">
                          {group.items.map((item) => (
                            <button
                              key={item.id}
                              type="button"
                              onClick={() => setOpenRecordId(item.id)}
                              className="flex w-full items-center gap-4 rounded-[4px] px-3 py-4 text-left transition-colors hover:bg-transparent-primary"
                            >
                              <div className="flex size-10 shrink-0 items-center justify-center rounded-[8px] bg-transparent-tertiary">
                                <Mic className="size-5 text-text-subdued" />
                              </div>
                              <div className="min-w-0 flex-1">
                                <p className="gs-text-body-md-regular max-w-[52ch] truncate text-text-default">
                                  {item.text}
                                </p>
                                <p className="gs-text-body-md-regular max-w-[52ch] truncate text-text-subdued">
                                  {formatDuration(item.duration_ms)}
                                </p>
                              </div>
                              <span className="gs-text-body-sm-regular flex h-10 shrink-0 items-center whitespace-nowrap text-text-subdued tabular-nums">
                                {formatTime(new Date(item.created_at))}
                              </span>
                            </button>
                          ))}
                        </div>
                      </div>
                    ))
                  )}
                </div>
              </div>
            </>
          )}

          {/* ---------------- FORMATTING ---------------- */}
          {page === "formatting" && config && (
            <>
              <HeaderCard
                title="Make dictations sound like you"
                description="Choose how OpenGlaido rewrites your dictations before pasting them into your apps."
              />
              <div className="flex min-h-0 flex-1 flex-col overflow-y-auto rounded-[8px] bg-background-card p-8">
                <div className="flex min-h-7 shrink-0 items-center gap-1.5">
                  <span className="gs-text-tag text-text-default">Formatting</span>
                </div>
                <div className="mt-4 flex flex-col">
                  {modes.map((m) => (
                    <div
                      key={m.id}
                      className="flex items-center justify-between gap-4 rounded-lg px-3 py-3"
                    >
                      <div className="flex min-w-0 flex-col gap-0.5">
                        <span className="gs-text-body-md-regular text-text-default">{m.title}</span>
                        <span className="gs-text-body-sm-regular text-text-subdued">{m.desc}</span>
                      </div>
                      <Toggle
                        on={config.mode === m.id}
                        onChange={(v) => {
                          if (v) saveConfig({ mode: m.id });
                        }}
                        label={m.title}
                      />
                    </div>
                  ))}
                </div>
              </div>
            </>
          )}

          {/* ---------------- DICTIONARY ---------------- */}
          {page === "dictionary" && (
            <>
              <HeaderCard
                title="Get every word right"
                description="Add names, jargon, and spelling corrections so OpenGlaido gets them right every time."
              />
              <div className="flex min-h-0 flex-1 flex-col rounded-[8px] bg-background-card pt-8 pb-8 pl-8">
                <div className="flex min-h-7 shrink-0 items-center gap-3 pr-8">
                  <span className="gs-text-tag text-text-default">Dictionary</span>
                  <div className="flex-1" />
                  <SearchField
                    value={dictQuery}
                    onChange={setDictQuery}
                    placeholder="Search words..."
                  />
                  <OutlineButton onClick={() => setAddWordOpen(true)}>
                    <Plus className="size-3.5" />
                    Add word
                  </OutlineButton>
                </div>
                <div className="mt-5 flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto pr-8">
                  {filteredDictionary.length === 0 ? (
                    <EmptyState
                      icon={BookOpen}
                      title="No words added"
                      description="Add names, jargon, and spelling corrections so OpenGlaido gets them right every time."
                      actionLabel="Add word"
                      onAction={() => setAddWordOpen(true)}
                    />
                  ) : (
                    filteredDictionary.map((item) => {
                      const hasReplacement =
                        item.replacement.trim() !== "" && item.replacement !== item.phrase;
                      return (
                        <div
                          key={item.id}
                          className="group flex items-center justify-between gap-4 rounded-[4px] px-3 py-2.5 transition-colors hover:bg-transparent-primary"
                        >
                          <div className="flex min-w-0 flex-col gap-0.5">
                            <span className="gs-text-body-md-regular truncate text-text-default">
                              {hasReplacement ? item.replacement : item.phrase}
                            </span>
                            {hasReplacement && (
                              <span className="gs-text-body-xs-regular w-full truncate text-text-subdued">
                                Replaces &quot;{item.phrase}&quot;
                              </span>
                            )}
                          </div>
                          <button
                            type="button"
                            onClick={() => deleteWord(item.id)}
                            title="Delete word"
                            className="shrink-0 text-text-disabled opacity-0 transition-opacity group-hover:opacity-100 hover:text-text-error"
                          >
                            <Trash2 className="size-4" />
                          </button>
                        </div>
                      );
                    })
                  )}
                </div>
              </div>
            </>
          )}

          {/* ---------------- SNIPPETS ---------------- */}
          {page === "snippets" && (
            <>
              <HeaderCard
                title="Expand short phrases into full text"
                description="Say a short trigger phrase and OpenGlaido expands it into a full block of text."
              />
              <div className="flex min-h-0 flex-1 flex-col rounded-[8px] bg-background-card pt-8 pb-8 pl-8">
                <div className="flex min-h-7 shrink-0 items-center gap-3 pr-8">
                  <span className="gs-text-tag text-text-default">Snippets</span>
                  <div className="flex-1" />
                  <OutlineButton onClick={() => setAddSnippetOpen(true)}>
                    <Plus className="size-3.5" />
                    Add snippet
                  </OutlineButton>
                </div>
                <div className="mt-5 flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto pr-8">
                  {snippets.length === 0 ? (
                    <EmptyState
                      icon={SquareText}
                      title="No snippets added"
                      description="Add your first snippet to create a shortcut for phrases you use often."
                      actionLabel="Add snippet"
                      onAction={() => setAddSnippetOpen(true)}
                    />
                  ) : (
                    snippets.map((snip) => (
                      <div
                        key={snip.id}
                        className="group flex items-center justify-between gap-4 rounded-[4px] px-3 py-2.5 transition-colors hover:bg-transparent-primary"
                      >
                        <div className="flex min-w-0 flex-1 items-center gap-3">
                          <span className="gs-text-body-md-medium shrink-0 truncate text-text-default">
                            {snip.trigger}
                          </span>
                          <span className="shrink-0 text-text-disabled">→</span>
                          <span className="gs-text-body-md-regular truncate text-text-subdued">
                            {snip.content}
                          </span>
                        </div>
                        <button
                          type="button"
                          onClick={() => deleteSnippet(snip.id)}
                          title="Delete snippet"
                          className="shrink-0 text-text-disabled opacity-0 transition-opacity group-hover:opacity-100 hover:text-text-error"
                        >
                          <Trash2 className="size-4" />
                        </button>
                      </div>
                    ))
                  )}
                </div>
              </div>
            </>
          )}
        </main>
      </div>

      {/* ---------------- Modals ---------------- */}
      {settingsOpen && config && (
        <SettingsModal config={config} onSave={saveConfig} onClose={() => setSettingsOpen(false)} />
      )}

      {openRecord && (
        <RecordPlayer
          item={openRecord}
          onClose={() => setOpenRecordId(null)}
          onDelete={deleteHistory}
          onRetranscribe={retranscribe}
          retranscribing={retranscribingId === openRecord.id}
          displayText={retranscribedText[openRecord.id] ?? openRecord.text}
        />
      )}

      {addWordOpen && (
        <AddEntryModal
          title="Add word"
          fields={[
            {
              label: "Word or phrase",
              value: newPhrase,
              onChange: setNewPhrase,
              placeholder: "e.g. Kubernetes",
            },
            {
              label: "Replace with (optional)",
              value: newReplacement,
              onChange: setNewReplacement,
              placeholder: "Leave empty to keep the word as-is",
            },
          ]}
          submitLabel="Add word"
          onSubmit={addWord}
          onClose={() => setAddWordOpen(false)}
        />
      )}

      {addSnippetOpen && (
        <AddEntryModal
          title="Add snippet"
          fields={[
            {
              label: "Trigger phrase",
              value: newTrigger,
              onChange: setNewTrigger,
              placeholder: "e.g. my email",
            },
            {
              label: "Expanded text",
              value: newContent,
              onChange: setNewContent,
              placeholder: "e.g. name@domain.com",
              multiline: true,
            },
          ]}
          hint="Supports dynamic tags: {date}, {time}, {clipboard}."
          submitLabel="Add snippet"
          onSubmit={addSnippet}
          onClose={() => setAddSnippetOpen(false)}
        />
      )}
    </div>
  );
}
