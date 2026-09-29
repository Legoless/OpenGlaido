import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  AudioLines,
  AudioWaveform,
  BookOpen,
  Captions,
  Check,
  ChevronDown,
  Clipboard,
  Command,
  Copy,
  Cpu,
  Dock,
  Ellipsis,
  Flame,
  Globe,
  KeyRound,
  Languages,
  MapPin,
  Mic,
  Music,
  Pause,
  Pencil,
  Play,
  Plus,
  Power,
  RotateCw,
  Search,
  Settings,
  ShieldCheck,
  Settings2,
  Timer,
  Trash2,
  TriangleAlert,
  X,
  createLucideIcon,
  type LucideIcon,
} from "lucide-react";

// ------------------------------------------------------------------
// Types (mirror src-tauri/src/transcribe.rs / hotkeys.rs)
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
  hotkey_hold: string;
  hotkey_toggle: string;
  enter_to_stop: boolean;
  input_device?: string | null;
  copy_to_clipboard: boolean;
  bar_location: string;
  launch_at_login: boolean;
  show_in_menu_bar: boolean;
  show_in_dock: boolean;
}

interface HotkeyStatus {
  engine: "native" | "plugin";
  permission_granted: boolean;
  error: string | null;
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
type SettingsTab = "dictation" | "hotkeys" | "general" | "model";
type IconType = LucideIcon;
/** Resolves to an error message (null on success); the caller shows it where the change was made. */
type SaveConfig = (patch: Partial<TranscriptionConfig>) => Promise<string | null>;

const IS_MAC = /Mac/i.test(navigator.userAgent);

// Lucide-based nav glyphs closer to Glaido's. The outline comes first so the inner strokes paint
// on top of it when the active icon is filled.
const HouseDoor = createLucideIcon("house-door", [
  [
    "path",
    {
      d: "M3 10a2 2 0 0 1 .709-1.528l7-6a2 2 0 0 1 2.582 0l7 6A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
      key: "outline",
    },
  ],
  ["path", { d: "M14 21v-5a1 1 0 0 0-1-1h-2a1 1 0 0 0-1 1v5", key: "door" }],
]);
const BookSpine = createLucideIcon("book-spine", [
  [
    "path",
    {
      d: "M20.001 19A2 2 0 0022 17V5a2 2 0 00-1.999-2L16 3.002A5 5 0 0012 5a5 5 0 00-4-2H4a2 2 0 00-2 2v12a2 2 0 001.999 2H8a5 5 0 014 2 5 5 0 014-2z",
      key: "outline",
    },
  ],
  ["path", { d: "M12 5v16", key: "spine" }],
]);
// Angular bolt (lucide's pre-1.0 zap), closer to Glaido's stat icon than the rounded 1.x one.
const Bolt = createLucideIcon("bolt-angular", [
  ["polygon", { points: "13 2 3 14 12 14 11 22 21 10 12 10 13 2", key: "bolt" }],
]);
const SnippetIcon = createLucideIcon("snippet", [
  ["rect", { width: "18", height: "18", x: "3", y: "3", rx: "2", key: "outline" }],
  ["path", { d: "M8 10h7", key: "line-1" }],
  ["path", { d: "M8 14h5", key: "line-2" }],
]);

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
      className="gs-text-body-md-medium inline-flex h-7 shrink-0 items-center gap-2.5 whitespace-nowrap rounded-[2px] border border-border-default pr-2.5 pl-3 text-text-subdued transition-colors hover:text-text-default"
    >
      {children}
    </button>
  );
}

