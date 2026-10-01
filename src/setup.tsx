import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Check, CircleAlert, TriangleAlert } from "lucide-react";
import { t } from "./i18n";
import type { DownloadProgress, SettingsTab, SetupIssue } from "./types";
import { LimeButton, OutlineButton } from "./ui";

/** A shown issue. `enter`: appeared after the card mounted (animates in); `leaving`: when it was
 *  fixed (it collapses out for EXIT_MS). */
export type SetupRow = SetupIssue & { enter?: boolean; leaving?: number };
const EXIT_MS = 300;
const ALL_SET_MS = 1800;

/** The new issues plus the fixed ones still collapsing, each kept where it was. */
export function mergeRows(prev: SetupRow[], next: SetupIssue[], now: number): SetupRow[] {
  const rows: SetupRow[] = next.map((n) => {
    const old = prev.find((p) => p.id === n.id);
    return { ...n, enter: old ? old.enter : true };
  });
  prev.forEach((r, i) => {
    if (next.some((n) => n.id === r.id)) return;
    const leaving = r.leaving ?? now;
    if (now - leaving < EXIT_MS) rows.splice(Math.min(i, rows.length), 0, { ...r, leaving });
  });
  return rows;
}

const actionLabel = (action: NonNullable<SetupIssue["action"]>) =>
  ({
    open_accessibility: t("Open Accessibility settings"),
    open_microphone: t("Open Microphone settings"),
    request_microphone: t("Allow access"),
    open_model: t("Open settings"),
    open_hotkeys: t("Open settings"),
  })[action];

type SetupProgress = { issues: SetupIssue[]; hasBeenReady: boolean; completed: boolean };

/** Celebrate initial setup once; resolving later settings changes only closes the card. */
export function updateSetupProgress(previous: SetupProgress, issues: SetupIssue[]): SetupProgress {
  const ready = issues.length === 0;
  return {
    issues,
    hasBeenReady: previous.hasBeenReady || ready,
    completed: ready && (previous.completed || (!previous.hasBeenReady && previous.issues.length > 0)),
  };
}

// Survives page switches, so coming back to Home doesn't replay setup completion.
let cached: SetupProgress = { issues: [], hasBeenReady: false, completed: false };

// Height + opacity in/out (grid rows 0fr ↔ 1fr); only a fade with reduced motion.
const COLLAPSE = "grid transition-[grid-template-rows,opacity,margin] duration-300 ease-out motion-reduce:transition-opacity";

