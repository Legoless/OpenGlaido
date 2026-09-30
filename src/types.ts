// Mirrors of the backend IPC types (src-tauri/src/transcribe.rs, db.rs, hotkeys/, commands/,
// frontmost.rs) and event payloads, snake_case like the Rust side.
import type { ReactNode } from "react";
import type { LucideIcon } from "lucide-react";

// ------------------------------------------------------------------
// Config
// ------------------------------------------------------------------
export type FormattingStyle = "standard" | "casual" | "lowercase";
export type ToolPolicy = "auto" | "ask" | "deny";

export interface AppRef {
  bundle_id: string;
  name: string;
}

export interface FormattingRule {
  id: string;
  name: string;
  enabled: boolean;
  apps: AppRef[];
  /** Hosts, suffix-matched against the browser URL. */
  websites: string[];
  style: FormattingStyle;
  raw_text: boolean;
  custom_prompt: string;
}

export interface McpServerPrefs {
  enabled: boolean;
  tool_policies: Record<string, ToolPolicy>;
}

export interface TranscriptionConfig {
  endpoint_url: string;
  /** Lives in the OS keychain; config.json never has it. */
  api_key: string;
  model_name: string;
  temperature?: number | null;
  llm_endpoint_url?: string | null;
  llm_model_name?: string | null;
  sound_feedback: boolean;
  hotkey_hold: string;
  hotkey_toggle: string;
  enter_to_stop: boolean;
  input_device?: string | null;
  copy_to_clipboard: boolean;
  bar_location: string;
  launch_at_login: boolean;
  show_in_menu_bar: boolean;
  show_in_dock: boolean;
  // Formatting ("All apps" + rules)
  style: FormattingStyle;
  raw_text: boolean;
  /** At most 500 characters (save_config rejects longer). */
  custom_prompt: string;
  email_rule: FormattingRule;
  custom_rules: FormattingRule[];
  /** Whisper ISO-639-1 codes: [] = auto-detect, exactly 1 = pinned, 2+ = auto-detect. */
  languages: string[];
  mute_background: boolean;
  show_bar_when_idle: boolean;
  theme: "dark" | "light" | "system";
  /** "system" or one of the UI locales ("en", "de", "sr-Latn", "zh-Hans", ...). */
  app_language: string;
  beta_features: boolean;
  // Commands
  commands_hold: string;
  commands_toggle: string;
  /** Built-in tool id -> enabled. */
  builtin_tools: Record<string, boolean>;
  search_provider: "none" | "brave" | "tavily" | "searxng";
  /** Lives in the OS keychain, like api_key. */
  search_api_key: string;
  searxng_url: string;
  mcp_servers: Record<string, McpServerPrefs>;
}

export interface HotkeyStatus {
  engine: "native" | "plugin";
  permission_granted: boolean;
  error: string | null;
}

// ------------------------------------------------------------------
// Data
// ------------------------------------------------------------------
export interface Source {
  title: string;
  url: string;
}

export interface HistoryItem {
  id: string;
  text: string;
  raw_text: string;
  duration_ms: number;
  audio_filename?: string | null;
  created_at: string;
  kind: "dictation" | "command";
  status: "ok" | "failed" | "running";
  error?: string | null;
  app_name?: string | null;
  app_bundle_id?: string | null;
  /** Command answer (markdown). */
  answer?: string | null;
  sources: Source[];
}

export interface DictionaryItem {
  id: string;
  phrase: string;
  replacement: string;
}

export interface SnippetItem {
  id: string;
  trigger: string;
  content: string;
}

/** list_apps / recent_apps. */
export interface AppInfo {
  bundle_id: string;
  name: string;
  path: string;
}

// ------------------------------------------------------------------
// Commands module (get_builtin_tools, mcp_*)
// ------------------------------------------------------------------
export interface BuiltinTool {
  id: string;
  group: "web" | "system" | "glaido";
  name: string;
  description: string;
  examples: string[];
  enabled: boolean;
  available: boolean;
  unavailable_reason: string | null;
}

export interface McpToolInfo {
  name: string;
  description: string;
  policy: ToolPolicy;
}

export interface McpServerInfo {
  name: string;
  enabled: boolean;
  status: "stopped" | "starting" | "running" | "failed";
  error: string | null;
  folder: string | null;
  description: string | null;
  tools: McpToolInfo[];
}

export interface RuntimeCheck {
  found: boolean;
  path: string | null;
  install_command: string;
}

export interface NewServerResult {
  folder: string;
  prompt: string;
}

export type ApprovalDecision = "once" | "always" | "deny";

// ------------------------------------------------------------------
// Events: "command-state", "command-window"
// ------------------------------------------------------------------
export type CommandPhase =
  | "listening"
  | "transcribing"
  | "thinking"
  | "tool"
  | "streaming"
  | "done"
  | "error"
  | "approval";

export interface CommandToolRun {
  name: string;
  label: string;
  /** files_apps action. */
  action: string;
  /** Query / subject, for the translated label. */
  detail: string;
  status: "running" | "done" | "failed";
}

export interface CommandApproval {
  request_id: string;
  server: string;
  tool: string;
  summary: string;
  expires_in_s: number;
}

export interface CommandState {
  id: string;
  phase: CommandPhase;
  instruction: string;
  has_selection: boolean;
  /** Markdown answer so far. */
  text: string;
  tools: CommandToolRun[];
  approval: CommandApproval | null;
  error: string | null;
  /** Binding string for the refine footer chip. */
  refine_keys: string;
  /** A finished answer: Enter pastes, ⌘C copies. */
  answer_ready: boolean;
}

export interface CommandWindowEvent {
  open: boolean;
}

// ------------------------------------------------------------------
// UI
// ------------------------------------------------------------------
export type Page = "home" | "formatting" | "dictionary" | "snippets" | "commands";
export type SettingsTab = "dictation" | "hotkeys" | "general" | "model";
export type IconType = LucideIcon;
/** Resolves to an error message (null on success); the caller shows it where the change was made. */
export type SaveConfig = (patch: Partial<TranscriptionConfig>) => Promise<string | null>;

export type HotkeyField = "hotkey_hold" | "hotkey_toggle" | "commands_hold" | "commands_toggle";
/** Settings › Hotkeys row (pencil + chips + recorder + warnings) for one binding field. */
export type HotkeyRow = (field: HotkeyField, icon: IconType, title: string, description: string) => ReactNode;

/** ⌘K palette actions, handled by App (which also closes the palette). */
export type PaletteAction =
  | { type: "settings" }
  | { type: "add_word" }
  | { type: "add_snippet" }
  | { type: "add_rule" }
  | { type: "toggle_theme" }
  | { type: "open_history"; id: string };
