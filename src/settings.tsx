import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  AudioLines,
  AudioWaveform,
  Check,
  FlaskConical,
  Search,
  SunMoon,
  VolumeX,
  Captions,
  Clipboard,
  Command,
  Cpu,
  Dock,
  Ellipsis,
  Languages,
  MapPin,
  Mic,
  Music,
  Pencil,
  Power,
  Settings,
  Settings2,
  TriangleAlert,
  X,
} from "lucide-react";
import { CommandsHotkeySection } from "./commands-hotkeys";
import { LOCALES, locale, t } from "./i18n";
import { ModelSettings } from "./model-settings";
import { UpdateSettings, useAppUpdates } from "./update-settings";
import type {
  HotkeyField,
  HotkeyRow,
  HotkeyStatus,
  IconType,
  SaveConfig,
  SettingsTab,
  TranscriptionConfig,
} from "./types";
import {
  Dropdown,
  IS_MAC,
  IconButton,
  Keycaps,
  LimeButton,
  MODIFIER_CODES,
  Modal,
  NoteText,
  OutlineButton,
  SettingsRow,
  SettingsSection,
  Toggle,
  bindingLabels,
  toBinding,
  useEscape,
} from "./ui";

export type CaptureResult = { binding: string; warning?: string | null; error?: string };

// start/stop_hotkey_capture are async commands; keep them in call order (StrictMode runs the
// recorder's effect twice in dev: start, stop, start).
let captureQueue: Promise<unknown> = Promise.resolve();
export function captureCommand<T>(cmd: "start_hotkey_capture" | "stop_hotkey_capture"): Promise<T> {
  const next = captureQueue.catch(() => {}).then(() => invoke<T>(cmd));
  captureQueue = next;
  return next;
}

/**
 * Records a new binding. Native capture (macOS) streams "hotkey-capture" events; otherwise the
 * webview's KeyboardEvent.code values are used. Esc alone cancels. Capture always stops on unmount.
 */
export function HotkeyRecorder({
  onSave,
  onCancel,
}: {
  onSave: (binding: string) => Promise<string | null>;
  onCancel: () => void;
}) {
  const [keys, setKeys] = useState("");
  const [result, setResult] = useState<CaptureResult | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  // Esc belongs to the recorder (cancel, or part of a chord), not to the Settings modal.
  useEscape(onCancel, true, true);

  // onCancel only closes the recorder (a state setter in the parent), so the first one is kept.
  useEffect(() => {
    let alive = true;
    let native = false;
    let latest = ""; // a slow check for an older chord must not become the saveable result
    const progress = (binding: string) => {
      latest = binding;
      setKeys(binding);
      setResult(null);
      setSaveError(null);
    };
    const finish = (binding: string) => {
      progress(binding);
      const current = () => alive && latest === binding;
      invoke<string | null>("check_hotkey", { binding })
        .then((warning) => current() && setResult({ binding, warning }))
        .catch((e) => current() && setResult({ binding, error: String(e) }));
    };

    // DOM fallback: chord = keys held when a non-modifier goes down, or the largest
    // modifier-only set once everything is released.
    const down = new Set<string>();
    let combo: string[] = [];
    let finished = false;
    const onKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopImmediatePropagation();
      if (e.repeat) return;
      // Esc alone cancels (in both modes); with modifiers it is part of a chord.
      const modified = e.metaKey || e.ctrlKey || e.altKey || e.shiftKey || down.size > 0;
      if (e.code === "Escape" && !modified) return onCancel();
      if (native) return;
      if (down.size === 0) {
        combo = [];
        finished = false;
      }
      if (!combo.includes(e.code)) combo.push(e.code);
      if (MODIFIER_CODES.includes(e.code)) {
        down.add(e.code);
        if (!finished) progress(toBinding(combo));
      } else {
        // Key-ups are not delivered for keys pressed while ⌘ is held, so never track non-modifiers.
        finished = true;
        finish(toBinding([...down, e.code]));
      }
    };
    const onKeyUp = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopImmediatePropagation();
      if (native) return;
      down.delete(e.code);
      if (down.size === 0 && !finished && combo.length > 0) {
        finished = true;
        finish(toBinding(combo));
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("keyup", onKeyUp, true);
    // Leaving the window ends recording: the backend stops native capture on blur too.
    window.addEventListener("blur", onCancel);

    const unlisten = listen<{ keys: string; done: boolean }>("hotkey-capture", (e) => {
      // The native engine reports Esc alone as an empty finished chord. The keydown above cancels:
      // closing here first would leave that keydown to the Settings modal, closing it too.
      if (e.payload.done && !e.payload.keys) return;
      if (e.payload.done) finish(e.payload.keys);
      else progress(e.payload.keys);
    });
    captureCommand<boolean>("start_hotkey_capture")
      .then((n) => {
        native = n;
      })
      .catch((e) => alive && setSaveError(String(e)));

    return () => {
      alive = false;
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("keyup", onKeyUp, true);
      window.removeEventListener("blur", onCancel);
      unlisten.then((f) => f());
      captureCommand("stop_hotkey_capture").catch(console.error);
    };
  }, []);

  const save = async () => {
    if (!result || result.error) return;
    await captureCommand("stop_hotkey_capture").catch(console.error);
    const err = await onSave(result.binding);
    if (err) {
      setSaveError(err);
      captureCommand("start_hotkey_capture").catch(console.error);
    }
  };

  return (
    <div className="flex flex-col items-end gap-1.5">
      <div className="flex items-center gap-2">
        <div className="flex h-[29px] min-w-[132px] items-center rounded-[4px] border border-border-accent/60 bg-background-input px-[2px]">
          {keys ? (
            <Keycaps labels={bindingLabels(keys)} />
          ) : (
            <span className="gs-text-body-md-regular px-2 text-text-disabled">{t("Press keys…")}</span>
          )}
        </div>
        <OutlineButton onClick={onCancel}>{t("Cancel")}</OutlineButton>
        <LimeButton onClick={save} disabled={!result || !!result.error}>
          {t("Save")}
        </LimeButton>
      </div>
      {result?.error && <NoteText tone="error">{t(result.error)}</NoteText>}
      {result?.warning && <NoteText tone="warning">{t(result.warning)}</NoteText>}
      {saveError && <NoteText tone="error">{t(saveError)}</NoteText>}
    </div>
  );
}

