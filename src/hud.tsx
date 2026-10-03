import { useEffect, useRef, useState, type RefObject } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { AudioLines, ChevronsRight, CircleAlert, TriangleAlert, X } from "lucide-react";
import { resolveLocale, setLocale, t } from "./i18n";
import type { TranscriptionConfig } from "./types";
import {
  barHeight, easeMicLevel, hudCancellable, hudView, IDLE_HUD, newestHudState, WAVEFORM_HEIGHT, type HudState, type HudView,
} from "./hud-state";

/** "dictation-error" payload: model failures reach the HUD; all notices reach the main toast. */
export type BarMessage = { message: string; level: "error" | "warning"; vars?: Record<string, string> };

// ------------------------------------------------------------------
// HUD dictation bar (/#hud): one pill in a 420x72 window that morphs between
// recording, processing and a model warning/error message.
// ------------------------------------------------------------------
const BAR_COUNT = 10;
const MESSAGE_MS = { warning: 3500, error: 4500 };

// Tint + 1px ring over the pill (tokens in index.css).
const TONES: Record<HudView, string> = {
  recording: "inset-ring-border-default",
  processing: "inset-ring-border-default",
  warning: "bg-warning-surface inset-ring-warning-border",
  error: "bg-error-surface inset-ring-error-border",
};
const ICON = "absolute inset-0 size-[18px] transition-[opacity,scale,filter] duration-300";
const ICON_OFF = "scale-50 opacity-0 blur-[2px]";

/** Mic (dictation) or chevrons (Commands); a warning/error icon replaces both for messages. */
export function HudIcon({ view, command }: { view: HudView; command: boolean }) {
  const isMessage = view === "warning" || view === "error";
  return (
    <span className="relative size-[18px] shrink-0">
      <AudioLines className={`${ICON} text-text-accent ${isMessage || command ? ICON_OFF : ""}`} />
      <ChevronsRight className={`${ICON} text-text-accent ${isMessage || !command ? ICON_OFF : ""}`} />
      <TriangleAlert className={`${ICON} text-text-warning ${view === "warning" ? "" : ICON_OFF}`} />
      <CircleAlert className={`${ICON} text-text-error ${view === "error" ? "" : ICON_OFF}`} />
    </span>
  );
}

/** While processing, the bars rest as dots under a sweeping shimmer (no implied percentage). */
export function HudSignal({ processing, bars }: { processing: boolean; bars?: RefObject<Array<HTMLSpanElement | null>> }) {
  return (
    <div
      {...(processing ? { role: "progressbar", "aria-label": t("Working…") } : { "aria-hidden": true })}
      className={`flex items-center gap-1 ${processing ? "gs-processing-shimmer" : ""}`}
      style={{ height: WAVEFORM_HEIGHT }}
    >
      {Array.from({ length: BAR_COUNT }, (_, i) => (
        <span key={i} ref={(el) => { if (bars) bars.current[i] = el; }} className="h-1 w-[3px] rounded-full bg-text-accent" />
      ))}
    </div>
  );
}