/** "Finish setting up" card on Home: permissions, microphone, models, hotkeys (get_setup_issues). */
export function SetupCard({ onOpenSettings }: { onOpenSettings: (tab: SettingsTab) => void }) {
  const [rows, setRows] = useState<SetupRow[]>(cached.issues);
  const [allSet, setAllSet] = useState(false);
  const refresh = useRef(() => {});

  useEffect(() => {
    let alive = true;
    refresh.current = () =>
      invoke<SetupIssue[]>("get_setup_issues")
        .then((next) => {
          if (!alive) return;
          const progress = updateSetupProgress(cached, next);
          const justCompleted = progress.completed && !cached.completed;
          cached = progress;
          setAllSet((v) => progress.completed && (v || justCompleted));
          setRows((prev) => mergeRows(prev, next, Date.now()));
        })
        .catch(console.error);
    const again = () => refresh.current();
    again();
    const timer = window.setInterval(again, 4000);
    window.addEventListener("focus", again);
    const unlisten = [
      listen("config-changed", again),
      listen("hotkey-status", again),
      listen<DownloadProgress>("model-download", (e) => {
        if (["done", "failed", "cancelled"].includes(e.payload.state)) again();
      }),
    ];
    return () => {
      alive = false;
      window.clearInterval(timer);
      window.removeEventListener("focus", again);
      unlisten.forEach((u) => u.then((f) => f()));
    };
  }, []);

  // Drop fixed rows once they have collapsed; the check shows briefly, then the card closes.
  useEffect(() => {
    if (!rows.some((r) => r.leaving)) return;
    const id = window.setTimeout(() => setRows((rs) => mergeRows(rs, rs.filter((r) => !r.leaving), Date.now())), EXIT_MS);
    return () => window.clearTimeout(id);
  }, [rows]);
  useEffect(() => {
    if (!allSet) return;
    const id = window.setTimeout(() => setAllSet(false), ALL_SET_MS);
    return () => window.clearTimeout(id);
  }, [allSet]);

  const live = rows.filter((r) => !r.leaving);
  const open = live.length > 0 || allSet;
  // Stays on while the card collapses.
  const done = live.length === 0 && cached.completed;
  const firstError = live.find((r) => r.level === "error" && r.action)?.id;

  const run = (action: SetupIssue["action"]) => {
    if (action === "open_model") onOpenSettings("model");
    else if (action === "open_hotkeys") onOpenSettings("hotkeys");
    else if (action === "open_accessibility") invoke("open_accessibility_settings").catch(console.error);
    else if (action === "open_microphone") invoke("open_microphone_settings").catch(console.error);
    else if (action === "request_microphone")
      invoke<boolean>("request_microphone_access").then(() => refresh.current()).catch(console.error);
  };

  return (
    // The negative margin cancels the page's flex gap while the card is closed.
    <section
      aria-label={t("Finish setting up OpenGlaido")}
      inert={!open}
      className={`${COLLAPSE} shrink-0 ${open ? "grid-rows-[1fr]" : "-mb-4 grid-rows-[0fr] opacity-0"}`}
    >
      <div className="min-h-0 overflow-hidden">
        <div
          className={`rounded-[8px] bg-background-card px-8 py-6 transition-shadow duration-300 ${
            done ? "shadow-[inset_0_0_0_1px_var(--color-border-accent)]" : ""
          }`}
        >
          <div className="grid min-h-7 items-center [&>*]:col-start-1 [&>*]:row-start-1" aria-live="polite">
            <div
              className={`flex items-center gap-2 transition-opacity duration-300 ${done ? "opacity-0" : ""}`}
              aria-hidden={done}
            >
              <span className="gs-text-tag text-text-default">{t("Finish setting up OpenGlaido")}</span>
              {live.length > 0 && (
                <span className="gs-text-tag rounded-[2px] bg-transparent-tertiary px-1.5 py-[3px] text-text-subdued tabular-nums">
                  {live.length}
                </span>
              )}
            </div>
            <div
              className={`flex items-center gap-2 transition-[opacity,transform] duration-300 ease-out ${
                done ? "" : "scale-95 opacity-0"
              }`}
              aria-hidden={!done}
            >
              <span className="flex size-5 items-center justify-center rounded-full bg-button-primary text-text-dark">
                <Check className="size-3.5" strokeWidth={3} />
              </span>
              <span className="gs-text-tag text-text-accent">{t("You're all set")}</span>
            </div>
          </div>

          <ul className={`flex flex-col transition-[margin] duration-300 ${done ? "" : "mt-3"}`}>
            {rows.map((row) => {
              const error = row.level === "error";
              const Icon = error ? CircleAlert : TriangleAlert;
              const label = row.action && actionLabel(row.action);
              const Button = row.id === firstError ? LimeButton : OutlineButton;
              return (
                <li
                  key={row.id}
                  inert={!!row.leaving}
                  className={`${COLLAPSE} ${
                    row.leaving
                      ? "grid-rows-[0fr] opacity-0"
                      : `grid-rows-[1fr] ${row.enter ? "starting:grid-rows-[0fr] starting:opacity-0" : ""}`
                  }`}
                >
                  <div className="min-h-0 overflow-hidden">
                    <div className="flex items-center gap-4 px-3 py-3">
                      <div
                        className={`flex size-9 shrink-0 items-center justify-center rounded-[8px] ${
                          error ? "bg-error-surface text-text-error" : "bg-warning-surface text-text-warning"
                        }`}
                      >
                        <Icon className="size-[18px]" aria-label={error ? t("Error") : t("Warning")} />
                      </div>
                      <div className="flex min-w-0 flex-1 flex-col gap-1">
                        <span className="gs-text-body-md-regular text-text-default">{t(row.title, row.vars)}</span>
                        <span className="gs-text-body-sm-regular text-text-subdued select-text">
                          {t(row.detail, row.vars)}
                        </span>
                      </div>
                      {label && <Button onClick={() => run(row.action)}>{label}</Button>}
                    </div>
                  </div>
                </li>
              );
            })}
          </ul>
        </div>
      </div>
    </section>
  );
}
