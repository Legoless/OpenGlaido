import { expect, test } from "bun:test";
import {
  BAR_REST, barHeight, easeMicLevel, hudCancellable, hudView, IDLE_HUD, newestHudState, WAVEFORM_HEIGHT, type HudState,
} from "../src/hud-state";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { HudIcon, HudSignal } from "../src/hud";

const BARS = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
const heightsAt = (level: number, timeMs: number, heard = true) => BARS.map((i) => barHeight(i, level, timeMs, heard));
const frames = (fromMs: number, toMs: number) => Array.from({ length: (toMs - fromMs) / 16 }, (_, k) => fromMs + k * 16);

test("quiet mic onsets respond promptly and equally at low and high frame rates", () => {
  const follow = (fps: number, initial: number, target: number) => {
    let level = initial;
    for (let frame = 0; frame < fps / 5; frame++) level = easeMicLevel(level, target, 1000 / fps);
    return level;
  };
  for (const [initial, target] of [[0, 0.45], [0.8, 0]]) {
    expect(follow(30, initial, target)).toBeCloseTo(follow(60, initial, target), 8);
    expect(follow(120, initial, target)).toBeCloseTo(follow(60, initial, target), 8);
  }
  expect(easeMicLevel(0, 0.45, 1000 / 60)).toBeGreaterThan(0.23);
  expect(easeMicLevel(0, 0.45, 66)).toBeGreaterThan(0.42);
  expect(follow(30, 0.8, 0)).toBeGreaterThan(0.1); // releases slower than it attacks
  expect(easeMicLevel(1, 0, 1000 / 60)).toBeGreaterThan(1 - easeMicLevel(0, 1, 1000 / 60));
  expect(easeMicLevel(0.8, 0, 500)).toBeLessThan(0.02);
  expect(easeMicLevel(0.2, 1, 0)).toBe(0.2);
  expect(easeMicLevel(0.2, 1, -1)).toBe(0.2); // first RAF timestamp can precede effect setup
  for (const [from, to] of [[0, 1], [1, 0], [0.3, 0.6], [NaN, Infinity], [-1, 2]]) {
    const level = easeMicLevel(from, to, 1000);
    expect(Number.isFinite(level)).toBe(true);
    expect(level).toBeGreaterThanOrEqual(0);
    expect(level).toBeLessThanOrEqual(1);
  }
});

test("bars rest still until the first sound, then ripple gently while quiet", () => {
  for (const t of [0, 700, 2500]) expect(heightsAt(0, t, false)).toEqual(Array(10).fill(BAR_REST));
  const ripple = frames(0, 8000).flatMap((t) => heightsAt(0, t));
  expect(Math.min(...ripple)).toBeGreaterThanOrEqual(BAR_REST);
  expect(Math.max(...ripple)).toBeLessThanOrEqual(BAR_REST + 1.5);
  expect(Math.max(...ripple) - Math.min(...ripple)).toBeGreaterThan(1.4);
  expect(heightsAt(0, 1000)).not.toEqual(heightsAt(0, 2000)); // travels over time
  expect(new Set(heightsAt(0, 1000)).size).toBe(10); // and across the bars
});

test("speech moves every bar on its own, within the waveform height", () => {
  for (const level of [0.2, 0.6, 1]) {
    const heights = frames(0, 10_000).flatMap((t) => heightsAt(level, t));
    expect(Math.min(...heights)).toBeGreaterThanOrEqual(BAR_REST);
    expect(Math.max(...heights)).toBeLessThanOrEqual(WAVEFORM_HEIGHT);
  }
  expect(new Set(heightsAt(0.6, 1234).map((h) => h.toFixed(1))).size).toBeGreaterThan(6);
  const loud = frames(0, 10_000).flatMap((t) => heightsAt(1, t));
  expect(Math.max(...loud)).toBeGreaterThan(WAVEFORM_HEIGHT - 0.5);
  expect(Math.min(...loud)).toBeGreaterThan(BAR_REST + 0.15 * (WAVEFORM_HEIGHT - BAR_REST) - 0.01); // floor
  // A steady voice keeps each bar moving, and a louder voice raises the bars.
  expect(barHeight(3, 0.7, 1000, true)).not.toBeCloseTo(barHeight(3, 0.7, 1200, true), 1);
  const mean = (level: number) => frames(0, 4000).flatMap((t) => heightsAt(level, t)).reduce((a, b) => a + b) / 2500;
  expect(mean(0.8)).toBeGreaterThan(mean(0.4) + 3);
});

