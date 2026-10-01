import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Captions, CircleAlert, Settings, TriangleAlert, X } from "lucide-react";
import { CommandWindow } from "./command-window";
import { CommandsPage } from "./commands-page";
import { DictionaryPage } from "./dictionary";
import { FormattingPage } from "./formatting";
import { Home } from "./home";
import { homeKeyChecks, homeKeyWarnings, type HomeKeyResult } from "./home-key-warnings";
import { Hud, type BarMessage } from "./hud";
import { CommandPalette } from "./palette";
import { applyConfigPatch, editedModelKeys, scopedConfigPatch } from "./providers";
import { SettingsModal } from "./settings";
import { SnippetsPage } from "./snippets";
import type {
  DictionaryItem,
  HistoryItem,
  HistoryStats,
  IconType,
  Page,
  PaletteAction,
  SaveConfig,
  SettingsTab,
  SnippetItem,
  TranscriptionConfig,
} from "./types";
import { BookSpine, CommandsIcon, DialogPortal, HouseDoor, SnippetIcon } from "./ui";
import { resolveLocale, setLocale, t } from "./i18n";

/** Applies Dark/Light/System to <html data-theme> (System follows the OS live). */
export function useTheme(theme: TranscriptionConfig["theme"] | undefined) {
  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      const resolved = theme === "system" ? (media.matches ? "dark" : "light") : (theme ?? "dark");
      document.documentElement.dataset.theme = resolved;
    };
    apply();
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [theme]);
}

// Active Glaido icons are filled lime with their inner strokes cut out in the background colour.
const NAV_ICON_CUT = "[&>:not(:first-child)]:fill-[var(--color-icon-cut-nav)] [&>:not(:first-child)]:stroke-[var(--color-icon-cut-nav)]";
type MainToast = BarMessage & { source: "dictation" | "model-key" };

// Every window loads this bundle; the hash picks its UI and never changes.
export default function App() {
  if (window.location.hash === "#hud") return <Hud />;
  if (window.location.hash === "#command") return <CommandWindow />;
  return <MainWindow />;
}

