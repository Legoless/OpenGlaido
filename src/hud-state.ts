/** command: a Commands capture (chevron icon); hands_free: recording hands-free (cancel "x"). */
export type HudState = {
  recording: boolean; processing: boolean; visible: boolean; revision: number; command: boolean; hands_free: boolean;
};
export type HudView = "recording" | "processing" | "warning" | "error";

export const IDLE_HUD: HudState = {
  recording: false, processing: false, visible: false, revision: -1, command: false, hands_free: false,
};
export const WAVEFORM_HEIGHT = 24;
export const BAR_REST = 4;
const RIPPLE = 1.5; // px above rest while quiet
const RIPPLE_SPEED = (2 * Math.PI) / 4000; // one idle wave every 4 s
const VOICE_FLOOR = 0.15; // share of a bar's loudness height that never swings away

/** Ease the mic level in time (fast attack, slower release), so a busy Mac doesn't add display lag. */
export function easeMicLevel(previous: number, target: number, elapsedMs: number): number {
  const bounded = (value: number) => Number.isFinite(value) ? Math.max(0, Math.min(1, value)) : 0;
  const current = bounded(previous);
  const next = bounded(target);
  const elapsed = Number.isFinite(elapsedMs) ? Math.max(0, elapsedMs) : 0;
  const blend = 1 - Math.exp(-elapsed / (next > current ? 22 : 120));
  return current + (next - current) * blend;
}

/** Bar height in px. Before the first sound (`heard`) bars rest still; then a slow ripple travels
 *  across them, and speech lifts every bar on its own wobble (~1.8 + ~3 Hz) so a steady voice stays alive. */
export function barHeight(index: number, level: number, timeMs: number, heard: boolean): number {
  const ripple = heard ? RIPPLE * (0.5 + 0.5 * Math.sin(timeMs * RIPPLE_SPEED - index * 0.7)) : 0;
  const wobble = 0.5 + 0.25 * Math.sin(timeMs * 0.0113 + index * 2.4) + 0.25 * Math.sin(timeMs * 0.0187 + index * 1.3);
  const voice = (WAVEFORM_HEIGHT - BAR_REST) * level ** 1.3 * (VOICE_FLOOR + (1 - VOICE_FLOOR) * wobble);
  return BAR_REST + Math.max(ripple, voice);
}

export const hudCancellable = (state: HudState) => state.recording && state.hands_free;

/** A late initial read or queued event must never resurrect a completed dictation. */
export function newestHudState(current: HudState, incoming: HudState): HudState {
  return incoming.revision > current.revision ? incoming : current;
}

export function hudView(state: HudState, message: "warning" | "error" | null): HudView | null {
  if (!state.visible) return null;
  if (state.recording) return "recording";
  return message ?? (state.processing ? "processing" : null);
}
