import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { AudioLines } from "lucide-react";
import { resolveLocale, setLocale, t } from "./i18n";
import type { TranscriptionConfig } from "./types";

// ------------------------------------------------------------------
// HUD dictation bar (/#hud): always-visible pill in a 420x72 window
// ------------------------------------------------------------------
const BAR_COUNT = 10;

export function Hud() {
  const [isRecording, setIsRecording] = useState(false);
  const [isProcessing, setIsProcessing] = useState(false);
  const [levels, setLevels] = useState<number[]>(() => Array(BAR_COUNT).fill(0));
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const applyConfig = (c: TranscriptionConfig) => setLocale(resolveLocale(c.app_language));
    invoke<TranscriptionConfig>("get_config").then(applyConfig).catch(console.error);
    const unlistenConfig = listen<TranscriptionConfig>("config-changed", (e) => applyConfig(e.payload));
    const unlistenRec = listen<boolean>("recording-status", (e) => setIsRecording(e.payload));
    const unlistenProc = listen<boolean>("processing-status", (e) => setIsProcessing(e.payload));
    invoke<boolean>("get_recording_state").then(setIsRecording).catch(console.error);
    return () => {
      unlistenConfig.then((f) => f());
      unlistenRec.then((f) => f());
      unlistenProc.then((f) => f());
    };
  }, []);

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
            <span className="gs-text-body-sm-regular truncate text-text-default">{t(error ?? "")}</span>
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
