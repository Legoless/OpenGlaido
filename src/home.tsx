import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check, Copy, Flame, Mic, Pause, Play, RotateCw, ShieldCheck, Sparkles, Timer, Trash2, X } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { HistoryItem } from "./types";
import { Bolt, EmptyState, IconButton, Modal, OutlineButton, SearchField } from "./ui";
import { locale, t } from "./i18n";
import { Markdown } from "./command-window";

// ------------------------------------------------------------------
// App icons (get_app_icon), cached for the session
// ------------------------------------------------------------------
const iconCache = new Map<string, Promise<string | null>>();

function appIcon(bundleId: string): Promise<string | null> {
  let hit = iconCache.get(bundleId);
  if (!hit) {
    hit = invoke<string | null>("get_app_icon", { bundleId }).catch(() => null);
    iconCache.set(bundleId, hit);
  }
  return hit;
}

/** An app's icon by bundle id; `fallback` renders while loading or when there is none. */
export function BundleIcon({ bundleId, size = 36, fallback }: { bundleId?: string | null; size?: number; fallback: ReactNode }) {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    setSrc(null);
    if (bundleId) appIcon(bundleId).then((s) => alive && setSrc(s));
    return () => {
      alive = false;
    };
  }, [bundleId]);
  if (src) return <img src={src} alt="" width={size} height={size} className="shrink-0 rounded-[8px]" draggable={false} />;
  return <>{fallback}</>;
}

/** The target app's icon in a 36px tile; mic (dictation) or sparkles (command) when unknown. */
export function AppIcon({ item, size = 36 }: { item: HistoryItem; size?: number }) {
  const Fallback = item.kind === "command" ? Sparkles : Mic;
  return (
    <BundleIcon
      bundleId={item.app_bundle_id}
      size={size}
      fallback={
        <div style={{ width: size, height: size }} className="flex shrink-0 items-center justify-center rounded-[8px] bg-transparent-tertiary">
          <Fallback className="size-5 text-text-subdued" />
        </div>
      }
    />
  );
}

/** Row title: the dictated text, the command instruction, or the failure notice. */
export function itemTitle(item: HistoryItem): string {
  if (item.status === "failed" && !item.text) return t("Transcription failed. Your audio is safe.");
  return item.text;
}