function LimeButton({
  onClick,
  children,
  disabled = false,
}: {
  onClick: () => void;
  children: React.ReactNode;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className="gs-text-body-md-medium inline-flex h-7 shrink-0 items-center gap-2.5 whitespace-nowrap rounded-[2px] bg-button-primary px-2.5 text-text-dark transition-colors hover:bg-text-accent/85 disabled:opacity-40"
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
    <div className="flex h-[30px] w-[200px] shrink-0 items-center gap-2.5 rounded-[4px] bg-transparent-primary px-[13px]">
      <Search className="size-3.5 shrink-0 text-text-subdued" />
      <input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="gs-text-body-sm-regular w-full min-w-0 bg-transparent text-text-default placeholder:text-text-disabled focus:outline-none"
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
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/30"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        className={`${width} rounded-[12px] border border-border-default bg-[#232323]/85 shadow-2xl backdrop-blur-xl`}
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

/** Outlined select (Glaido style) or, with `buttonLabel`, an outlined button that opens the same menu. */
function Dropdown({
  value,
  options,
  onChange,
  className = "",
  buttonLabel,
}: {
  value: string | null;
  options: { value: string | null; label: string }[];
  onChange: (v: string | null) => void;
  className?: string;
  buttonLabel?: string;
}) {
  // Portalled fixed-position menu: never clipped by the scrolling settings column, and not
  // positioned relative to the modal (its backdrop-filter is a containing block for fixed children).
  const [menu, setMenu] = useState<React.CSSProperties | null>(null);
  const openMenu = (e: React.MouseEvent<HTMLButtonElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    const right = window.innerWidth - r.right;
    const openUp = r.bottom + 248 > window.innerHeight;
    setMenu(
      openUp
        ? { right, bottom: window.innerHeight - r.top + 4, minWidth: r.width }
        : { right, top: r.bottom + 4, minWidth: r.width },
    );
  };
  const current = options.find((o) => o.value === value)?.label ?? value ?? "";
  return (
    <>
      {buttonLabel ? (
        <button
          type="button"
          onClick={openMenu}
          className="gs-text-body-md-medium h-[29px] shrink-0 rounded-[2px] border border-border-default px-[9px] whitespace-nowrap text-text-subdued transition-colors hover:text-text-default"
        >
          {buttonLabel}
        </button>
      ) : (
        <button
          type="button"
          onClick={openMenu}
          className={`gs-text-body-md-regular flex h-[29px] items-center justify-between gap-2 rounded-[4px] border border-border-default bg-background-input pr-2 pl-2.5 text-left text-text-default ${className}`}
        >
          <span className="truncate">{current}</span>
          <ChevronDown className="size-3 shrink-0 text-text-subdued" />
        </button>
      )}
      {menu &&
        createPortal(
          <>
            <div className="fixed inset-0 z-[70]" onMouseDown={() => setMenu(null)} />
            <div
              style={menu}
              className="fixed z-[71] flex max-h-[240px] flex-col overflow-y-auto rounded-[6px] border border-border-default bg-[#262626] p-1 shadow-2xl"
            >
              {options.map((o) => (
                <button
                  key={o.value ?? ""}
                  type="button"
                  onClick={() => {
                    setMenu(null);
                    if (o.value !== value) onChange(o.value);
                  }}
                  className="gs-text-body-md-regular flex h-8 shrink-0 items-center justify-between gap-4 rounded-[4px] px-2.5 text-left whitespace-nowrap text-text-default hover:bg-transparent-secondary"
                >
                  {o.label}
                  {o.value === value && <Check className="size-3.5 text-text-accent" />}
                </button>
              ))}
            </div>
          </>,
          document.body,
        )}
    </>
  );
}

// ------------------------------------------------------------------
// HUD dictation bar (/#hud): always-visible pill in a 420x72 window
// ------------------------------------------------------------------
const BAR_COUNT = 10;

function Hud({ isRecording, isProcessing }: { isRecording: boolean; isProcessing: boolean }) {
  const [levels, setLevels] = useState<number[]>(() => Array(BAR_COUNT).fill(0));
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let timer: number | undefined;
    const unlistenLevel = listen<number>("mic-level", (e) =>
      setLevels((prev) => [...prev.slice(1), Math.min(1, Math.max(0, e.payload))]),
    );
    const unlistenError = listen<string>("dictation-error", (e) => {
      setError(e.payload);
      window.clearTimeout(timer);
      timer = window.setTimeout(() => setError(null), 3000);
    });
    return () => {
      window.clearTimeout(timer);
      unlistenLevel.then((f) => f());
      unlistenError.then((f) => f());
    };
  }, []);

  useEffect(() => {
    if (isRecording) setLevels(Array(BAR_COUNT).fill(0));
  }, [isRecording]);

  const state = error ? "error" : isRecording ? "recording" : isProcessing ? "processing" : "idle";
  return (
    <div className="flex h-screen w-screen items-center justify-center bg-transparent select-none">
      <div className="flex h-[38px] max-w-[396px] items-center rounded-[12px] bg-background-primary px-3 text-text-accent shadow-[inset_0_0_0_1px_rgba(255,255,255,0.1),0_4px_14px_rgba(0,0,0,0.18)]">
        <div
          className={`flex min-w-0 items-center gap-2 ${state === "processing" ? "gs-waveform-processing" : ""}`}
        >
          <AudioLines className="size-[18px] shrink-0" />
          {state === "error" ? (
            <span className="gs-text-body-sm-regular truncate text-text-default">{error}</span>
          ) : (
            <div className="flex h-[18px] items-center gap-1">
              {levels.map((v, i) => (
                <span
                  key={i}
                  className={`w-[3px] rounded-full bg-current ${
                    state === "idle" ? "h-[3px] opacity-30" : "transition-[height] duration-75 ease-out"
                  }`}
                  style={
                    state === "recording" ? { height: 4 + v * 14 } : state === "processing" ? { height: 4 } : undefined
                  }
                />
              ))}
            </div>
          )}
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
  note,
  children,
}: {
  icon: IconType;
  title: string;
  description: string;
  /** Error / warning / hint shown under the row. */
  note?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="flex flex-col">
      {/* Wraps the control onto its own line when the window is too narrow. */}
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2 px-3 py-4">
        <div className="flex size-10 shrink-0 items-center justify-center rounded-[4px] bg-transparent-primary text-text-default">
          <Icon className="size-[18px]" />
        </div>
        <div className="flex min-w-[140px] flex-1 flex-col gap-1">
          <span className="gs-text-body-md-regular text-text-default">{title}</span>
          <span className="gs-text-body-sm-regular text-text-subdued">{description}</span>
        </div>
        <div className="ml-auto flex max-w-full items-center justify-end">{children}</div>
      </div>
      {note && <div className="-mt-2 flex flex-col gap-1 pr-3 pb-2 pl-[68px]">{note}</div>}
    </div>
  );
}

function SettingsSection({ label }: { label: string }) {
  return <span className="gs-text-tag px-3 pt-6 pb-2 text-text-subdued select-none">{label}</span>;
}

