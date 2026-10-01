import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { LoaderCircle, RefreshCw } from "lucide-react";
import { t } from "./i18n";
import { NoteText, OutlineButton, SettingsRow } from "./ui";

export type UpdateStatus = {
  phase: "idle" | "checking" | "up_to_date" | "downloading" | "ready" | "installing" | "error";
  current_version: string;
  next_version: string | null;
  message: string | null;
  revision: number;
};

// Events can overtake the initial read or a manual check's reply.
export function newestUpdateStatus(current: UpdateStatus | null, incoming: UpdateStatus): UpdateStatus {
  return current && incoming.revision < current.revision ? current : incoming;
}

export function useAppUpdates() {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [error, setError] = useState(false);
  const [checking, setChecking] = useState(false);
  const latest = useRef<UpdateStatus | null>(null);
  const generation = useRef(0);
  const apply = useCallback((incoming: UpdateStatus) => {
    const next = newestUpdateStatus(latest.current, incoming);
    if (next !== incoming) return;
    latest.current = next;
    setStatus(next);
    setError(false);
  }, []);

  useEffect(() => {
    let alive = true;
    const revision = latest.current?.revision;
    const unlisten = listen<UpdateStatus>("app-update-status", (event) => {
      if (alive) apply(event.payload);
    });
    // Listen first, so updates between the initial read and subscription are not lost.
    unlisten.then(async () => {
      if (!alive) return;
      const initial = await invoke<UpdateStatus>("get_update_status");
      if (alive) apply(initial);
    }).catch(() => {
      if (alive && latest.current?.revision === revision) setError(true);
    });
    return () => {
      alive = false;
      generation.current += 1;
      unlisten.then((stop) => stop()).catch(() => {});
    };
  }, [apply]);

  const check = async () => {
    const mountedGeneration = generation.current;
    const revision = latest.current?.revision;
    setChecking(true);
    setError(false);
    try {
      const next = await invoke<UpdateStatus>("check_for_updates");
      if (generation.current === mountedGeneration) apply(next);
    } catch {
      if (generation.current === mountedGeneration && latest.current?.revision === revision) setError(true);
    } finally {
      if (generation.current === mountedGeneration) setChecking(false);
    }
  };
  return { status, error, checking, check };
}

export function UpdateSettings({ status, error, checking, check }: ReturnType<typeof useAppUpdates>) {
  const phase = checking ? "checking" : status?.phase;
  const busy = phase === "checking" || phase === "downloading" || phase === "installing";
  let message = "";
  if (error || phase === "error") message = !error && status?.message ? t(status.message) : t("Couldn’t check for updates. Try again later.");
  else if (phase === "checking") message = t("Checking for updates…");
  else if (phase === "downloading") message = t("Downloading update…");
  else if (phase === "ready") {
    message = t("Update ready. Close the main window to install when idle.");
    if (status?.next_version) message = `${t("Version {v}", { v: status.next_version })} — ${message}`;
  }
  else if (phase === "installing") message = t("Installing update…");
  else if (status?.message) message = t(status.message);
  else if (phase === "up_to_date") message = t("OpenGlaido is up to date.");

  return (
    <SettingsRow
      icon={RefreshCw}
      title={t("App updates")}
      description={t("Updates install automatically when OpenGlaido is idle and its windows are closed.")}
      note={message && <div role="status"><NoteText tone={error || phase === "error" ? "error" : "hint"}>{message}</NoteText></div>}
    >
      <OutlineButton onClick={check} disabled={busy || phase === "ready"}>
        {busy && <LoaderCircle className="size-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
        {t("Check for updates")}
      </OutlineButton>
    </SettingsRow>
  );
}
