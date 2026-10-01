import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  BookOpen,
  BookPlus,
  Captions,
  FileText,
  ListPlus,
  MessageSquareText,
  Moon,
  Search,
  Settings,
  SquarePlus,
  Sun,
} from "lucide-react";
import { t } from "./i18n";
import type { HistoryItem, IconType, Page, PaletteAction, TranscriptionConfig } from "./types";
import { BookSpine, CommandsIcon, DIALOG_STYLE, DialogLayer, HouseDoor, IS_MAC, Keycaps, SnippetIcon, isPlainEnter, useEscape, useModalDialog } from "./ui";

export interface CommandPaletteProps {
  open: boolean;
  onClose: () => void;
  /** App switches the page and closes the palette. */
  onNavigate: (page: Page) => void;
  /** App runs the action and closes the palette. */
  onAction: (action: PaletteAction) => void;
  config: TranscriptionConfig | null;
}

interface Item {
  key: string;
  group: string;
  label: string;
  icon: IconType;
  /** Extra words that find this item ("personalization" → Formatting). */
  aliases?: string[];
  hint?: string[];
  run: () => void;
  sub?: string;
}

const DOCS_URL = "https://github.com/Legoless/OpenGlaido#readme";

/** ⌘K / Ctrl+K: jump anywhere, run actions, search transcriptions (≥ 2 characters, up to 15). */
export function CommandPalette({ open, onClose, onNavigate, onAction, config }: CommandPaletteProps) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<HistoryItem[]>([]);
  const [index, setIndex] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  const dialog = useRef<HTMLDialogElement>(null);
  useEscape(onClose, open);
  useModalDialog(dialog, open);

  useEffect(() => {
    if (open) {
      setQuery("");
      setResults([]);
      setIndex(0);
    }
  }, [open]);

  useEffect(() => {
    const q = query.trim();
    if (q.length < 2) {
      setResults([]);
      return;
    }
    let alive = true;
    const timer = window.setTimeout(() => {
      invoke<HistoryItem[]>("search_history", { query: q, limit: 15 })
        .then((r) => alive && setResults(r))
        .catch(console.error);
    }, 120);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [query]);

  // What is on screen (System may be showing light).
  const light = open && document.documentElement.dataset.theme === "light";
  const items = useMemo<Item[]>(() => {
    const nav = t("Navigation");
    const cmd = t("Commands");
    const base: Item[] = [
      { key: "home", group: nav, label: t("Home"), icon: HouseDoor, run: () => onNavigate("home") },
      { key: "formatting", group: nav, label: t("Formatting"), icon: Captions, aliases: ["personalization", "rules", "style"], run: () => onNavigate("formatting") },
      { key: "dictionary", group: nav, label: t("Dictionary"), icon: BookSpine, aliases: ["words", "vocabulary"], run: () => onNavigate("dictionary") },
      { key: "snippets", group: nav, label: t("Snippets"), icon: SnippetIcon, run: () => onNavigate("snippets") },
      ...(config?.beta_features
        ? [{ key: "commands", group: nav, label: t("Commands"), icon: CommandsIcon, aliases: ["tools", "mcp"], run: () => onNavigate("commands") }]
        : []),
      { key: "settings", group: nav, label: t("Settings"), icon: Settings, hint: [IS_MAC ? "⌘" : "Ctrl", ","], aliases: ["preferences", "hotkeys"], run: () => onAction({ type: "settings" }) },
      {
        key: "docs",
        group: nav,
        label: t("Documentation"),
        icon: BookOpen,
        aliases: ["help", "docs"],
        run: () => {
          openUrl(DOCS_URL).catch(console.error);
          onClose();
        },
      },
      { key: "theme", group: cmd, label: light ? t("Switch to dark mode") : t("Switch to light mode"), icon: light ? Moon : Sun, aliases: ["theme", "appearance"], run: () => onAction({ type: "toggle_theme" }) },
      { key: "add_word", group: cmd, label: t("Add words"), icon: BookPlus, aliases: ["dictionary"], run: () => onAction({ type: "add_word" }) },
      { key: "add_snippet", group: cmd, label: t("Add snippet"), icon: SquarePlus, run: () => onAction({ type: "add_snippet" }) },
      { key: "add_rule", group: cmd, label: t("Add formatting rule"), icon: ListPlus, aliases: ["rule"], run: () => onAction({ type: "add_rule" }) },
    ];
    const q = query.trim().toLowerCase();
    const matched = q
      ? base.filter((it) => it.label.toLowerCase().includes(q) || it.aliases?.some((a) => a.startsWith(q) || q.startsWith(a)))
      : base;
    const history: Item[] = results.map((r) => ({
      key: `h-${r.id}`,
      group: t("Transcriptions"),
      label: (r.kind === "command" ? r.text : r.text) || t("Transcription failed. Your audio is safe."),
      sub: new Date(r.created_at).toLocaleString(),
      icon: r.kind === "command" ? MessageSquareText : FileText,
      run: () => onAction({ type: "open_history", id: r.id }),
    }));
    return [...matched, ...history];
  }, [query, results, config?.beta_features, light]);

  useEffect(() => {
    setIndex(0);
  }, [query, results]);
  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(`[data-index="${index}"]`)?.scrollIntoView({ block: "nearest" });
  }, [index]);

  if (!open) return null;

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setIndex((i) => Math.min(items.length - 1, i + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setIndex((i) => Math.max(0, i - 1));
    } else if (isPlainEnter(e)) {
      e.preventDefault();
      items[index]?.run();
    }
  };

  let lastGroup = "";
  return (
    <DialogLayer>
    <dialog ref={dialog} role="dialog" tabIndex={-1} aria-modal="true" aria-label={t("Type a command or search...")} style={{ ...DIALOG_STYLE, paddingTop: "14vh" }} className="fixed inset-0 z-[90] flex items-start justify-center bg-black/30 pt-[14vh] backdrop:bg-transparent" onCancel={(e) => e.preventDefault()} onMouseDown={onClose}>
      <div
        className="flex max-h-[60vh] w-[560px] max-w-[90vw] flex-col overflow-hidden rounded-[12px] border border-border-default bg-modal-surface shadow-2xl backdrop-blur-xl"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-3 border-b border-border-default px-4 py-3">
          <Search className="size-4 shrink-0 text-text-subdued" />
          <input
            autoFocus
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder={t("Type a command or search...")}
            className="gs-text-body-md-regular min-w-0 flex-1 bg-transparent text-text-default placeholder:text-text-disabled focus:outline-none"
          />
        </div>
        <div ref={listRef} className="min-h-0 flex-1 overflow-y-auto p-1.5">
          {items.length === 0 && (
            <p className="gs-text-body-md-regular px-3 py-6 text-center text-text-disabled">{t("No results")}</p>
          )}
          {items.map((it, i) => {
            const header = it.group !== lastGroup ? it.group : null;
            lastGroup = it.group;
            const Icon = it.icon;
            return (
              <div key={it.key}>
                {header && <div className="gs-text-tag px-3 pt-2.5 pb-1.5 text-text-disabled">{header}</div>}
                <button
                  type="button"
                  data-index={i}
                  onMouseMove={() => setIndex(i)}
                  onClick={it.run}
                  className={`flex w-full items-center gap-3 rounded-[6px] px-3 py-2 text-left ${
                    i === index ? "bg-transparent-tertiary" : ""
                  }`}
                >
                  <Icon className="size-4 shrink-0 text-text-subdued" />
                  <span className="min-w-0 flex-1">
                    <span className="gs-text-body-md-regular block truncate text-text-default">{it.label}</span>
                    {it.sub && <span className="gs-text-body-sm-regular block truncate text-text-disabled">{it.sub}</span>}
                  </span>
                  {it.hint && <Keycaps labels={it.hint} />}
                </button>
              </div>
            );
          })}
        </div>
        <div className="gs-text-body-sm-regular flex items-center gap-4 border-t border-border-default px-4 py-2 text-text-disabled">
          <span className="flex items-center gap-1.5">
            <Keycaps labels={["↑", "↓"]} />
            {t("Navigate")}
          </span>
          <span className="flex items-center gap-1.5">
            <Keycaps labels={["↵"]} />
            {t("Select")}
          </span>
          <span className="flex items-center gap-1.5">
            <Keycaps labels={["ESC"]} />
            {t("Close")}
          </span>
        </div>
      </div>
    </dialog>
    </DialogLayer>
  );
}