export function Hud() {
  const [status, setStatus] = useState<HudState>(IDLE_HUD);
  const statusRef = useRef<HudState>(IDLE_HUD);
  const target = useRef(0);
  const level = useRef(0);
  // Any sound since recording began: until then the bars rest still, afterwards they ripple.
  const heard = useRef(false);
  const barEls = useRef<Array<HTMLSpanElement | null>>(Array(BAR_COUNT).fill(null));
  // The last message stays rendered while the pill animates out; `messageOn` says it's current.
  const [message, setMessage] = useState<BarMessage | null>(null);
  const [messageOn, setMessageOn] = useState(false);
  // What the pill shows; kept while it's hidden so it doesn't morph on the way out.
  const [view, setView] = useState<HudView>("recording");
  // Size changes animate only once the pill is on screen (it appears at its final size).
  const [settled, setSettled] = useState(false);
  const pillRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const applyConfig = (c: TranscriptionConfig) => setLocale(resolveLocale(c.app_language));
    invoke<TranscriptionConfig>("get_config").then(applyConfig).catch(console.error);
    const unlistenConfig = listen<TranscriptionConfig>("config-changed", (e) => applyConfig(e.payload));
    const applyStatus = (incoming: HudState) => {
      const previous = statusRef.current;
      const next = newestHudState(previous, incoming);
      if (next === previous) return;
      statusRef.current = next;
      if (next.recording && !previous.recording) {
        target.current = 0;
        level.current = 0;
        heard.current = false;
        setMessageOn(false);
      }
      setStatus(next);
    };
    // Register before reading: the revision prevents a slower read replacing a newer event.
    const unlistenStatus = listen<HudState>("hud-state", (e) => applyStatus(e.payload));
    unlistenStatus.then(() => invoke<HudState>("get_hud_state")).then(applyStatus).catch(console.error);
    return () => {
      unlistenConfig.then((f) => f());
      unlistenStatus.then((f) => f());
    };
  }, []);

  useEffect(() => {
    let timer: number | undefined;
    const unlistenLevel = listen<number>("mic-level", (e) => {
      target.current = e.payload;
      if (e.payload > 0) heard.current = true;
    });
    const unlistenError = listen<BarMessage>("dictation-error", (e) => {
      setMessage(e.payload);
      setMessageOn(true);
      window.clearTimeout(timer);
      timer = window.setTimeout(() => setMessageOn(false), MESSAGE_MS[e.payload.level]);
    }, { target: "hud" });
    return () => {
      window.clearTimeout(timer);
      unlistenLevel.then((f) => f());
      unlistenError.then((f) => f());
    };
  }, []);

  // Eases the mic level and moves the ten bars from it. Heights are written on the elements so a
  // 60 Hz update doesn't re-render the pill, and React doesn't reset them.
  useEffect(() => {
    let raf = 0;
    let lastFrame = performance.now();
    // Reduce Motion: no idle ripple in pauses (speech still moves the bars).
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)");
    const tick = (now: number) => {
      const elapsed = now - lastFrame;
      lastFrame = now;
      const { recording } = statusRef.current;
      level.current = easeMicLevel(level.current, recording ? target.current : 0, elapsed);
      for (let i = 0; i < BAR_COUNT; i++) {
        const el = barEls.current[i];
        if (el) el.style.height = `${barHeight(i, level.current, now, recording && heard.current && !reduced.matches)}px`;
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, []);

  // The pill's width/height follow its content (set here, not in render, so they can transition).
  useEffect(() => {
    const pill = pillRef.current!;
    const content = contentRef.current!;
    const observer = new ResizeObserver(() => {
      const { paddingLeft, paddingRight } = getComputedStyle(pill);
      pill.style.width = `${content.offsetWidth + parseFloat(paddingLeft) + parseFloat(paddingRight)}px`;
      pill.style.height = `${Math.max(38, content.offsetHeight + 14)}px`;
    });
    observer.observe(content);
    return () => observer.disconnect();
  }, []);

  const live = hudView(status, messageOn && message ? message.level : null);
  if (live && live !== view) setView(live);
  const shown = live !== null;
  const isMessage = view === "warning" || view === "error";
  const cancellable = hudCancellable(status, live);

  useEffect(() => {
    if (!shown) return setSettled(false);
    const id = requestAnimationFrame(() => setSettled(true));
    return () => cancelAnimationFrame(id);
  }, [shown]);

  return (
    <div className="flex h-screen w-screen items-end justify-center bg-transparent pb-[17px] select-none">
      <div
        ref={pillRef}
        className={`hud-pill relative flex origin-bottom items-center overflow-hidden rounded-[12px] bg-background-primary pl-3 ${cancellable ? "pr-0" : "pr-3"} shadow-[0_4px_14px_rgba(0,0,0,0.18)] ${
          settled ? "transition-[width,height,opacity,scale,translate]" : "transition-[opacity,scale,translate]"
        } ${
          shown
            ? "duration-[120ms] ease-[cubic-bezier(0.25,1.25,0.5,1)]"
            : "translate-y-1 scale-90 opacity-0 duration-0"
        }`}
      >
        <span
          className={`absolute inset-0 rounded-[inherit] inset-ring transition-[background-color,box-shadow] duration-300 ${TONES[view]}`}
        />
        <div
          ref={contentRef}
          className="relative flex w-max shrink-0 items-center"
        >
          <HudIcon view={view} command={status.command} />
          {/* Waveform and message share one cell; the hidden one is 0x0 so only the shown one sizes the pill. */}
          <div className="ml-2 grid items-center">
            <div className={`col-start-1 row-start-1 flex items-center ${isMessage ? "size-0" : ""}`}>
              <div
                className={`shrink-0 origin-left transition-[opacity,scale] ${
                  isMessage ? "scale-x-0 opacity-0 duration-150" : "delay-100 duration-200"
                }`}
              >
                <HudSignal processing={view === "processing"} bars={barEls} />
              </div>
            </div>
            <div className={`col-start-1 row-start-1 flex items-center ${isMessage ? "" : "size-0"}`}>
              {/* Keyed by text: a new message fades in (index.css), leaving fades out by transition. */}
              <p
                key={message?.message}
                className={`gs-text-body-sm-regular line-clamp-2 w-max max-w-[346px] shrink-0 break-words text-text-default transition-[opacity,translate] duration-100 ${
                  isMessage ? "hud-message-in" : "-translate-x-1.5 opacity-0"
                }`}
              >
                {message && t(message.message, message.vars)}
              </p>
            </div>
          </div>
          {/* The bar never takes clicks (it must not steal focus): the hotkey tap catches this
              spot to cancel a hands-free capture or pending parsing (CANCEL_HIT in lib.rs). */}
          {cancellable && (
            <span aria-hidden="true" className="mr-2 ml-0.5 grid h-5 w-[22px] shrink-0 place-items-center pl-0.5 text-text-subdued">
              <X className="size-3.5" />
            </span>
          )}
        </div>
      </div>
    </div>
  );
}
