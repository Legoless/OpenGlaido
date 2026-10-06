import { expect, test } from "bun:test";
import { chooseMic, micChoice } from "../src/settings";

test("a disconnected microphone leaves the list and comes back with its saved choice", () => {
  const both = ["MacBook Pro Microphone", "Mikme Microphone"];
  expect(micChoice(both, ["Mikme Microphone"])).toEqual({ devices: both, value: "Mikme Microphone" });
  // Unplugged: not listed, System default (null) shown; the saved choice is untouched by the caller.
  expect(micChoice(["MacBook Pro Microphone"], ["Mikme Microphone"])).toEqual({ devices: ["MacBook Pro Microphone"], value: null });
  expect(micChoice(both, [])).toEqual({ devices: both, value: null });
  // The first saved one that is connected is shown.
  expect(micChoice(both, ["Desk Mic", "Mikme Microphone", "MacBook Pro Microphone"]).value).toBe("Mikme Microphone");
  // Before the first list arrives, the latest choice shows instead of flashing System default.
  expect(micChoice(null, ["Mikme Microphone", "Desk Mic"])).toEqual({ devices: ["Mikme Microphone"], value: "Mikme Microphone" });
  expect(micChoice(null, [])).toEqual({ devices: [], value: null });
});

test("each desk keeps its microphone", () => {
  const home = ["Built-in", "Home Mic"];
  const office = ["Built-in", "Office Mic"];
  const travel = ["Built-in"];
  const inUse = (saved: string[], devices: string[]) => micChoice(devices, saved).value;

  let saved = chooseMic([], "Home Mic", home);
  saved = chooseMic(saved, "Office Mic", office);
  expect(inUse(saved, home)).toBe("Home Mic");
  expect(inUse(saved, office)).toBe("Office Mic");
  // Picking the built-in while away does not beat the desk microphones at their desks.
  saved = chooseMic(saved, "Built-in", travel);
  expect(saved).toEqual(["Home Mic", "Office Mic", "Built-in"]);
  expect([inUse(saved, home), inUse(saved, office), inUse(saved, travel)]).toEqual(["Home Mic", "Office Mic", "Built-in"]);
  // Picking it at a desk beats that desk's microphone only.
  saved = chooseMic(saved, "Built-in", office);
  expect(saved).toEqual(["Home Mic", "Built-in", "Office Mic"]);
  expect([inUse(saved, home), inUse(saved, office)]).toEqual(["Home Mic", "Built-in"]);
  // Picking the one already in use changes nothing; System default forgets them all.
  expect(chooseMic(saved, "Built-in", office)).toEqual(saved);
  expect(chooseMic(saved, "Home Mic", home)).toEqual(saved);
  expect(chooseMic(saved, null, home)).toEqual([]);
});