/** Whisper's languages (ISO-639-1, plus haw/yue); names come from Intl in the app language. */
export const WHISPER_LANGUAGES = (
  "en zh de es ru ko fr ja pt tr pl ca nl ar sv it id hi fi vi he uk el ms cs ro da hu ta no th ur hr bg lt la mi ml cy " +
  "sk te fa lv bn sr az sl kn et mk br eu is hy ne mn bs kk sq sw gl mr pa si km sn yo so af oc ka be tg sd gu am yi lo " +
  "uz fo ht ps tk nn mt sa lb my bo tl mg as tt haw ln ha ba jw su yue"
).split(" ");

export function languageName(code: string): string {
  try {
    const name = new Intl.DisplayNames([locale()], { type: "language" }).of(code === "jw" ? "jv" : code);
    return name ? name.charAt(0).toLocaleUpperCase(locale()) + name.slice(1) : code;
  } catch {
    return code;
  }
}

/** Pick one language for accuracy, or several and let Whisper detect which (none = any). */
function LanguagePicker({
  selected,
  onSave,
  onClose,
}: {
  selected: string[];
  onSave: (languages: string[]) => Promise<string | null>;
  onClose: () => void;
}) {
  const [picked, setPicked] = useState<string[]>(selected);
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string | null>(null);
  const all = useMemo(
    () => WHISPER_LANGUAGES.map((code) => ({ code, name: languageName(code) })).sort((a, b) => a.name.localeCompare(b.name, locale())),
    [],
  );
  const q = query.trim().toLowerCase();
  const shown = [
    ...all.filter((l) => picked.includes(l.code)),
    ...all.filter((l) => !picked.includes(l.code)),
  ].filter((l) => !q || l.name.toLowerCase().includes(q) || l.code === q);
  const toggle = (code: string) =>
    setPicked((p) => (p.includes(code) ? p.filter((c) => c !== code) : [...p, code]));
  const summary =
    picked.length === 0
      ? t("Any language (auto-detect)")
      : picked.length === 1
        ? t("{language} only, for the best accuracy", { language: languageName(picked[0]) })
        : t("Detects which of these {n} you speak", { n: picked.length });

  return (
    <Modal onClose={onClose} width="w-[480px] max-w-[92vw]">
      <div className="flex max-h-[76vh] flex-col">
        <div className="flex items-center justify-between px-6 pt-5">
          <h2 className="gs-text-heading-md text-text-default">{t("Dictation languages")}</h2>
          <IconButton onClick={onClose} title={t("Close")}>
            <X className="size-4" />
          </IconButton>
        </div>
        <p className="gs-text-body-sm-regular px-6 pt-1 text-text-subdued">{summary}</p>
        <div className="mx-6 mt-4 flex items-center gap-2 rounded-[4px] border border-border-default bg-background-input px-2.5 py-1.5">
          <Search className="size-3.5 text-text-subdued" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t("Search languages...")}
            autoFocus
            className="gs-text-body-md-regular min-w-0 flex-1 bg-transparent text-text-default placeholder:text-text-disabled focus:outline-none"
          />
        </div>
        <div className="mt-2 min-h-0 flex-1 overflow-y-auto px-3 pb-2">
          {shown.map((l) => {
            const on = picked.includes(l.code);
            return (
              <button
                key={l.code}
                type="button"
                onClick={() => toggle(l.code)}
                className="flex w-full items-center justify-between gap-3 rounded-[4px] px-3 py-2 text-left transition-colors hover:bg-transparent-secondary"
              >
                <span className={`gs-text-body-md-regular ${on ? "text-text-default" : "text-text-subdued"}`}>{l.name}</span>
                {on && <Check className="size-4 text-text-accent" />}
              </button>
            );
          })}
        </div>
        {error && <div className="px-6"><NoteText tone="error">{t(error)}</NoteText></div>}
        <div className="flex items-center justify-between gap-2 border-t border-border-default px-6 py-4">
          <OutlineButton onClick={() => setPicked([])}>{t("Any language")}</OutlineButton>
          <div className="flex gap-2">
            <OutlineButton onClick={onClose}>{t("Cancel")}</OutlineButton>
            <LimeButton
              onClick={async () => {
                const err = await onSave(picked);
                if (err) setError(err);
                else onClose();
              }}
            >
              {t("Save")}
            </LimeButton>
          </div>
        </div>
      </div>
    </Modal>
  );
}

