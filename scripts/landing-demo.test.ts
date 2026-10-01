import { test, expect } from "bun:test";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";

test("landing motion respects preferences, can pause or play, and never accesses audio or network", () => {
  const source = readFileSync(new URL("../website/demo.js", import.meta.url), "utf8");
  for (const reducedMotion of [false, true]) {
    const body = { dataset: { motion: "" } };
    let click: () => void = () => { throw new Error("Click handler was not registered"); };
    let change: (event: { matches: boolean }) => void = () => { throw new Error("Preference handler was not registered"); };
    const attributes: Record<string, string> = {};
    const toggle = {
      hidden: true,
      setAttribute: (name: string, value: string) => { attributes[name] = value; },
      addEventListener: (event: string, handler: () => void) => { expect(event).toBe("click"); click = handler; },
    };
    const label = { textContent: "" };
    const elements = { "#motion-toggle": toggle, "#motion-label": label };
    runInNewContext(source, {
      document: { body, querySelector: (selector: keyof typeof elements) => elements[selector] },
      window: {
        matchMedia: (query: string) => {
          expect(query).toBe("(prefers-reduced-motion: reduce)");
          return {
            matches: reducedMotion,
            addEventListener: (event: string, handler: typeof change) => { expect(event).toBe("change"); change = handler; },
          };
        },
      },
      // Network, audio, and timer APIs are absent: accidental access fails this check.
    });
    const expectMotion = (running: boolean) => {
      expect(body.dataset.motion).toBe(running ? "running" : "paused");
      expect(label.textContent).toBe(running ? "Pause motion" : "Play motion");
      expect(attributes["aria-label"]).toBe(running ? "Pause animations" : "Play animations");
    };
    expect(toggle.hidden).toBe(false);
    expectMotion(!reducedMotion);
    click();
    expectMotion(reducedMotion);
    click();
    expectMotion(!reducedMotion);
    change({ matches: true });
    expectMotion(false);
    click(); // An explicit choice can enable motion despite the OS preference.
    expectMotion(true);
    change({ matches: false });
    expectMotion(true);
    click();
    expectMotion(false);
    change({ matches: true });
    expectMotion(false);
    change({ matches: false });
    expectMotion(true);
  }
});