function NoteText({ tone, children }: { tone: "error" | "warning" | "hint"; children: React.ReactNode }) {
  return (
    <p
      className={`gs-text-body-sm-regular flex items-start gap-1.5 select-text ${
        tone === "error" ? "text-text-error" : "text-text-subdued"
      }`}
    >
      {tone === "warning" && <TriangleAlert className="mt-px size-3.5 shrink-0 text-text-accent" />}
      {children}
    </p>
  );
}

// Hotkey binding strings: see the doc comment in src-tauri/src/hotkeys.rs.
const MODIFIER_CODES = [
  "Fn",
  "ControlLeft",
  "ControlRight",
  "AltLeft",
  "AltRight",
  "ShiftLeft",
  "ShiftRight",
  "MetaLeft",
  "MetaRight",
];

const KEY_LABELS: Record<string, string> = {
  Fn: "fn",
  ControlLeft: "Left ^",
  ControlRight: "Right ^",
  AltLeft: "Left ⌥",
  AltRight: "Right ⌥",
  ShiftLeft: "Left ⇧",
  ShiftRight: "Right ⇧",
  MetaLeft: "Left ⌘",
  MetaRight: "Right ⌘",
  Control: IS_MAC ? "^" : "Ctrl",
  Alt: IS_MAC ? "⌥" : "Alt",
  Shift: "⇧",
  Meta: IS_MAC ? "⌘" : "Win",
  Space: "Spacebar",
  Enter: "Enter",
};

function bindingLabels(binding: string): string[] {
  return binding
    .split("+")
    .filter(Boolean)
    .map((code) => KEY_LABELS[code] ?? code.replace(/^(Key|Digit)(?=.$)/, ""));
}

/** Canonical binding: modifiers in a fixed order, then at most one key. */
function toBinding(codes: string[]): string {
  const key = codes.find((c) => !MODIFIER_CODES.includes(c));
  return [...MODIFIER_CODES.filter((m) => codes.includes(m)), ...(key ? [key] : [])].join("+");
}

function Keycaps({ labels }: { labels: string[] }) {
  return (
    <span className="flex items-center gap-1.5">
      {labels.map((label, i) => (
        <kbd
          key={i}
          className="inline-flex h-6 min-w-6 items-center justify-center rounded-[2px] bg-transparent-tertiary px-[9px] font-mono text-[12px] leading-4 whitespace-nowrap text-text-subdued"
        >
          {label}
        </kbd>
      ))}
    </span>
  );
}

type CaptureResult = { binding: string; warning?: string | null; error?: string };

// start/stop_hotkey_capture are async commands; keep them in call order (StrictMode runs the
// recorder's effect twice in dev: start, stop, start).
let captureQueue: Promise<unknown> = Promise.resolve();
function captureCommand<T>(cmd: "start_hotkey_capture" | "stop_hotkey_capture"): Promise<T> {
  const next = captureQueue.catch(() => {}).then(() => invoke<T>(cmd));
  captureQueue = next;
  return next;
}

/**
 * Records a new binding. Native capture (macOS) streams "hotkey-capture" events; otherwise the
 * webview's KeyboardEvent.code values are used. Esc alone cancels. Capture always stops on unmount.
 */
function HotkeyRecorder({
  onSave,
  onCancel,
}: {
  onSave: (binding: string) => Promise<string | null>;
  onCancel: () => void;
}) {
  const [keys, setKeys] = useState("");
  const [result, setResult] = useState<CaptureResult | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);

  // onCancel only closes the recorder (a state setter in the parent), so the first one is kept.
  useEffect(() => {
    let alive = true;
    let native = false;
    let latest = ""; // a slow check for an older chord must not become the saveable result
    const progress = (binding: string) => {
      latest = binding;
      setKeys(binding);
      setResult(null);
      setSaveError(null);
    };
    const finish = (binding: string) => {
      progress(binding);
      const current = () => alive && latest === binding;
      invoke<string | null>("check_hotkey", { binding })
        .then((warning) => current() && setResult({ binding, warning }))
        .catch((e) => current() && setResult({ binding, error: String(e) }));
    };

    // DOM fallback: chord = keys held when a non-modifier goes down, or the largest
    // modifier-only set once everything is released.
    const down = new Set<string>();
    let combo: string[] = [];
    let finished = false;
    const onKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopImmediatePropagation();
      if (e.repeat) return;
      if (e.code === "Escape" && down.size === 0) return onCancel();
      if (native) return;
      if (down.size === 0) {
        combo = [];
        finished = false;
      }
      if (!combo.includes(e.code)) combo.push(e.code);
      if (MODIFIER_CODES.includes(e.code)) {
        down.add(e.code);
        if (!finished) progress(toBinding(combo));
      } else {
        // Key-ups are not delivered for keys pressed while ⌘ is held, so never track non-modifiers.
        finished = true;
        finish(toBinding([...down, e.code]));
      }
    };
    const onKeyUp = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopImmediatePropagation();
      if (native) return;
      down.delete(e.code);
      if (down.size === 0 && !finished && combo.length > 0) {
        finished = true;
        finish(toBinding(combo));
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("keyup", onKeyUp, true);
    // Leaving the window ends recording: the backend stops native capture on blur too.
    window.addEventListener("blur", onCancel);

    const unlisten = listen<{ keys: string; done: boolean }>("hotkey-capture", (e) => {
      // The native engine reports Esc alone as an empty finished chord.
      if (e.payload.done && !e.payload.keys) return onCancel();
      if (e.payload.done) finish(e.payload.keys);
      else progress(e.payload.keys);
    });
    captureCommand<boolean>("start_hotkey_capture")
      .then((n) => {
        native = n;
      })
      .catch((e) => alive && setSaveError(String(e)));

    return () => {
      alive = false;
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("keyup", onKeyUp, true);
      window.removeEventListener("blur", onCancel);
      unlisten.then((f) => f());
      captureCommand("stop_hotkey_capture").catch(console.error);
    };
  }, []);

  const save = async () => {
    if (!result || result.error) return;
    await captureCommand("stop_hotkey_capture").catch(console.error);
    const err = await onSave(result.binding);
    if (err) {
      setSaveError(err);
      captureCommand("start_hotkey_capture").catch(console.error);
    }
  };

  return (
    <div className="flex flex-col items-end gap-1.5">
      <div className="flex items-center gap-2">
        <div className="flex h-[29px] min-w-[132px] items-center rounded-[4px] border border-border-accent/60 bg-background-input px-[2px]">
          {keys ? (
            <Keycaps labels={bindingLabels(keys)} />
          ) : (
            <span className="gs-text-body-md-regular px-2 text-text-disabled">Press keys…</span>
          )}
        </div>
        <OutlineButton onClick={onCancel}>Cancel</OutlineButton>
        <LimeButton onClick={save} disabled={!result || !!result.error}>
          Save
        </LimeButton>
      </div>
      {result?.error && <NoteText tone="error">{result.error}</NoteText>}
      {result?.warning && <NoteText tone="warning">{result.warning}</NoteText>}
      {saveError && <NoteText tone="error">{saveError}</NoteText>}
    </div>
  );
}