export const BAR_LOCATIONS = [
  { value: "bottom", label: "Bottom" },
  { value: "raised", label: "Raised" },
  { value: "high", label: "High" },
];

const THEMES = [
  { value: "dark", label: "Dark" },
  { value: "light", label: "Light" },
  { value: "system", label: "System" },
];

const TAB_ICON_CUT = "[&>:not(:first-child)]:fill-[var(--color-icon-cut-tab)] [&>:not(:first-child)]:stroke-[var(--color-icon-cut-tab)]";

const HOTKEY_FIELDS: HotkeyField[] = ["hotkey_hold", "hotkey_toggle", "commands_hold", "commands_toggle"];

export function SettingsModal({
  config,
  onSave,
  onClose,
  initialTab = "dictation",
}: {
  config: TranscriptionConfig;
  onSave: SaveConfig;
  onClose: () => void;
  initialTab?: SettingsTab;
}) {
  const [tab, setTab] = useState<SettingsTab>(initialTab);
  const [languagesOpen, setLanguagesOpen] = useState(false);
  const [errors, setErrors] = useState<Partial<Record<keyof TranscriptionConfig, string>>>({});
  const [devices, setDevices] = useState<string[]>([]);
  const [hotkeyStatus, setHotkeyStatus] = useState<HotkeyStatus | null>(null);
  const [hotkeyChecks, setHotkeyChecks] = useState<Partial<Record<HotkeyField, CaptureResult>>>({});
  const [recording, setRecording] = useState<HotkeyField | null>(null);
  const updates = useAppUpdates();

  useEffect(() => {
    invoke<string[]>("list_input_devices").then(setDevices).catch(console.error);
    invoke<HotkeyStatus>("get_hotkey_status").then(setHotkeyStatus).catch(console.error);
    const unlisten = listen<HotkeyStatus>("hotkey-status", (e) => setHotkeyStatus(e.payload));
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // Conflict warnings / platform errors for the active bindings.
  useEffect(() => {
    let alive = true;
    for (const field of HOTKEY_FIELDS) {
      const binding = config[field];
      const set = (r: CaptureResult) => alive && setHotkeyChecks((prev) => ({ ...prev, [field]: r }));
      if (!binding) {
        set({ binding });
        continue;
      }
      invoke<string | null>("check_hotkey", { binding })
        .then((warning) => set({ binding, warning }))
        .catch((e) => set({ binding, error: String(e) }));
    }
    return () => {
      alive = false;
    };
  }, [config.hotkey_hold, config.hotkey_toggle, config.commands_hold, config.commands_toggle]);

  const save = async (patch: Partial<TranscriptionConfig>) => {
    const err = await onSave(patch);
    setErrors((prev) => {
      const next = { ...prev };
      for (const key of Object.keys(patch) as (keyof TranscriptionConfig)[]) {
        if (err) next[key] = err;
        else delete next[key];
      }
      return next;
    });
    return err;
  };
  const errorNote = (key: keyof TranscriptionConfig) =>
    errors[key] && <NoteText tone="error">{t(errors[key])}</NoteText>;

  const tabs: { id: SettingsTab; label: string; icon: IconType; filled?: boolean }[] = [
    { id: "dictation", label: t("Dictation"), icon: Captions, filled: true },
    { id: "hotkeys", label: t("Hotkeys"), icon: Command },
    { id: "general", label: t("General"), icon: Settings2 },
    { id: "model", label: t("Model"), icon: Cpu },
  ];

  const deviceOptions = [
    { value: null, label: t("System default") },
    ...devices.map((d) => ({ value: d, label: d })),
  ];
  if (config.input_device && !devices.includes(config.input_device)) {
    deviceOptions.push({ value: config.input_device, label: t("{name} (unavailable)", { name: config.input_device }) });
  }

  const hotkeyRow: HotkeyRow = (field, icon, title, description) => {
    const check = hotkeyChecks[field];
    return (
      <SettingsRow
        icon={icon}
        title={title}
        description={description}
        note={
          recording !== field &&
          (check?.error || check?.warning) && (
            <>
              {check?.error && <NoteText tone="error">{t(check.error)}</NoteText>}
              {check?.warning && <NoteText tone="warning">{t(check.warning)}</NoteText>}
            </>
          )
        }
      >
        {recording === field ? (
          <HotkeyRecorder
            onCancel={() => setRecording(null)}
            onSave={async (binding) => {
              const err = await save({ [field]: binding });
              if (!err) setRecording(null);
              return err;
            }}
          />
        ) : (
          <div className="flex items-center gap-2">
            <IconButton onClick={() => setRecording(field)} title={t("Edit hotkey")}>
              <Pencil className="size-5" />
            </IconButton>
            {config[field] ? (
              <Keycaps labels={bindingLabels(config[field])} />
            ) : (
              <span className="gs-text-body-md-regular text-text-disabled">{t("Disabled")}</span>
            )}
          </div>
        )}
      </SettingsRow>
    );
  };

  const usesFn = HOTKEY_FIELDS.some((f) => config[f].split("+").includes("Fn"));

  return (
    <Modal
      onClose={onClose}
      width="flex h-[calc(100vh-122px)] min-h-[min(500px,calc(100vh-40px))] max-h-[900px] w-[calc(100vw-288px)] min-w-[min(720px,calc(100vw-40px))] max-w-[1200px] flex-col"
    >
      {/* Header */}
      <div className="flex shrink-0 items-center justify-between px-6 pt-6">
        <h2 className="text-[20px] leading-7 font-medium tracking-[0.01em] text-text-default">{t("Settings")}</h2>
        <IconButton onClick={onClose} title={t("Close settings")}>
          <X className="size-3.5" strokeWidth={1.5} />
        </IconButton>
      </div>

      {/* Body */}
      <div className="flex min-h-0 flex-1 gap-12 pt-6 pl-6">
        {/* Tab list */}
        <div className="flex w-[244px] max-w-[22vw] shrink-0 flex-col gap-1">
          {tabs.map((tb) => {
            const Icon = tb.icon;
            const active = tab === tb.id;
            return (
              <button
                key={tb.id}
                type="button"
                onClick={() => setTab(tb.id)}
                className={`flex h-10 items-center gap-3 rounded-[4px] pl-2.5 transition-colors ${
                  active
                    ? "bg-transparent-primary text-text-default"
                    : "text-text-subdued hover:bg-transparent-secondary hover:text-text-default"
                }`}
              >
                <Icon
                  className={`size-[18px] shrink-0 ${active ? "text-text-accent" : ""} ${active && tb.filled ? TAB_ICON_CUT : ""}`}
                  fill={active && tb.filled ? "currentColor" : "none"}
                />
                <span className="gs-text-body-md-regular">{tb.label}</span>
              </button>
            );
          })}
        </div>

        {/* Content */}
        <div className="flex min-w-0 flex-1 flex-col overflow-y-auto pr-[44px] pb-6 [scrollbar-gutter:stable]">
          <h3 className="gs-text-heading-md text-text-default">
            {tabs.find((tb) => tb.id === tab)?.label}
          </h3>

          {tab === "dictation" && (
            <>
              <SettingsSection label={t("Input")} />
              <SettingsRow
                icon={Mic}
                title={t("Microphone")}
                description={t("Select your input device")}
                note={errorNote("input_device")}
              >
                <Dropdown
                  value={config.input_device ?? null}
                  options={deviceOptions}
                  onChange={(v) => save({ input_device: v })}
                  className="w-[346px] max-w-full"
                />
              </SettingsRow>
              <SettingsSection label={t("Language")} />
              <SettingsRow
                icon={Languages}
                title={t("Dictation language")}
                description={t("The language you speak when dictating")}
                note={errorNote("languages")}
              >
                <OutlineButton onClick={() => setLanguagesOpen(true)}>{t("Change")}</OutlineButton>
              </SettingsRow>
              <SettingsSection label={t("Behavior")} />
              <SettingsRow
                icon={Clipboard}
                title={t("Copy to clipboard")}
                description={t("Also copy transcribed text to the clipboard after pasting")}
                note={errorNote("copy_to_clipboard")}
              >
                <Toggle
                  on={config.copy_to_clipboard}
                  onChange={(v) => save({ copy_to_clipboard: v })}
                  label={t("Copy to clipboard")}
                />
              </SettingsRow>
              <SettingsRow
                icon={VolumeX}
                title={IS_MAC ? t("Mute background") : t("Mute system audio")}
                description={t("Mute your default output device while OpenGlaido is recording")}
                note={errorNote("mute_background")}
              >
                <Toggle
                  on={config.mute_background}
                  onChange={(v) => save({ mute_background: v })}
                  label={t("Mute background")}
                />
              </SettingsRow>
              <SettingsRow
                icon={Music}
                title={t("Interaction sounds")}
                description={t("Play audio feedback when recording starts and stops")}
                note={errorNote("sound_feedback")}
              >
                <Toggle
                  on={config.sound_feedback}
                  onChange={(v) => save({ sound_feedback: v })}
                  label={t("Interaction sounds")}
                />
              </SettingsRow>
              <SettingsRow
                icon={MapPin}
                title={t("Dictation bar location")}
                description={t("Choose how high the dictation bar appears on screen")}
                note={errorNote("bar_location")}
              >
                <Dropdown
                  value={config.bar_location}
                  options={BAR_LOCATIONS.map((o) => ({ ...o, label: t(o.label) }))}
                  onChange={(v) => v && save({ bar_location: v })}
                  className="w-[132px]"
                />
              </SettingsRow>
            </>
          )}

          {tab === "hotkeys" && (
            <>
              {hotkeyStatus && (!hotkeyStatus.permission_granted || hotkeyStatus.error) && (
                <div className="mt-4 flex flex-wrap items-center gap-3 rounded-[6px] border border-border-default bg-transparent-primary px-3 py-2.5">
                  <TriangleAlert className="size-4 shrink-0 text-text-accent" />
                  <div className="flex min-w-[200px] flex-1 flex-col">
                    {!hotkeyStatus.permission_granted && (
                      <span className="gs-text-body-md-regular text-text-default">
                        {t("OpenGlaido needs Accessibility access to detect hotkeys.")}
                      </span>
                    )}
                    {hotkeyStatus.error && (
                      <span className="gs-text-body-sm-regular text-text-error select-text">
                        {t(hotkeyStatus.error)}
                      </span>
                    )}
                  </div>
                  {!hotkeyStatus.permission_granted && (
                    <OutlineButton
                      onClick={() => invoke("open_accessibility_settings").catch(console.error)}
                    >
                      {t("Open Accessibility settings")}
                    </OutlineButton>
                  )}
                </div>
              )}
              <SettingsSection label={t("Dictation")} />
              {hotkeyRow("hotkey_hold", AudioLines, t("Dictation"), t("Hold to speak. OpenGlaido inserts what you say."))}
              {hotkeyRow(
                "hotkey_toggle",
                AudioWaveform,
                t("Hands-free dictation"),
                t("Press once to dictate hands-free. Press again to stop."),
              )}
              {usesFn && (
                <div className="pr-3 pb-2 pl-[68px]">
                  <NoteText tone="hint">
                    {t("Set System Settings › Keyboard › “Press 🌐 key to” to “Do Nothing” so fn doesn't open the emoji picker.")}
                  </NoteText>
                </div>
              )}
              <CommandsHotkeySection config={config} hotkeyRow={hotkeyRow} />
              <SettingsSection label={t("App")} />
              <SettingsRow
                icon={Command}
                title={t("Enter to stop and paste")}
                description={t("While dictating hands-free, Enter stops the session and pastes the result")}
                note={errorNote("enter_to_stop")}
              >
                <Toggle
                  on={config.enter_to_stop}
                  onChange={(v) => save({ enter_to_stop: v })}
                  label={t("Enter to stop and paste")}
                />
              </SettingsRow>
              <SettingsRow icon={Settings} title={t("Settings")} description={t("Open this window")}>
                <Keycaps labels={[IS_MAC ? "⌘" : "Ctrl", ","]} />
              </SettingsRow>
            </>
          )}

          {tab === "general" && (
            <>
              <SettingsSection label={t("Appearance")} />
              <SettingsRow icon={SunMoon} title={t("Theme")} description={t("Choose between dark, light, or system theme")} note={errorNote("theme")}>
                <Dropdown
                  value={config.theme}
                  options={THEMES.map((o) => ({ ...o, label: t(o.label) }))}
                  onChange={(v) => v && save({ theme: v as TranscriptionConfig["theme"] })}
                  className="w-[132px]"
                />
              </SettingsRow>
              <SettingsRow
                icon={Languages}
                title={t("App language")}
                description={t("Language used for menus, settings, and notifications")}
                note={errorNote("app_language")}
              >
                <Dropdown
                  value={config.app_language}
                  options={[{ value: "system", label: t("System default") }, ...LOCALES.map((l) => ({ value: l.code, label: l.label }))]}
                  onChange={(v) => v && save({ app_language: v })}
                  className="w-[180px]"
                />
              </SettingsRow>
              <SettingsSection label={t("System")} />
              <UpdateSettings {...updates} />
              <SettingsRow
                icon={Power}
                title={t("Launch app at login")}
                description={t("Start OpenGlaido automatically when you log in")}
                note={errorNote("launch_at_login")}
              >
                <Toggle
                  on={config.launch_at_login}
                  onChange={(v) => save({ launch_at_login: v })}
                  label={t("Launch app at login")}
                />
              </SettingsRow>
              <SettingsRow
                icon={Ellipsis}
                title={IS_MAC ? t("Show in Menu bar") : t("Show tray icon")}
                description={IS_MAC ? t("Display the OpenGlaido icon in the menu bar") : t("Show the OpenGlaido icon in the notification area")}
                note={errorNote("show_in_menu_bar")}
              >
                <Toggle
                  on={config.show_in_menu_bar}
                  onChange={(v) => save({ show_in_menu_bar: v })}
                  label={t("Show in Menu bar")}
                />
              </SettingsRow>
              {IS_MAC && (
                <SettingsRow
                  icon={Dock}
                  title={t("Show in Dock")}
                  description={t("Keep OpenGlaido visible in the Dock and app switcher")}
                  note={errorNote("show_in_dock")}
                >
                  <Toggle
                    on={config.show_in_dock}
                    onChange={(v) => save({ show_in_dock: v })}
                    label={t("Show in Dock")}
                  />
                </SettingsRow>
              )}
              <SettingsSection label={t("Beta features")} />
              <SettingsRow
                icon={FlaskConical}
                title={t("Enable beta features")}
                description={t("Try early features that may change or behave unexpectedly. Reveals the Commands screen.")}
                note={errorNote("beta_features")}
              >
                <Toggle
                  on={config.beta_features}
                  onChange={(v) => save({ beta_features: v })}
                  label={t("Enable beta features")}
                />
              </SettingsRow>
            </>
          )}

          {tab === "model" && <ModelSettings config={config} save={save} errorNote={errorNote} />}
        </div>
      </div>

      {/* Footer */}
      <div className="shrink-0 pt-[30px] pb-[22px] pl-[34px]">
        {updates.status && <span className="gs-text-body-xs-regular text-text-disabled">{t("Version {v}", { v: updates.status.current_version })}</span>}
      </div>
      {languagesOpen && (
        <LanguagePicker
          selected={config.languages}
          onSave={(languages) => save({ languages })}
          onClose={() => setLanguagesOpen(false)}
        />
      )}
    </Modal>
  );
}
