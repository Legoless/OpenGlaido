import { AudioWaveform, MessageSquareText } from "lucide-react";
import { t } from "./i18n";
import type { HotkeyRow, TranscriptionConfig } from "./types";
import { SettingsSection } from "./ui";

export interface CommandsHotkeySectionProps {
  config: TranscriptionConfig;
  /** Settings' own row renderer: pencil, chips, recorder and warnings for a binding field. */
  hotkeyRow: HotkeyRow;
}

/** Settings › Hotkeys "Commands" section, between Dictation and App (same rows as dictation). */
export function CommandsHotkeySection({ config, hotkeyRow }: CommandsHotkeySectionProps) {
  // Commands are a beta feature: their hotkeys only exist once it is on.
  if (!config.beta_features) return null;
  return (
    <>
      <SettingsSection label={t("Commands")} />
      {hotkeyRow(
        "commands_hold",
        MessageSquareText,
        t("Commands"),
        t("Hold to tell OpenGlaido what to write, rewrite, format, or answer"),
      )}
      {hotkeyRow(
        "commands_toggle",
        AudioWaveform,
        t("Hands-free commands"),
        t("Press once for hands-free commands. Press again to stop."),
      )}
    </>
  );
}