const LANGUAGES: { value: string | null; label: string }[] = [
  { value: null, label: "Auto-detect" },
  { value: "en", label: "English" },
  { value: "es", label: "Spanish" },
  { value: "fr", label: "French" },
  { value: "de", label: "German" },
  { value: "it", label: "Italian" },
  { value: "pt", label: "Portuguese" },
  { value: "nl", label: "Dutch" },
  { value: "pl", label: "Polish" },
  { value: "cs", label: "Czech" },
  { value: "sk", label: "Slovak" },
  { value: "sl", label: "Slovenian" },
  { value: "hr", label: "Croatian" },
  { value: "sr", label: "Serbian" },
  { value: "hu", label: "Hungarian" },
  { value: "ro", label: "Romanian" },
  { value: "bg", label: "Bulgarian" },
  { value: "el", label: "Greek" },
  { value: "ru", label: "Russian" },
  { value: "uk", label: "Ukrainian" },
  { value: "tr", label: "Turkish" },
  { value: "sv", label: "Swedish" },
  { value: "no", label: "Norwegian" },
  { value: "da", label: "Danish" },
  { value: "fi", label: "Finnish" },
  { value: "ar", label: "Arabic" },
  { value: "he", label: "Hebrew" },
  { value: "hi", label: "Hindi" },
  { value: "id", label: "Indonesian" },
  { value: "vi", label: "Vietnamese" },
  { value: "th", label: "Thai" },
  { value: "zh", label: "Chinese" },
  { value: "ja", label: "Japanese" },
  { value: "ko", label: "Korean" },
];

const BAR_LOCATIONS = [
  { value: "bottom", label: "Bottom" },
  { value: "raised", label: "Raised" },
  { value: "high", label: "High" },
];

// Active Glaido icons are filled lime with their inner strokes cut out in the background colour.
const NAV_ICON_CUT = "[&>:not(:first-child)]:fill-[#151515] [&>:not(:first-child)]:stroke-[#151515]";
const TAB_ICON_CUT = "[&>:not(:first-child)]:fill-[#282828] [&>:not(:first-child)]:stroke-[#282828]";

type HotkeyField = "hotkey_hold" | "hotkey_toggle";