/** One line of a markdown answer, without the markup. */
function plainPreview(markdown: string): string {
  return markdown.replace(/```[^\n]*\n?/g, "").replace(/[*_`#]+/g, "").replace(/\s+/g, " ").trim().slice(0, 140);
}

/** Row subtitle: target app (and what the row is). */
export function itemSubtitle(item: HistoryItem): string {
  const app = item.app_name || t("Unknown app");
  if (item.kind === "command") {
    if (item.status === "running") return t("Command · working…");
    if (item.status === "failed") return t("Command failed · {app}", { app });
    return item.answer ? plainPreview(item.answer) : t("Command · {app}", { app });
  }
  if (item.status === "failed") return item.error ? t(item.error) : app;
  return app;
}

// ------------------------------------------------------------------
// Stats helpers
// ------------------------------------------------------------------
export function wordCount(text: string): number {
  return text.trim().split(/\s+/).filter(Boolean).length;
}

export function formatSaved(ms: number): string {
  const totalMin = Math.round(ms / 60000);
  const h = Math.floor(totalMin / 60);
  const m = totalMin % 60;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m`;
}

export function dayKey(d: Date): string {
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
}

export function computeStreak(history: HistoryItem[]): number {
  const days = new Set(history.map((h) => dayKey(new Date(h.created_at))));
  let streak = 0;
  const cursor = new Date();
  while (days.has(dayKey(cursor))) {
    streak += 1;
    cursor.setDate(cursor.getDate() - 1);
  }
  return streak;
}

export function formatGroupDate(d: Date): string {
  return d.toLocaleDateString(locale(), { month: "short", day: "numeric", year: "numeric" });
}

export function groupLabel(d: Date): string {
  const today = new Date();
  const yesterday = new Date();
  yesterday.setDate(yesterday.getDate() - 1);
  if (dayKey(d) === dayKey(today)) return t("Today");
  if (dayKey(d) === dayKey(yesterday)) return t("Yesterday");
  return formatGroupDate(d);
}

export function formatTime(d: Date): string {
  return d.toLocaleTimeString(locale() === "en" ? "en-GB" : locale(), { hour: "2-digit", minute: "2-digit" });
}

export function formatDuration(ms: number): string {
  return `${(ms / 1000).toFixed(1)}s`;
}

export function formatClock(seconds: number): string {
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
  const isCommand = item.kind === "command";
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
    navigator.clipboard.writeText(isCommand && item.answer ? item.answer : displayText);
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
            <AppIcon item={item} size={40} />
            <div className="flex min-w-0 flex-col">
              <span className="gs-text-body-md-regular truncate text-text-default">
                {formatGroupDate(new Date(item.created_at))}, {formatTime(new Date(item.created_at))}
              </span>
              <span className="gs-text-body-sm-regular truncate text-text-subdued">
                {item.app_name || t("Unknown app")}
                {isCommand ? ` · ${t("Command")}` : ""}
              </span>
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-0.5">
            <IconButton onClick={copyText} title={t("Copy text")} subdued={copied}>
              {copied ? <Check className="size-4 text-text-accent" /> : <Copy className="size-4" />}
            </IconButton>
            {item.audio_filename && !isCommand && (
              <IconButton onClick={() => onRetranscribe(item.id)} title={t("Transcribe this recording again")}>
                <RotateCw className={`size-4 ${retranscribing ? "animate-spin" : ""}`} />
              </IconButton>
            )}
            <IconButton onClick={() => onDelete(item.id)} title={t("Delete recording")}>
              <Trash2 className="size-4" />
            </IconButton>
            <IconButton onClick={onClose} title={t("Close")} subdued>
              <X className="size-4" />
            </IconButton>
          </div>
        </div>

        <div className="mt-4 shrink-0 px-5">
          <div className="h-px w-full bg-border-default" />
        </div>

        {/* Transcript */}
        <div className="flex max-h-[40vh] min-h-0 flex-col gap-3 overflow-y-auto px-5 py-4">
          {item.status === "failed" && (
            <div className="flex items-center justify-between gap-3 rounded-[4px] bg-transparent-secondary px-3 py-2.5">
              <div className="min-w-0">
                <p className="gs-text-body-md-medium text-text-default">{t("Transcription failed. Your audio is safe.")}</p>
                {item.error && <p className="gs-text-body-sm-regular text-text-subdued">{t(item.error)}</p>}
              </div>
              {item.audio_filename && !isCommand && (
                <OutlineButton onClick={() => onRetranscribe(item.id)}>{retranscribing ? t("Retrying…") : t("Retry")}</OutlineButton>
              )}
            </div>
          )}
          {displayText && (
            <p className={`gs-text-body-md-regular select-text ${isCommand ? "text-text-subdued" : "text-text-default"}`}>{displayText}</p>
          )}
          {isCommand && item.answer && <Markdown text={item.answer} />}
          {item.sources.length > 0 && (
            <div className="flex flex-col gap-1">
              <span className="gs-text-tag text-text-subdued">{t("Sources")}</span>
              {item.sources.map((s, i) => (
                <button
                  key={s.url + i}
                  type="button"
                  onClick={() => openUrl(s.url).catch(console.error)}
                  className="gs-text-body-sm-regular truncate text-left text-text-accent hover:underline"
                >
                  [{i + 1}] {s.title || s.url}
                </button>
              ))}
            </div>
          )}
        </div>

        {/* Player footer */}
        {item.audio_filename && (
          <div className="shrink-0">
            <div className="px-5">
              <div className="h-px w-full bg-border-default" />
            </div>
            <div className="flex w-full items-center gap-2.5 px-5 pt-3 pb-1">
              <IconButton onClick={togglePlay} title={playing ? t("Pause") : t("Play")}>
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
// Home page: stats + Activity (history) + record player
// ------------------------------------------------------------------
export function Home({
  history,
  onHistoryChanged,
  openRecordId,
  extraRecord,
  onOpenRecord,
  onOpenSettings,
}: {
  history: HistoryItem[];
  /** A record opened from ⌘K that is older than the loaded page. */
  extraRecord?: HistoryItem | null;
  /** Reloads history after a change made here. */
  onHistoryChanged: () => void;
  openRecordId: string | null;
  onOpenRecord: (id: string | null) => void;
  onOpenSettings: () => void;
}) {
  const [historyQuery, setHistoryQuery] = useState("");
  const [retranscribingId, setRetranscribingId] = useState<string | null>(null);

  // ---- history actions ----
  const deleteHistory = async (id: string) => {
    try {
      await invoke("delete_history_entry", { id });
      onOpenRecord(null);
      onHistoryChanged();
    } catch (e) {
      console.error(e);
    }
  };
  const retranscribe = async (id: string) => {
    setRetranscribingId(id);
    try {
      await invoke<HistoryItem>("retranscribe", { id });
      onHistoryChanged();
    } catch (e) {
      console.error("Retranscribe failed:", e);
      onHistoryChanged();
    } finally {
      setRetranscribingId(null);
    }
  };

  // ---- derived: stats ----
  const stats = useMemo(() => {
    const dictations = history.filter((h) => h.kind !== "command" && h.status === "ok");
    const words = dictations.reduce((a, h) => a + wordCount(h.text), 0);
    const ms = dictations.reduce((a, h) => a + h.duration_ms, 0);
    const typingMs = (words / 40) * 60000;
    const savedMs = Math.max(0, typingMs - ms);
    const wpm = ms > 0 ? Math.round(words / (ms / 60000)) : 0;
    const streak = computeStreak(dictations);

    const savedSubtitle =
      savedMs <= 0
        ? t("No time saved yet")
        : savedMs < 3600000
          ? t("Just getting started")
          : savedMs < 8 * 3600000
            ? t("Nice progress")
            : t("That's a full day");
    const streakSubtitle =
      streak === 0
        ? t("Dictate today to start")
        : streak === 1
          ? t("Great start!")
          : streak < 7
            ? t("Keep it going!")
            : t("On fire!");

    return {
      saved: formatSaved(savedMs),
      savedSubtitle,
      wpm: t("{n} wpm", { n: wpm }),
      wpmSubtitle: t("{x}x faster than typing", { x: (wpm / 40).toFixed(1) }),
      streak: streak === 1 ? t("1 day") : t("{n} days", { n: streak }),
      streakSubtitle,
    };
  }, [history, locale()]);

  // ---- derived: grouped + filtered history ----
  const historyGroups = useMemo(() => {
    const q = historyQuery.trim().toLowerCase();
    const filtered = q
      ? history.filter((h) => [h.text, h.answer ?? "", h.app_name ?? ""].some((s) => s.toLowerCase().includes(q)))
      : history;
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
    // The date: "Today" becomes "Yesterday" after midnight.
  }, [history, historyQuery, locale(), new Date().toDateString()]);

  const openRecord = openRecordId
    ? (history.find((h) => h.id === openRecordId) ?? (extraRecord?.id === openRecordId ? extraRecord : undefined))
    : undefined;

  const statCards = [
    { icon: Timer, label: t("Time saved"), value: stats.saved, subtitle: stats.savedSubtitle },
    { icon: Bolt, label: t("Dictation speed"), value: stats.wpm, subtitle: stats.wpmSubtitle },
    { icon: Flame, label: t("Day streak"), value: stats.streak, subtitle: stats.streakSubtitle },
  ];

  return (
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
                <span className="gs-text-tag min-w-0 truncate whitespace-nowrap text-text-default">{card.label}</span>
              </div>
              <div className="mt-auto">
                <div className="truncate whitespace-nowrap text-[34px] leading-[40px] font-medium text-text-default">
                  {card.value}
                </div>
                <div className="gs-text-body-sm-regular mt-2 truncate whitespace-nowrap text-text-subdued">
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
          <span className="gs-text-tag text-text-default">{t("Activity")}</span>
          <ShieldCheck className="size-3.5 text-text-disabled" />
          <div className="flex-1" />
          <SearchField
            value={historyQuery}
            onChange={setHistoryQuery}
            placeholder={t("Search transcriptions...")}
          />
        </div>
        <div className="mt-5 flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto pr-[18px] [scrollbar-gutter:stable]">
          {historyGroups.length === 0 ? (
            <EmptyState
              icon={Mic}
              title={t("No dictations yet")}
              description={t("Your transcriptions will appear here once you start dictating.")}
              actionLabel={t("Open settings")}
              onAction={onOpenSettings}
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
                    <div
                      key={item.id}
                      role="button"
                      tabIndex={0}
                      onClick={() => onOpenRecord(item.id)}
                      onKeyDown={(e) => e.key === "Enter" && onOpenRecord(item.id)}
                      className="flex w-full cursor-default items-center gap-5 rounded-[4px] px-3 py-4 text-left transition-colors hover:bg-transparent-primary"
                    >
                      <AppIcon item={item} />
                      <div className="min-w-0 flex-1">
                        <p
                          className={`gs-text-body-md-regular max-w-[436px] truncate ${
                            item.status === "failed" ? "text-text-subdued" : "text-text-default"
                          }`}
                        >
                          {itemTitle(item)}
                        </p>
                        <p className="gs-text-body-md-regular max-w-[436px] truncate text-text-subdued">
                          {itemSubtitle(item)}
                        </p>
                      </div>
                      {item.status === "failed" && item.audio_filename && item.kind !== "command" && (
                        <span onClick={(e) => e.stopPropagation()}>
                          <OutlineButton onClick={() => retranscribe(item.id)}>
                            {retranscribingId === item.id ? t("Retrying…") : t("Retry")}
                          </OutlineButton>
                        </span>
                      )}
                      <span className="gs-text-body-sm-regular flex h-10 shrink-0 items-center whitespace-nowrap text-text-subdued">
                        {formatTime(new Date(item.created_at))}
                      </span>
                    </div>
                  ))}
                </div>
              </div>
            ))
          )}
        </div>
      </div>

      {openRecord && (
        <RecordPlayer
          key={openRecord.id}
          item={openRecord}
          onClose={() => onOpenRecord(null)}
          onDelete={deleteHistory}
          onRetranscribe={retranscribe}
          retranscribing={retranscribingId === openRecord.id}
          displayText={openRecord.text}
        />
      )}
    </>
  );
}
