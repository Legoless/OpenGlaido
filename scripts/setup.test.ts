// bun test scripts/setup.test.ts
import { expect, test } from "bun:test";
import { mergeRows, updateSetupProgress, type SetupRow } from "../src/setup";
import type { SetupIssue } from "../src/types";

const issue = (id: string): SetupIssue => ({ id, level: "error", title: id, detail: "", vars: {}, action: null });
const ids = (rows: SetupRow[]) => rows.map((r) => `${r.id}${r.leaving === undefined ? "" : `@${r.leaving}`}`);

test("switching providers after setup never replays the all-set message", () => {
  let progress = updateSetupProgress({ issues: [], hasBeenReady: false, completed: false }, []);
  expect(progress.completed).toBe(false);
  for (const id of ["stt_key", "llm_key", "stt_model"]) {
    progress = updateSetupProgress(progress, [issue(id)]);
    expect(progress.completed).toBe(false);
    progress = updateSetupProgress(progress, []);
    expect(progress.completed).toBe(false);
    expect(progress.issues).toEqual([]);
  }
});

test("initial setup completion celebrates once and keeps its heading through collapse", () => {
  let progress = updateSetupProgress({ issues: [], hasBeenReady: false, completed: false }, [issue("microphone")]);
  expect(progress.completed).toBe(false);
  progress = updateSetupProgress(progress, []);
  expect(progress.completed).toBe(true);
  // Polling or returning to Home retains the heading without another completion transition.
  progress = updateSetupProgress(progress, []);
  expect(progress.completed).toBe(true);
  progress = updateSetupProgress(progress, [issue("stt_key")]);
  expect(progress.completed).toBe(false);
  progress = updateSetupProgress(progress, []);
  expect(progress.completed).toBe(false);
});

test("fixed issues stay where they were while they collapse", () => {
  const prev = [issue("a"), issue("b"), issue("c")];
  expect(ids(mergeRows(prev, [issue("a"), issue("c")], 1000))).toEqual(["a", "b@1000", "c"]);
  expect(ids(mergeRows(prev, [issue("c")], 1000))).toEqual(["a@1000", "b@1000", "c"]);
  expect(ids(mergeRows(prev, [], 1000))).toEqual(["a@1000", "b@1000", "c@1000"]);
});

test("collapsed rows drop out, returning issues come back", () => {
  const rows = mergeRows([issue("a"), issue("b")], [issue("b")], 1000);
  expect(ids(mergeRows(rows, [issue("b")], 1100))).toEqual(["a@1000", "b"]);
  expect(ids(mergeRows(rows, [issue("b")], 1300))).toEqual(["b"]);
  expect(ids(mergeRows(rows, [issue("a"), issue("b")], 1100))).toEqual(["a", "b"]);
});

test("new issues come in the backend's order", () => {
  expect(ids(mergeRows([issue("b")], [issue("a"), issue("b"), issue("c")], 0))).toEqual(["a", "b", "c"]);
});

test("only issues that appear after mounting animate in", () => {
  const rows = mergeRows([issue("a")], [issue("a"), issue("b")], 0);
  expect(rows.map((r) => !!r.enter)).toEqual([false, true]);
  expect(mergeRows(rows, [issue("b")], 0).map((r) => !!r.enter)).toEqual([false, true]);
});
