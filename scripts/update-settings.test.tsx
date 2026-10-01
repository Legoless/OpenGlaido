import { expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { newestUpdateStatus, UpdateSettings, type UpdateStatus } from "../src/update-settings";

const status = (phase: UpdateStatus["phase"], revision = 1): UpdateStatus => ({
  phase, revision, current_version: "0.1.0", next_version: null, message: null,
});

test("late update reads and check replies cannot overwrite newer events", () => {
  const downloading = status("downloading", 3);
  const ready = status("ready", 4);
  expect(newestUpdateStatus(null, downloading)).toBe(downloading);
  expect(newestUpdateStatus(downloading, status("idle", 1))).toBe(downloading);
  expect(newestUpdateStatus(downloading, status("checking", 2))).toBe(downloading);
  expect(newestUpdateStatus(downloading, ready)).toBe(ready);
});

test("update settings show progress and inline failures without allowing duplicate checks", () => {
  for (const phase of ["idle", "checking", "up_to_date", "downloading", "ready", "installing", "error"] as const) {
    const html = renderToStaticMarkup(<UpdateSettings status={status(phase)} error={false} checking={false} check={async () => {}} />);
    const button = html.match(/<button([^>]*)>/)?.[1];
    expect(button).toBeDefined();
    expect(button!.includes("disabled")).toBe(["checking", "downloading", "ready", "installing"].includes(phase));
    expect(html.includes("animate-spin")).toBe(["checking", "downloading", "installing"].includes(phase));
    if (phase === "ready") expect(html).toContain("Close the main window to install when idle.");
    if (phase === "error") expect(html).toContain("Couldn’t check for updates. Try again later.");
  }
  const unavailable = { ...status("up_to_date"), message: "No public updates are available yet." };
  const html = renderToStaticMarkup(<UpdateSettings status={unavailable} error={false} checking={false} check={async () => {}} />);
  expect(html).toContain(unavailable.message);
  expect(html).not.toContain("OpenGlaido is up to date.");
  const development = { ...status("idle"), message: "Updates are available in packaged builds only." };
  expect(renderToStaticMarkup(<UpdateSettings status={development} error={false} checking={false} check={async () => {}} />)).toContain(development.message);
  const pending = renderToStaticMarkup(<UpdateSettings status={status("idle")} error={false} checking check={async () => {}} />);
  expect(pending).toContain("Checking for updates…");
  expect(pending.match(/<button([^>]*)>/)?.[1]).toContain("disabled");
  for (const message of ["Couldn’t download a verified update. Try again later.", "Couldn’t install the update. Try again later."]) {
    const failed = renderToStaticMarkup(<UpdateSettings status={{ ...status("error"), message }} error={false} checking={false} check={async () => {}} />);
    expect(failed).toContain(message);
    expect(failed).not.toContain("Couldn’t check for updates.");
  }
  const ready = renderToStaticMarkup(<UpdateSettings status={{ ...status("ready"), next_version: "0.2.0" }} error={false} checking={false} check={async () => {}} />);
  expect(ready).toContain("Version 0.2.0");
  expect(ready).toContain("its windows are closed.");
});