function MainWindow() {
  const [page, setPage] = useState<Page>("home");
  // The Settings tab to open on; null = closed.
  const [settingsTab, setSettingsTab] = useState<SettingsTab | null>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [config, setConfig] = useState<TranscriptionConfig | null>(null);

  const [history, setHistory] = useState<HistoryItem[]>([]);
  const [historyStats, setHistoryStats] = useState<HistoryStats>({ words: 0, duration_ms: 0, streak: 0 });
  const historyRef = useRef(history);
  const historyQuery = useRef("");
  const historyGeneration = useRef(0);
  const historyLoading = useRef(false);
  const [historyHasMore, setHistoryHasMore] = useState(false);
  const [dictionary, setDictionary] = useState<DictionaryItem[]>([]);
  const [snippets, setSnippets] = useState<SnippetItem[]>([]);

  // Modals opened from here (⌘K) and closed by Esc live here; the pages render them.
  const [openRecordId, setOpenRecordId] = useState<string | null>(null);
  const [addWordOpen, setAddWordOpen] = useState(false);
  const [addSnippetOpen, setAddSnippetOpen] = useState(false);

  const [toast, setToast] = useState<MainToast | null>(null);
  const toastRef = useRef<MainToast | null>(null);
  const toastTimer = useRef<number | undefined>(undefined);
  const dismissToast = () => {
    window.clearTimeout(toastTimer.current);
    toastRef.current = null;
    setToast(null);
  };
  const showToast = (message: MainToast) => {
    window.clearTimeout(toastTimer.current);
    toastRef.current = message;
    setToast(message);
    toastTimer.current = window.setTimeout(dismissToast, 5000);
  };
  const refreshHomeKeys = useRef(() => {});
  const warnedHomeKeys = useRef(new Set<string>());
  const [addRuleOpen, setAddRuleOpen] = useState(false);
  // A ⌘K result older than the loaded history page.
  const [extraRecord, setExtraRecord] = useState<HistoryItem | null>(null);
  // What the backend last applied, plus the saves still queued on top of it (shown right away).
  const confirmedRef = useRef<TranscriptionConfig | null>(null);
  const pendingRef = useRef<ReturnType<typeof scopedConfigPatch>[]>([]);
  const saveQueue = useRef<Promise<unknown>>(Promise.resolve());
  const showConfig = () => {
    const c = confirmedRef.current;
    if (c) setConfig(pendingRef.current.reduce<TranscriptionConfig>((a, p) => applyConfigPatch(a, p) ?? a, c));
  };
  const confirmConfig = (c: TranscriptionConfig) => {
    confirmedRef.current = c;
    showConfig();
  };

  // Language + theme come from settings; set the locale before anything below renders.
  const localeCode = resolveLocale(config?.app_language);
  setLocale(localeCode);
  useTheme(config?.theme);
  useEffect(() => {
    invoke("set_tray_labels", {
      microphone: t("Microphone"),
      systemDefault: t("System Default"),
      show: t("Show OpenGlaido"),
      quit: t("Quit"),
    }).catch(console.error);
  }, [localeCode]);

  // ---- data loaders ----
  const loadConfig = async () => {
    try {
      confirmConfig(await invoke<TranscriptionConfig>("get_config"));
    } catch (e) {
      console.error(e);
    }
  };
  const loadHistory = async (refreshKeys = true) => {
    const generation = ++historyGeneration.current;
    historyLoading.current = true;
    try {
      const [rows, stats] = await Promise.all([
        invoke<HistoryItem[]>("get_history_page", { query: historyQuery.current, limit: 100 }),
        invoke<HistoryStats>("get_history_stats"),
      ]);
      if (generation !== historyGeneration.current) return;
      historyRef.current = rows;
      setHistory(rows);
      setHistoryStats(stats);
      setHistoryHasMore(rows.length === 100);
      if (refreshKeys) refreshHomeKeys.current();
    } catch (e) {
      console.error(e);
    } finally {
      if (generation === historyGeneration.current) historyLoading.current = false;
    }
  };
  const loadMoreHistory = async () => {
    if (historyLoading.current || !historyHasMore) return;
    const last = historyRef.current[historyRef.current.length - 1];
    if (!last) return;
    const generation = historyGeneration.current;
    historyLoading.current = true;
    try {
      const rows = await invoke<HistoryItem[]>("get_history_page", {
        query: historyQuery.current, limit: 100, before: { created_at: last.created_at, id: last.id },
      });
      if (generation !== historyGeneration.current) return;
      historyRef.current = [...historyRef.current, ...rows];
      setHistory(historyRef.current);
      setHistoryHasMore(rows.length === 100);
    } catch (e) {
      console.error(e);
    } finally {
      if (generation === historyGeneration.current) historyLoading.current = false;
    }
  };
  const searchHistory = (query: string) => {
    if (historyQuery.current === query) return;
    historyQuery.current = query;
    void loadHistory(false);
  };
  const loadDictionary = async () => {
    try {
      setDictionary(await invoke<DictionaryItem[]>("get_dictionary"));
    } catch (e) {
      console.error(e);
    }
  };
  const loadSnippets = async () => {
    try {
      setSnippets(await invoke<SnippetItem[]>("get_snippets"));
    } catch (e) {
      console.error(e);
    }
  };

  // ---- events ----
  useEffect(() => {
    const unlistenDone = listen<string>("transcription-completed", () => loadHistory());
    const unlistenHistory = listen("history-updated", () => loadHistory());
    // Settings changed by the backend (MCP prefs, "Always allow").
    const unlistenConfig = listen<TranscriptionConfig>("config-changed", (e) => confirmConfig(e.payload));
    loadConfig();
    loadHistory();
    loadDictionary();
    loadSnippets();
    return () => {
      unlistenDone.then((f) => f());
      unlistenHistory.then((f) => f());
      unlistenConfig.then((f) => f());
    };
  }, []);

  // ---- main-window toasts (key warnings never go through the dictation HUD) ----
  useEffect(() => {
    const unlisten = listen<BarMessage>("dictation-error", (e) => {
      showToast({ ...e.payload, source: "dictation" });
    });
    return () => {
      window.clearTimeout(toastTimer.current);
      unlisten.then((f) => f());
    };
  }, []);

  const checks = homeKeyChecks(confirmedRef.current);
  const checkIdentity = JSON.stringify(checks.map((check) => check.id));
  const homeVisible = page === "home" && !settingsTab && pendingRef.current.length === 0;
  const homeCheckContext = useRef({ homeVisible, checkIdentity });
  homeCheckContext.current = { homeVisible, checkIdentity };
  useEffect(() => {
    warnedHomeKeys.current = homeKeyWarnings(checks, [], warnedHomeKeys.current, false).warned;
    if (!homeVisible || checks.length === 0) return;
    let alive = true;
    let inFlight = false;
    const refresh = async () => {
      if (inFlight || !document.hasFocus()) return;
      inFlight = true;
      const results = await Promise.all(checks.map(async (check): Promise<HomeKeyResult> => {
        try {
          const valid = await invoke<boolean>("validate_provider_key", { baseUrl: check.baseUrl, apiKey: check.apiKey });
          return { id: check.id, result: valid ? "verified" : "unsupported" };
        } catch {
          return { id: check.id, result: "failed" };
        }
      }));
      inFlight = false;
      const current = homeCheckContext.current;
      if (!alive || !document.hasFocus() || !current.homeVisible || current.checkIdentity !== checkIdentity) return;
      const result = homeKeyWarnings(homeKeyChecks(confirmedRef.current), results, warnedHomeKeys.current, toastRef.current?.source !== "dictation");
      warnedHomeKeys.current = result.warned;
      if (result.providers.length > 0) {
        showToast({
          source: "model-key", level: "warning",
          message: "{provider} API key could not be verified. Check it in Settings › Model.",
          vars: { provider: result.providers.join(" / ") },
        });
      } else if (results.every(({ result }) => result !== "failed") && toastRef.current?.source === "model-key") {
        dismissToast();
      }
    };
    refreshHomeKeys.current = refresh;
    void refresh();
    window.addEventListener("focus", refresh);
    return () => {
      alive = false;
      refreshHomeKeys.current = () => {};
      window.removeEventListener("focus", refresh);
      if (toastRef.current?.source === "model-key") dismissToast();
    };
  }, [checkIdentity, homeVisible]);

  // ---- keyboard shortcuts ----
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === ",") {
        e.preventDefault();
        setSettingsTab((v) => (v ? null : "dictation"));
        return;
      }
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPaletteOpen((v) => !v);
        return;
      }
      // Esc: every overlay closes itself (ui.tsx useEscape), topmost first.
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // A ⌘K record older than the loaded page refreshes with it (Retry, Retranscribe).
  useEffect(() => {
    const id = extraRecord?.id;
    if (!id) return;
    invoke<HistoryItem>("get_history_item", { id })
      .then((entry) => setExtraRecord((current) => current?.id === id ? entry : current))
      .catch(() => setExtraRecord((current) => current?.id === id ? null : current));
  }, [history]);

  // Turning beta features off hides the Commands screen.
  useEffect(() => {
    if (config && !config.beta_features && page === "commands") setPage("home");
  }, [config?.beta_features, page]);

  // Optimistic: the change shows right away (so the next edit builds on it, even while an earlier
  // save is still running). Saves run one at a time, each on top of what the backend applied;
  // a failed one drops out of what is shown.
  const saveConfig: SaveConfig = (patch) => {
    if (!config) return Promise.resolve("Settings are not loaded yet");
    const pending = scopedConfigPatch(config, patch);
    const editedSearchProvider = typeof patch.search_api_key === "string" ? (patch.search_provider ?? config.search_provider) : null;
    pendingRef.current.push(pending);
    showConfig();
    const run = async (): Promise<string | null> => {
      try {
        const base = confirmedRef.current;
        if (!base) return "Settings are not loaded yet";
        const newConfig = applyConfigPatch(base, pending);
        if (!newConfig) return "The provider changed before the API key was saved. Please try again.";
        if (editedSearchProvider && editedSearchProvider !== newConfig.search_provider) return "The provider changed before the API key was saved. Please try again.";
        confirmedRef.current = await invoke<TranscriptionConfig>("save_config", {
          newConfig,
          editedModelKeys: editedModelKeys(patch),
          editedSearchProvider,
        });
        return null;
      } catch (e) {
        console.error(e);
        return String(e);
      } finally {
        pendingRef.current.splice(pendingRef.current.indexOf(pending), 1);
        showConfig();
      }
    };
    const next = saveQueue.current.then(run, run);
    saveQueue.current = next;
    return next;
  };

  const navigate = (next: Page) => {
    setPaletteOpen(false);
    setPage(next);
  };
  const runPaletteAction = (action: PaletteAction) => {
    setPaletteOpen(false);
    switch (action.type) {
      case "settings":
        setSettingsTab("dictation");
        break;
      case "add_word":
        setPage("dictionary");
        setAddWordOpen(true);
        break;
      case "add_snippet":
        setPage("snippets");
        setAddSnippetOpen(true);
        break;
      case "add_rule":
        setPage("formatting");
        setAddRuleOpen(true);
        break;
      case "toggle_theme":
        // By what is on screen (System may currently be light).
        saveConfig({ theme: document.documentElement.dataset.theme === "light" ? "dark" : "light" });
        break;
      case "open_history":
        setPage("home");
        if (history.some((h) => h.id === action.id)) {
          setExtraRecord(history.find((h) => h.id === action.id)!);
        } else {
          invoke<HistoryItem>("get_history_item", { id: action.id }).then(setExtraRecord).catch(console.error);
        }
        setOpenRecordId(action.id);
        break;
    }
  };

  const navItems: { id: Page; label: string; icon: IconType; beta?: boolean }[] = [
    { id: "home", label: t("Home"), icon: HouseDoor },
    { id: "formatting", label: t("Formatting"), icon: Captions },
    { id: "dictionary", label: t("Dictionary"), icon: BookSpine },
    { id: "snippets", label: t("Snippets"), icon: SnippetIcon },
    ...(config?.beta_features ? [{ id: "commands" as const, label: t("Commands"), icon: CommandsIcon, beta: true }] : []),
  ];

  // ------------------------------------------------------------------
  // Main window
  // ------------------------------------------------------------------
  return (
    <div className="flex h-screen w-screen flex-col bg-background-primary text-text-default select-none">
      {/* Drag strip for the overlay title bar (traffic lights live here) */}
      <div data-tauri-drag-region className="h-10 w-full shrink-0" />

      <div className="flex min-h-0 flex-1 gap-8 px-4 pb-4 pt-5">
        {/* Sidebar */}
        <aside className="flex w-[224px] shrink-0 flex-col">
          <nav className="flex flex-1 flex-col gap-0.5">
            {navItems.map((item) => {
              const Icon = item.icon;
              const active = page === item.id;
              return (
                <button
                  key={item.id}
                  type="button"
                  onClick={() => setPage(item.id)}
                  className={`flex h-10 w-full shrink-0 items-center gap-2.5 rounded-[4px] px-2.5 transition-colors ${
                    item.id === "home" ? "mb-3" : ""
                  } ${
                    active
                      ? "bg-transparent-primary text-text-default"
                      : "text-text-subdued hover:bg-transparent-secondary hover:text-text-default"
                  }`}
                >
                  <Icon
                    className={`size-5 shrink-0 ${active ? `text-text-accent ${NAV_ICON_CUT}` : ""}`}
                    fill={active ? "currentColor" : "none"}
                  />
                  <span className="gs-text-body-md-regular">{item.label}</span>
                  {item.beta && (
                    <span className="gs-text-tag ml-auto rounded-[2px] bg-badge-surface px-1.5 py-[3px] text-badge-text">
                      {t("Beta")}
                    </span>
                  )}
                </button>
              );
            })}
          </nav>
          <button
            type="button"
            onClick={() => setSettingsTab("dictation")}
            className="flex h-10 w-full items-center gap-2.5 rounded-[4px] px-2.5 text-text-subdued transition-colors hover:bg-transparent-secondary hover:text-text-default"
          >
            <Settings className="size-5 shrink-0" />
            <span className="gs-text-body-md-regular">{t("Settings")}</span>
          </button>
        </aside>

        {/* Content */}
        <main className="flex min-w-0 flex-1 flex-col gap-4">
          {page === "home" && (
            <Home
              history={history}
              onError={(message) => showToast({ source: "dictation", level: "error", message })}
              historyStats={historyStats}
              onSearchHistory={searchHistory}
              onLoadMoreHistory={loadMoreHistory}
              hasMoreHistory={historyHasMore}
              onHistoryChanged={loadHistory}
              openRecordId={openRecordId}
              extraRecord={extraRecord}
              onOpenRecord={(id) => {
                setOpenRecordId(id);
                if (id) setExtraRecord(historyRef.current.find((item) => item.id === id) ?? null);
              }}
              onOpenSettings={setSettingsTab}
            />
          )}
          {page === "formatting" && config && (
            <FormattingPage config={config} onSave={saveConfig} addRuleOpen={addRuleOpen} onAddRuleOpenChange={setAddRuleOpen} />
          )}
          {page === "dictionary" && (
            <DictionaryPage
              dictionary={dictionary}
              onChanged={loadDictionary}
              addOpen={addWordOpen}
              onAddOpenChange={setAddWordOpen}
            />
          )}
          {page === "snippets" && (
            <SnippetsPage
              snippets={snippets}
              onChanged={loadSnippets}
              addOpen={addSnippetOpen}
              onAddOpenChange={setAddSnippetOpen}
            />
          )}
          {page === "commands" && config && <CommandsPage config={config} onSave={saveConfig} />}
        </main>
      </div>

      {/* ---------------- Modals ---------------- */}
      {settingsTab && config && (
        <SettingsModal config={config} onSave={saveConfig} onClose={() => setSettingsTab(null)} initialTab={settingsTab} />
      )}

      <CommandPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        onNavigate={navigate}
        onAction={runPaletteAction}
        config={config}
      />

      {toast && (
        <DialogPortal>
        <div
          role="status"
          className={`fixed right-4 bottom-4 z-[80] flex w-[360px] max-w-[calc(100vw-32px)] items-start gap-3 rounded-[8px] border bg-background-card bg-linear-to-b p-4 shadow-2xl ${
            toast.level === "warning"
              ? "border-warning-border from-warning-surface to-warning-surface"
              : "border-error-border from-error-surface to-error-surface"
          }`}
        >
          {toast.level === "warning" ? (
            <TriangleAlert className="mt-0.5 size-4 shrink-0 text-text-warning" />
          ) : (
            <CircleAlert className="mt-0.5 size-4 shrink-0 text-text-error" />
          )}
          <p className="gs-text-body-md-regular min-w-0 flex-1 break-words text-text-default select-text">
            {t(toast.message, toast.vars)}
          </p>
          <button
            type="button"
            onClick={dismissToast}
            title={t("Dismiss")}
            className="shrink-0 text-text-disabled transition-colors hover:text-text-default"
          >
            <X className="size-3.5" />
          </button>
        </div>
        </DialogPortal>
      )}
    </div>
  );
}
