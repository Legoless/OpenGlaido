import { expect, test } from "bun:test";
import { micChoice } from "../src/settings";

test("a disconnected microphone leaves the list and comes back with its saved choice", () => {
  const both = ["MacBook Pro Microphone", "Mikme Microphone"];
  expect(micChoice(both, "Mikme Microphone")).toEqual({ devices: both, value: "Mikme Microphone" });
  // Unplugged: not listed, System default (null) shown; the saved choice is untouched by the caller.
  expect(micChoice(["MacBook Pro Microphone"], "Mikme Microphone")).toEqual({ devices: ["MacBook Pro Microphone"], value: null });
  expect(micChoice(both, null)).toEqual({ devices: both, value: null });
  // Before the first list arrives, the saved choice shows instead of flashing System default.
  expect(micChoice(null, "Mikme Microphone")).toEqual({ devices: ["Mikme Microphone"], value: "Mikme Microphone" });
  expect(micChoice(null, null)).toEqual({ devices: [], value: null });
});
