import { expect, test } from "bun:test";
import { hudView, IDLE_HUD, newestHudState, type HudState } from "../src/hud-state";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { HudSignal } from "../src/hud";

test("processing replaces microphone bars with an indeterminate progress strip", () => {
  const processing = renderToStaticMarkup(createElement(HudSignal, { processing: true }));
  expect(processing).toContain('role="progressbar"');
  expect(processing).toContain('aria-label="Working…"');
  expect(processing).not.toContain("aria-valuenow");
  expect(processing).not.toContain("w-[3px]");
  expect(processing.match(/<span\b/g)).toHaveLength(1);
  const recording = renderToStaticMarkup(createElement(HudSignal, { processing: false }));
  expect(recording).not.toContain('role="progressbar"');
  expect(recording.match(/<span\b/g)).toHaveLength(10);
});

test("the HUD follows recording, the full response, errors and cancellation without stale reads", () => {
  const recording: HudState = { recording: true, processing: false, visible: true, revision: 1 };
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
