export type HudState = { recording: boolean; processing: boolean; visible: boolean; revision: number };
export type HudView = "recording" | "processing" | "warning" | "error";

export const IDLE_HUD: HudState = { recording: false, processing: false, visible: false, revision: -1 };

/** A late initial read or queued event must never resurrect a completed dictation. */
export function newestHudState(current: HudState, incoming: HudState): HudState {
  return incoming.revision > current.revision ? incoming : current;
}

export function hudView(state: HudState, message: "warning" | "error" | null): HudView | null {
  if (!state.visible) return null;
  if (state.recording) return "recording";
  return message ?? (state.processing ? "processing" : null);
}