test("processing keeps the bars as resting dots under a shimmer, with no solid strip", () => {
  const processing = renderToStaticMarkup(createElement(HudSignal, { processing: true }));
  expect(processing).toContain('role="progressbar"');
  expect(processing).toContain('aria-label="Working…"');
  expect(processing).toContain("gs-processing-shimmer");
  expect(processing).not.toContain("aria-valuenow");
  expect(processing).not.toContain("w-full");
  expect(processing.match(/<span class="h-1 w-\[3px\] rounded-full/g)).toHaveLength(10);
  const recording = renderToStaticMarkup(createElement(HudSignal, { processing: false }));
  expect(recording).not.toContain('role="progressbar"');
  expect(recording).not.toContain("gs-processing-shimmer");
  expect(recording.match(/<span\b/g)).toHaveLength(10);
});

test("Commands show chevrons and only a hands-free recording offers cancel", () => {
  const visibleIcons = (view: "recording" | "processing" | "error", command: boolean) =>
    [...renderToStaticMarkup(createElement(HudIcon, { view, command })).matchAll(/class="lucide (lucide-[\w-]+)([^"]*)"/g)]
      .filter((m) => !m[2].includes("opacity-0"))
      .map((m) => m[1]);
  expect(visibleIcons("recording", false)).toEqual(["lucide-audio-lines"]);
  expect(visibleIcons("recording", true)).toEqual(["lucide-chevrons-right"]);
  expect(visibleIcons("processing", true)).toEqual(["lucide-chevrons-right"]);
  expect(visibleIcons("error", true)).toEqual(["lucide-circle-alert"]);
  const recording: HudState = { ...IDLE_HUD, recording: true, visible: true, revision: 1 };
  expect(hudCancellable(recording)).toBe(false);
  expect(hudCancellable({ ...recording, hands_free: true })).toBe(true);
  expect(hudCancellable({ ...recording, recording: false, processing: true, hands_free: true })).toBe(false);
});

test("the HUD follows recording, the full response, errors and cancellation without stale reads", () => {
  const recording: HudState = { ...IDLE_HUD, recording: true, visible: true, revision: 1 };
  const processing: HudState = { ...recording, recording: false, processing: true, revision: 2 };
  const complete: HudState = { ...IDLE_HUD, revision: 3 };
  expect(hudView(IDLE_HUD, null)).toBeNull();
  expect(hudView(recording, "error")).toBe("recording");
  expect(hudView({ ...recording, processing: true }, null)).toBe("recording");
  expect(hudView(processing, null)).toBe("processing");
  expect(hudView(processing, "error")).toBe("error");
  expect(hudView(processing, null)).toBe("processing"); // expired warning does not hide ongoing work
  expect(hudView(complete, "error")).toBeNull(); // timed-out error cannot hold the bar open
  expect(hudView({ ...complete, visible: true }, "error")).toBe("error");
  expect(newestHudState(recording, IDLE_HUD)).toBe(recording); // delayed initial read
  expect(newestHudState(processing, recording)).toBe(processing);
  expect(newestHudState(processing, complete)).toBe(complete);
  expect(newestHudState(complete, processing)).toBe(complete); // late event after no-speech/cancel
  expect(newestHudState(complete, { ...processing, revision: 3 })).toBe(complete);
});
