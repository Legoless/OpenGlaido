import { expect, test } from "bun:test";
import { Children, isValidElement, type ComponentProps, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { DictationBehaviorSettings } from "../src/settings";
import { Toggle } from "../src/ui";
import type { SaveConfig, TranscriptionConfig } from "../src/types";

const config = {
  copy_to_clipboard: false,
  cancel_on_focus_change: false,
  mute_background: false,
  sound_feedback: true,
  bar_location: "bottom",
};

function focusToggle(enabled: boolean, save: SaveConfig) {
  const section = DictationBehaviorSettings({ config: { ...config, cancel_on_focus_change: enabled }, save, errorNote: () => null });
  const row = Children.toArray(section.props.children).find(
    (child) => isValidElement<{ title: string }>(child) && child.props.title === "Cancel when focus changes",
  );
  if (!isValidElement<{ children: ReactElement<ComponentProps<typeof Toggle>> }>(row)) throw new Error("Missing focus setting");
  return Toggle(row.props.children.props);
}

test("focus cancellation starts unchecked and explains window or field changes", () => {
  const html = renderToStaticMarkup(<DictationBehaviorSettings config={config} save={async () => null} errorNote={() => null} />);
  expect(html).toContain("Behavior");
  expect(html).toContain("Stop dictation if you switch to another window or field.");
  expect(html).toContain('aria-checked="false" aria-label="Cancel when focus changes"');
  expect(renderToStaticMarkup(focusToggle(true, async () => null))).toContain('aria-checked="true"');
});

test("the focus switch saves only its enabled or disabled setting", () => {
  const patches: Partial<TranscriptionConfig>[] = [];
  const save: SaveConfig = async (patch) => { patches.push(patch); return null; };
  focusToggle(false, save).props.onClick();
  focusToggle(true, save).props.onClick();
  expect(patches).toEqual([{ cancel_on_focus_change: true }, { cancel_on_focus_change: false }]);
});

test("focus-setting save errors use the existing inline settings note", () => {
  const html = renderToStaticMarkup(<DictationBehaviorSettings
    config={config}
    save={async () => null}
    errorNote={(key) => key === "cancel_on_focus_change" ? "Could not save focus cancellation" : null}
  />);
  expect(html).toContain("Could not save focus cancellation");
});