function SettingsModal({
  config,
  onSave,
  onClose,
}: {
  config: TranscriptionConfig;
  onSave: SaveConfig;
  onClose: () => void;
}) {
  const [tab, setTab] = useState<SettingsTab>("dictation");
  const [endpointUrl, setEndpointUrl] = useState(config.endpoint_url);
  const [modelName, setModelName] = useState(config.model_name);
  const [apiKey, setApiKey] = useState(config.api_key);
  const [errors, setErrors] = useState<Partial<Record<keyof TranscriptionConfig, string>>>({});
  const [devices, setDevices] = useState<string[]>([]);
  const [hotkeyStatus, setHotkeyStatus] = useState<HotkeyStatus | null>(null);
  const [hotkeyChecks, setHotkeyChecks] = useState<Partial<Record<HotkeyField, CaptureResult>>>({});
  const [recording, setRecording] = useState<HotkeyField | null>(null);

  useEffect(() => {
    invoke<string[]>("list_input_devices").then(setDevices).catch(console.error);
    invoke<HotkeyStatus>("get_hotkey_status").then(setHotkeyStatus).catch(console.error);
    const unlisten = listen<HotkeyStatus>("hotkey-status", (e) => setHotkeyStatus(e.payload));
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // Conflict warnings / platform errors for the active bindings.
  useEffect(() => {
    let alive = true;
    for (const field of ["hotkey_hold", "hotkey_toggle"] as const) {
      const binding = config[field];
      const set = (r: CaptureResult) => alive && setHotkeyChecks((prev) => ({ ...prev, [field]: r }));
      if (!binding) {
        set({ binding });
        continue;
      }
      invoke<string | null>("check_hotkey", { binding })
        .then((warning) => set({ binding, warning }))
        .catch((e) => set({ binding, error: String(e) }));
    }
    return () => {
      alive = false;
    };
  }, [config.hotkey_hold, config.hotkey_toggle]);

  const save = async (patch: Partial<TranscriptionConfig>) => {
    const err = await onSave(patch);
    setErrors((prev) => {
      const next = { ...prev };
      for (const key of Object.keys(patch) as (keyof TranscriptionConfig)[]) {
        if (err) next[key] = err;
        else delete next[key];
      }
      return next;
    });
    return err;
  };
  const errorNote = (key: keyof TranscriptionConfig) =>
    errors[key] && <NoteText tone="error">{errors[key]}</NoteText>;

  const tabs: { id: SettingsTab; label: string; icon: IconType; filled?: boolean }[] = [
    { id: "dictation", label: "Dictation", icon: Captions, filled: true },
    { id: "hotkeys", label: "Hotkeys", icon: Command },
    { id: "general", label: "General", icon: Settings2 },
    { id: "model", label: "Model", icon: Cpu },
  ];

  const deviceOptions = [
    { value: null, label: "System default" },
    ...devices.map((d) => ({ value: d, label: d })),
  ];
  if (config.input_device && !devices.includes(config.input_device)) {
    deviceOptions.push({ value: config.input_device, label: `${config.input_device} (unavailable)` });
  }

  const hotkeyRow = (field: HotkeyField, icon: IconType, title: string, description: string) => {
    const check = hotkeyChecks[field];
    return (
      <SettingsRow
        icon={icon}
        title={title}
        description={description}
        note={
          recording !== field &&
          (check?.error || check?.warning) && (
            <>
              {check?.error && <NoteText tone="error">{check.error}</NoteText>}
              {check?.warning && <NoteText tone="warning">{check.warning}</NoteText>}
            </>
          )
        }
      >
        {recording === field ? (
          <HotkeyRecorder
            onCancel={() => setRecording(null)}
            onSave={async (binding) => {
              const err = await save({ [field]: binding });
              if (!err) setRecording(null);
              return err;
            }}
          />
        ) : (
          <div className="flex items-center gap-2">
            <IconButton onClick={() => setRecording(field)} title="Edit hotkey">
              <Pencil className="size-5" />
            </IconButton>
            {config[field] ? (
              <Keycaps labels={bindingLabels(config[field])} />
            ) : (
              <span className="gs-text-body-md-regular text-text-disabled">Disabled</span>
            )}
          </div>
        )}
      </SettingsRow>
    );
  };

  const usesFn = [config.hotkey_hold, config.hotkey_toggle].some((b) => b.split("+").includes("Fn"));

  return (
    <Modal
      onClose={onClose}
      width="flex h-[calc(100vh-122px)] min-h-[min(500px,calc(100vh-40px))] max-h-[900px] w-[calc(100vw-288px)] min-w-[min(720px,calc(100vw-40px))] max-w-[1200px] flex-col"
    >
      {/* Header */}
      <div className="flex shrink-0 items-center justify-between px-6 pt-6">
        <h2 className="text-[20px] leading-7 font-medium tracking-[0.01em] text-text-default">Settings</h2>
        <IconButton onClick={onClose} title="Close settings">
          <X className="size-3.5" strokeWidth={1.5} />
        </IconButton>
      </div>

      {/* Body */}
      <div className="flex min-h-0 flex-1 gap-12 pt-6 pl-6">
        {/* Tab list */}
        <div className="flex w-[244px] max-w-[22vw] shrink-0 flex-col gap-1">
          {tabs.map((t) => {
            const Icon = t.icon;
            const active = tab === t.id;
            return (
              <button
                key={t.id}
                type="button"
                onClick={() => setTab(t.id)}
                className={`flex h-10 items-center gap-3 rounded-[4px] pl-2.5 transition-colors ${
                  active
                    ? "bg-transparent-primary text-text-default"
                    : "text-text-subdued hover:bg-transparent-secondary hover:text-text-default"
                }`}
              >
                <Icon
                  className={`size-[18px] shrink-0 ${active ? "text-text-accent" : ""} ${active && t.filled ? TAB_ICON_CUT : ""}`}
                  fill={active && t.filled ? "currentColor" : "none"}
                />
                <span className="gs-text-body-md-regular">{t.label}</span>
              </button>
            );
          })}
        </div>

        {/* Content */}
        <div className="flex min-w-0 flex-1 flex-col overflow-y-auto pr-[44px] pb-6 [scrollbar-gutter:stable]">
          <h3 className="gs-text-heading-md text-text-default">
            {tabs.find((t) => t.id === tab)?.label}
          </h3>

          {tab === "dictation" && (
            <>
              <SettingsSection label="Input" />
              <SettingsRow
                icon={Mic}
                title="Microphone"
                description="Select your input device"
                note={errorNote("input_device")}
              >
                <Dropdown
                  value={config.input_device ?? null}
                  options={deviceOptions}
                  onChange={(v) => save({ input_device: v })}
                  className="w-[346px] max-w-full"
                />
              </SettingsRow>
              <SettingsSection label="Language" />
              <SettingsRow
                icon={Languages}
                title="Dictation language"
                description="The language you speak when dictating"
                note={errorNote("language")}
              >
                <div className="flex min-w-0 items-center gap-3">
                  <span className="gs-text-body-md-regular min-w-0 truncate text-text-subdued">
                    {LANGUAGES.find((l) => l.value === (config.language || null))?.label ?? config.language}
                  </span>
                  <Dropdown
                    value={config.language || null}
                    options={LANGUAGES}
                    onChange={(v) => save({ language: v })}
                    buttonLabel="Change"
                  />
                </div>
              </SettingsRow>
              <SettingsSection label="Behavior" />
              <SettingsRow
                icon={Clipboard}
                title="Copy to clipboard"
                description="Also copy transcribed text to the clipboard after pasting"
                note={errorNote("copy_to_clipboard")}
              >
                <Toggle
                  on={config.copy_to_clipboard}
                  onChange={(v) => save({ copy_to_clipboard: v })}
                  label="Copy to clipboard"
                />
              </SettingsRow>
              <SettingsRow
                icon={Music}
                title="Interaction sounds"
                description="Play audio feedback when recording starts and stops"
                note={errorNote("sound_feedback")}
              >
                <Toggle
                  on={config.sound_feedback}
                  onChange={(v) => save({ sound_feedback: v })}
                  label="Interaction sounds"
                />
              </SettingsRow>
              <SettingsRow
                icon={MapPin}
                title="Dictation bar location"
                description="Choose how high the dictation bar appears on screen"
                note={errorNote("bar_location")}
              >
                <Dropdown
                  value={config.bar_location}
                  options={BAR_LOCATIONS}
                  onChange={(v) => v && save({ bar_location: v })}
                  className="w-[132px]"
                />
              </SettingsRow>
            </>
          )}

          {tab === "hotkeys" && (
            <>
              {hotkeyStatus && (!hotkeyStatus.permission_granted || hotkeyStatus.error) && (
                <div className="mt-4 flex flex-wrap items-center gap-3 rounded-[6px] border border-border-default bg-transparent-primary px-3 py-2.5">
                  <TriangleAlert className="size-4 shrink-0 text-text-accent" />
                  <div className="flex min-w-[200px] flex-1 flex-col">
                    {!hotkeyStatus.permission_granted && (
                      <span className="gs-text-body-md-regular text-text-default">
                        OpenGlaido needs Accessibility access to detect hotkeys.
                      </span>
                    )}
                    {hotkeyStatus.error && (
                      <span className="gs-text-body-sm-regular text-text-error select-text">
                        {hotkeyStatus.error}
                      </span>
                    )}
                  </div>
                  {!hotkeyStatus.permission_granted && (
                    <OutlineButton
                      onClick={() => invoke("open_accessibility_settings").catch(console.error)}
                    >
                      Open Accessibility settings
                    </OutlineButton>
                  )}
                </div>
              )}
              <SettingsSection label="Dictation" />
              {hotkeyRow("hotkey_hold", AudioLines, "Dictation", "Hold to speak. OpenGlaido inserts what you say.")}
              {hotkeyRow(
                "hotkey_toggle",
                AudioWaveform,
                "Hands-free dictation",
                "Press once to dictate hands-free. Press again to stop.",
              )}
              {usesFn && (
                <div className="pr-3 pb-2 pl-[68px]">
                  <NoteText tone="hint">
                    Set System Settings › Keyboard › “Press 🌐 key to” to “Do Nothing” so fn doesn't open
                    the emoji picker.
                  </NoteText>
                </div>
              )}
              <SettingsSection label="App" />
              <SettingsRow
                icon={Command}
                title="Enter to stop and paste"
                description="While dictating hands-free, Enter stops the session and pastes the result"
                note={errorNote("enter_to_stop")}
              >
                <Toggle
                  on={config.enter_to_stop}
                  onChange={(v) => save({ enter_to_stop: v })}
                  label="Enter to stop and paste"
                />
              </SettingsRow>
              <SettingsRow icon={Settings} title="Settings" description="Open this window">
                <Keycaps labels={[IS_MAC ? "⌘" : "Ctrl", ","]} />
              </SettingsRow>
            </>
          )}

          {tab === "general" && (
            <>
              <SettingsSection label="System" />
              <SettingsRow
                icon={Power}
                title="Launch app at login"
                description="Start OpenGlaido automatically when you log in"
                note={errorNote("launch_at_login")}
              >
                <Toggle
                  on={config.launch_at_login}
                  onChange={(v) => save({ launch_at_login: v })}
                  label="Launch app at login"
                />
              </SettingsRow>
              <SettingsRow
                icon={Ellipsis}
                title="Show in Menu bar"
                description="Display the OpenGlaido icon in the menu bar"
                note={errorNote("show_in_menu_bar")}
              >
                <Toggle
                  on={config.show_in_menu_bar}
                  onChange={(v) => save({ show_in_menu_bar: v })}
                  label="Show in Menu bar"
                />
              </SettingsRow>
              <SettingsRow
                icon={Dock}
                title="Show in Dock"
                description="Keep OpenGlaido visible in the Dock and app switcher"
                note={errorNote("show_in_dock")}
              >
                <Toggle
                  on={config.show_in_dock}
                  onChange={(v) => save({ show_in_dock: v })}
                  label="Show in Dock"
                />
              </SettingsRow>
            </>
          )}

          {tab === "model" && (
            <>
              <SettingsSection label="Speech to text" />
              <SettingsRow
                icon={Globe}
                title="STT endpoint URL"
                description="OpenAI-compatible endpoint"
                note={errorNote("endpoint_url")}
              >
                <TextInput
                  value={endpointUrl}
                  onChange={setEndpointUrl}
                  onCommit={() => save({ endpoint_url: endpointUrl.trim() })}
                  placeholder="https://api.groq.com/openai/v1/audio/transcriptions"
                  className="w-[200px]"
                />
              </SettingsRow>
              <SettingsRow
                icon={Cpu}
                title="Model name"
                description="Whisper model to request"
                note={errorNote("model_name")}
              >
                <TextInput
                  value={modelName}
                  onChange={setModelName}
                  onCommit={() => save({ model_name: modelName.trim() })}
                  placeholder="whisper-large-v3-turbo"
                  className="w-[180px]"
                />
              </SettingsRow>
              <SettingsRow
                icon={KeyRound}
                title="API key"
                description="Bearer token for the endpoint"
                note={errorNote("api_key")}
              >
                <TextInput
                  type="password"
                  value={apiKey}
                  onChange={setApiKey}
                  onCommit={() => save({ api_key: apiKey.trim() })}
                  placeholder="gsk_... or custom key"
                  className="w-[200px]"
                />
              </SettingsRow>
            </>
          )}
        </div>
      </div>

      {/* Footer */}
      <div className="shrink-0 pt-[30px] pb-[22px] pl-[34px]">
        <span className="gs-text-body-xs-regular text-text-disabled">Version 0.1.0</span>
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
            <Plus className="size-3" />
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
    <div className="flex flex-1 flex-col items-center justify-center pt-6 pb-8">
      <div className="flex size-12 items-center justify-center rounded-[6px] bg-transparent-primary">
        <Icon className="size-5 text-text-subdued" strokeWidth={1.5} />
      </div>
      <span className="gs-text-heading-md mt-[17px] text-text-default">{title}</span>
      <p className="gs-text-body-md-regular mt-[7px] max-w-[320px] text-center text-text-subdued">
        {description}
      </p>
      <div className="mt-5">
        <LimeButton onClick={onAction}>
          <Plus className="size-3" />
          {actionLabel}
        </LimeButton>
      </div>
    </div>
  );
}

// ------------------------------------------------------------------
// Header card shared by Formatting / Dictionary / Snippets
// ------------------------------------------------------------------
function HeaderCard({ title, description }: { title: string; description: string }) {
  return (
    <div className="flex shrink-0 flex-col gap-2 rounded-[8px] bg-background-card p-8">
      <h2 className="gs-text-heading-md text-text-default">{title}</h2>
      <p className="gs-text-body-md-regular text-text-subdued">{description}</p>
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

  const [formattingError, setFormattingError] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);

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

  // ---- dictation errors -> toast (main window; the HUD shows its own error pill) ----
  useEffect(() => {
    if (isHud) return;
    let timer: number | undefined;
    const unlisten = listen<string>("dictation-error", (e) => {
      setToast(e.payload);
      window.clearTimeout(timer);
      timer = window.setTimeout(() => setToast(null), 5000);
    });
    return () => {
      window.clearTimeout(timer);
      unlisten.then((f) => f());
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

  // Optimistic: show the change right away, then take the config the backend applied (or revert).
  const saveConfig: SaveConfig = async (patch) => {
    if (!config) return "Settings are not loaded yet";
    const newConfig = { ...config, ...patch };
    setConfig(newConfig);
    try {
      setConfig(await invoke<TranscriptionConfig>("save_config", { newConfig }));
      return null;
    } catch (e) {
      console.error(e);
      setConfig(config);
      return String(e);
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
    { id: "home", label: "Home", icon: HouseDoor },
    { id: "formatting", label: "Formatting", icon: Captions },
    { id: "dictionary", label: "Dictionary", icon: BookSpine },
    { id: "snippets", label: "Snippets", icon: SnippetIcon },
  ];

  const openRecord = openRecordId ? history.find((h) => h.id === openRecordId) : undefined;

  const statCards = [
    { icon: Timer, label: "Time saved", value: stats.saved, subtitle: stats.savedSubtitle },
    { icon: Bolt, label: "Dictation speed", value: stats.wpm, subtitle: stats.wpmSubtitle },
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
          <nav className="flex flex-1 flex-col gap-0.5">
            {navItems.map((item) => {
              const Icon = item.icon;
              const active = page === item.id;
              return (
                <button
                  key={item.id}
                  type="button"
                  onClick={() => setPage(item.id)}
                  className={`flex h-10 w-full shrink-0 items-center gap-2.5 rounded-[4px] px-2.5 transition-colors ${
                    item.id === "home" ? "mb-3" : ""
                  } ${
                    active
                      ? "bg-transparent-primary text-text-default"
                      : "text-text-subdued hover:bg-transparent-secondary hover:text-text-default"
                  }`}
                >
                  <Icon
                    className={`size-5 shrink-0 ${active ? `text-text-accent ${NAV_ICON_CUT}` : ""}`}
                    fill={active ? "currentColor" : "none"}
                  />
                  <span className="gs-text-body-md-regular">{item.label}</span>
                </button>
              );
            })}
          </nav>
          <button
            type="button"
            onClick={() => setSettingsOpen(true)}
            className="flex h-10 w-full items-center gap-2.5 rounded-[4px] px-2.5 text-text-subdued transition-colors hover:bg-transparent-secondary hover:text-text-default"
          >
            <Settings className="size-5 shrink-0" />
            <span className="gs-text-body-md-regular">Settings</span>
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
                      className="flex h-[179px] flex-col rounded-[8px] bg-background-card p-[30px]"
                    >
                      <div className="flex items-center gap-[9px]">
                        <Icon className={`size-5 text-text-accent ${card.icon === Timer ? "-scale-x-100" : ""}`} />
                        <span className="gs-text-tag text-text-default">{card.label}</span>
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
                <div className="flex min-h-7 shrink-0 items-center gap-2 pr-8">
                  <span className="gs-text-tag text-text-default">Activity</span>
                  <ShieldCheck className="size-3.5 text-text-disabled" />
                  <div className="flex-1" />
                  <SearchField
                    value={historyQuery}
                    onChange={setHistoryQuery}
                    placeholder="Search transcriptions..."
                  />
                </div>
                <div className="mt-5 flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto pr-[18px] [scrollbar-gutter:stable]">
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
                      <div key={group.label} className="flex shrink-0 flex-col gap-6">
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
                              className="flex w-full items-center gap-5 rounded-[4px] px-3 py-4 text-left transition-colors hover:bg-transparent-primary"
                            >
                              <div className="flex size-9 shrink-0 items-center justify-center rounded-[8px] bg-transparent-tertiary">
                                <Mic className="size-5 text-text-subdued" />
                              </div>
                              <div className="min-w-0 flex-1">
                                <p className="gs-text-body-md-regular max-w-[436px] truncate text-text-default">
                                  {item.text}
                                </p>
                                <p className="gs-text-body-md-regular max-w-[436px] truncate text-text-subdued">
                                  {formatDuration(item.duration_ms)}
                                </p>
                              </div>
                              <span className="gs-text-body-sm-regular flex h-10 shrink-0 items-center whitespace-nowrap text-text-subdued">
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
                <div className="mt-5 flex flex-col">
                  {modes.map((m) => (
                    <div
                      key={m.id}
                      className="flex min-h-[60px] items-center justify-between gap-4 rounded-[4px] px-3 py-2.5"
                    >
                      <div className="flex min-w-0 flex-col">
                        <span className="gs-text-body-md-regular text-text-default">{m.title}</span>
                        <span className="gs-text-body-md-regular truncate text-text-subdued">{m.desc}</span>
                      </div>
                      <Toggle
                        on={config.mode === m.id}
                        onChange={async (v) => {
                          if (v) setFormattingError(await saveConfig({ mode: m.id }));
                        }}
                        label={m.title}
                      />
                    </div>
                  ))}
                  {formattingError && (
                    <p className="gs-text-body-sm-regular px-3 pt-2 text-text-error select-text">
                      {formattingError}
                    </p>
                  )}
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
                    <Plus className="size-3" />
                    Add word
                  </OutlineButton>
                </div>
                <div className="mt-5 flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto pr-[18px] [scrollbar-gutter:stable]">
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
                          className="group flex items-center justify-between gap-4 rounded-[4px] px-3 py-[14px] transition-colors hover:bg-transparent-primary"
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
                    <Plus className="size-3" />
                    Add snippet
                  </OutlineButton>
                </div>
                <div className="mt-5 flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto pr-[18px] [scrollbar-gutter:stable]">
                  {snippets.length === 0 ? (
                    <EmptyState
                      icon={SnippetIcon}
                      title="No snippets added"
                      description="Add your first snippet to create a shortcut for phrases you use often."
                      actionLabel="Add snippet"
                      onAction={() => setAddSnippetOpen(true)}
                    />
                  ) : (
                    snippets.map((snip) => (
                      <div
                        key={snip.id}
                        className="group flex items-center justify-between gap-4 rounded-[4px] px-3 py-[14px] transition-colors hover:bg-transparent-primary"
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

      {toast && (
        <div className="fixed right-4 bottom-4 z-[80] flex w-[360px] max-w-[calc(100vw-32px)] items-start gap-3 rounded-[8px] border border-border-default bg-background-card p-4 shadow-2xl">
          <TriangleAlert className="mt-0.5 size-4 shrink-0 text-text-error" />
          <p className="gs-text-body-md-regular min-w-0 flex-1 break-words text-text-default select-text">
            {toast}
          </p>
          <button
            type="button"
            onClick={() => setToast(null)}
            title="Dismiss"
            className="shrink-0 text-text-disabled transition-colors hover:text-text-default"
          >
            <X className="size-3.5" />
          </button>
        </div>
      )}
    </div>
  );
}
