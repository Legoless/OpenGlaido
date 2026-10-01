import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  AppWindow,
  BookOpen,
  Calculator,
  Check,
  Copy,
  FileText,
  FolderOpen,
  Globe,
  History,
  Plus,
  Puzzle,
  RefreshCw,
  Search,
  Telescope,
  Trash2,
  TriangleAlert,
  MonitorPlay,
  X,
} from "lucide-react";
import { t } from "./i18n";
import type { BuiltinTool, McpServerInfo, NewServerResult, RuntimeCheck, SaveConfig, ToolPolicy, TranscriptionConfig } from "./types";
import { Dropdown, EmptyState, HeaderCard, IconButton, LimeButton, Modal, NoteText, OutlineButton, TextInput, Toggle } from "./ui";

export interface CommandsPageProps {
  config: TranscriptionConfig;
  onSave: SaveConfig;
}

const TOOL_ICONS: Record<string, typeof Globe> = {
  web_search: Search,
  read_page: FileText,
  youtube: MonitorPlay,
  deep_research: Telescope,
  math_dates: Calculator,
  files_apps: AppWindow,
  history_search: History,
  docs_search: BookOpen,
};

const GROUPS: { id: BuiltinTool["group"]; label: string }[] = [
  { id: "web", label: "Web" },
  { id: "system", label: "System" },
  { id: "glaido", label: "OpenGlaido" },
];

/** Commands screen (sidebar, beta): built-in commands and custom tools (local MCP servers). */
export function CommandsPage({ config, onSave }: CommandsPageProps) {
  const [tab, setTab] = useState<"builtin" | "custom">("builtin");
  return (
    <>
      <HeaderCard
        title={t("Connect OpenGlaido to the rest of your work")}
        description={t("Start with the built-in commands, or add your own tools for anything else.")}
      />
      <div className="flex min-h-0 flex-1 flex-col rounded-[8px] bg-background-card p-8">
        <div className="flex shrink-0 items-center gap-1 self-start rounded-[6px] bg-transparent-primary p-1">
          {(["builtin", "custom"] as const).map((id) => (
            <button
              key={id}
              type="button"
              onClick={() => setTab(id)}
              className={`gs-text-body-md-regular rounded-[4px] px-3 py-1 transition-colors ${
                tab === id ? "bg-transparent-tertiary text-text-default" : "text-text-subdued hover:text-text-default"
              }`}
            >
              {id === "builtin" ? t("Built in") : t("Custom")}
            </button>
          ))}
        </div>
        <div className="mt-6 min-h-0 flex-1 overflow-y-auto pr-2">
          {tab === "builtin" ? <BuiltIn config={config} onSave={onSave} /> : <Custom />}
        </div>
      </div>
    </>
  );
}

// ------------------------------------------------------------------
// Built in
// ------------------------------------------------------------------
function BuiltIn({ config, onSave }: CommandsPageProps) {
  const [tools, setTools] = useState<BuiltinTool[]>([]);
  const [open, setOpen] = useState<BuiltinTool | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = () => invoke<BuiltinTool[]>("get_builtin_tools").then(setTools).catch(console.error);
  useEffect(() => {
    load();
  }, [config.builtin_tools, config.search_provider, config.search_api_key, config.searxng_url]);

  const setEnabled = async (id: string, enabled: boolean) => {
    setError(await onSave({ builtin_tools: { ...config.builtin_tools, [id]: enabled } }));
  };

  return (
    <div className="flex flex-col gap-7">
      {GROUPS.map((group) => {
        const items = tools.filter((tl) => tl.group === group.id);
        const unavailable = items.length > 0 && items.every((tl) => !tl.available);
        const someUnavailable = items.some((tl) => !tl.available);
        return (
          <section key={group.id} className="flex flex-col gap-3">
            <div className="flex items-center gap-2">
              <span className="gs-text-tag text-text-subdued">{t(group.label)}</span>
              {(unavailable || someUnavailable) && (
                <span className="gs-text-tag rounded-[2px] bg-transparent-tertiary px-1.5 py-[3px] text-text-disabled">
                  {unavailable ? t("Not available yet") : t("Needs setup")}
                </span>
              )}
            </div>
            <div className="grid grid-cols-2 gap-3">
              {items.map((tool) => {
                const Icon = TOOL_ICONS[tool.id] ?? Globe;
                return (
                  <div
                    key={tool.id}
                    role="button"
                    tabIndex={0}
                    onClick={() => setOpen(tool)}
                    onKeyDown={(e) => e.key === "Enter" && setOpen(tool)}
                    className={`flex cursor-default items-start gap-3 rounded-[8px] border border-border-default p-4 transition-colors hover:bg-transparent-secondary ${
                      tool.available ? "" : "opacity-50"
                    }`}
                  >
                    <div className="flex size-9 shrink-0 items-center justify-center rounded-[6px] bg-transparent-tertiary">
                      <Icon className="size-[18px] text-text-subdued" />
                    </div>
                    <div className="min-w-0 flex-1">
                      <p className="gs-text-body-md-medium text-text-default">{t(tool.name)}</p>
                      <p className="gs-text-body-sm-regular line-clamp-2 text-text-subdued">{t(tool.description)}</p>
                    </div>
                    <span onClick={(e) => e.stopPropagation()}>
                      {tool.available ? (
                        <Toggle on={tool.enabled} onChange={(v) => setEnabled(tool.id, v)} label={t(tool.name)} />
                      ) : null}
                    </span>
                  </div>
                );
              })}
            </div>
          </section>
        );
      })}
      {error && <NoteText tone="error">{t(error)}</NoteText>}
      {open && <ToolDialog tool={open} config={config} onSave={onSave} onClose={() => setOpen(null)} />}
    </div>
  );
}

const PROVIDERS = [
  { value: "none", label: "None" },
  { value: "brave", label: "Brave Search" },
  { value: "tavily", label: "Tavily" },
  { value: "searxng", label: "SearXNG" },
];

function ToolDialog({ tool, config, onSave, onClose }: { tool: BuiltinTool } & CommandsPageProps & { onClose: () => void }) {
  const needsSearch = tool.id === "web_search" || tool.id === "deep_research";
  const [provider, setProvider] = useState<string>(config.search_provider);
  const [key, setKey] = useState(config.search_api_key);
  const keyDrafts = useRef(new Map([[config.search_provider as string, config.search_api_key]]));
  const selectedProvider = useRef(provider);
  const [loadingKey, setLoadingKey] = useState(false);
  const [keyLoadFailed, setKeyLoadFailed] = useState(false);
  const [url, setUrl] = useState(config.searxng_url);
  const [error, setError] = useState<string | null>(null);
  const Icon = TOOL_ICONS[tool.id] ?? Globe;

  const selectProvider = async (next: string) => {
    selectedProvider.current = next;
    setProvider(next);
    setError(null);
    setKeyLoadFailed(false);
    setKey("");
    if (next !== "brave" && next !== "tavily") { setLoadingKey(false); return; }
    const cached = keyDrafts.current.get(next);
    if (cached !== undefined) { setKey(cached); setLoadingKey(false); return; }
    setLoadingKey(true);
    try {
      const saved = await invoke<string>("get_search_provider_key", { provider: next });
      if (!keyDrafts.current.has(next)) keyDrafts.current.set(next, saved);
      if (selectedProvider.current === next) setKey(keyDrafts.current.get(next)!);
    } catch (e) {
      if (selectedProvider.current === next) { setError(String(e)); setKeyLoadFailed(true); }
    } finally {
      if (selectedProvider.current === next) setLoadingKey(false);
    }
  };

  const save = async () => {
    const err = await onSave({
      search_provider: provider as TranscriptionConfig["search_provider"],
      search_api_key: key.trim(),
      searxng_url: url.trim(),
    });
    if (err) setError(err);
    else onClose();
  };

  return (
    <Modal onClose={onClose} width="w-[520px] max-w-[92vw]">
      <div className="flex flex-col gap-4 p-6">
        <div className="flex items-start gap-3">
          <div className="flex size-10 shrink-0 items-center justify-center rounded-[6px] bg-transparent-tertiary">
            <Icon className="size-5 text-text-subdued" />
          </div>
          <div className="min-w-0 flex-1">
            <h2 className="gs-text-heading-md text-text-default">{t(tool.name)}</h2>
            <p className="gs-text-body-md-regular text-text-subdued">{t(tool.description)}</p>
          </div>
          <IconButton onClick={onClose} title={t("Close")}>
            <X className="size-4" />
          </IconButton>
        </div>
        {!tool.available && tool.unavailable_reason && (
          <NoteText tone="warning">{t(tool.unavailable_reason)}</NoteText>
        )}
        <div className="flex flex-col gap-1.5">
          <span className="gs-text-tag text-text-subdued">{t("Try saying")}</span>
          {tool.examples.map((ex) => (
            <p key={ex} className="gs-text-body-md-regular text-text-default">“{t(ex)}”</p>
          ))}
        </div>
        {needsSearch && (
          <div className="flex flex-col gap-3 border-t border-border-default pt-4">
            <span className="gs-text-tag text-text-subdued">{t("Search provider")}</span>
            <Dropdown
              value={provider}
              options={PROVIDERS.map((p) => ({ ...p, label: t(p.label) }))}
              onChange={(v) => v && void selectProvider(v)}
              className="w-[220px]"
            />
            {(provider === "brave" || provider === "tavily") && (
              <TextInput type="password" value={key} onChange={(value) => { keyDrafts.current.set(provider, value); setKey(value); setKeyLoadFailed(false); setError(null); }} placeholder={t("API key")} className="w-full" />
            )}
            {provider === "searxng" && (
              <TextInput value={url} onChange={setUrl} placeholder="https://searx.example.org" className="w-full" />
            )}
            <p className="gs-text-body-sm-regular text-text-disabled">
              {t("Web search and deep research use this provider. The key is stored in your system keychain.")}
            </p>
            {error && <NoteText tone="error">{t(error)}</NoteText>}
            <div className="flex justify-end gap-2">
              <OutlineButton onClick={onClose}>{t("Cancel")}</OutlineButton>
              <LimeButton onClick={save} disabled={loadingKey || keyLoadFailed}>{t("Save")}</LimeButton>
            </div>
          </div>
        )}
      </div>
    </Modal>
  );
}

// ------------------------------------------------------------------
// Custom (local MCP servers)
// ------------------------------------------------------------------
function Custom() {
  const [servers, setServers] = useState<McpServerInfo[]>([]);
  const [open, setOpen] = useState<string | null>(null);
  const [wizard, setWizard] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const load = () => invoke<McpServerInfo[]>("mcp_list_servers").then(setServers).catch(console.error);

  useEffect(() => {
    load();
    // Coming back to the window picks up config edits made elsewhere.
    window.addEventListener("focus", load);
    return () => window.removeEventListener("focus", load);
  }, []);
  // Poll while a server is starting.
  useEffect(() => {
    if (!servers.some((s) => s.status === "starting")) return;
    const timer = window.setTimeout(load, 1000);
    return () => window.clearTimeout(timer);
  }, [servers]);

  const run = async (p: Promise<unknown>) => {
    try {
      await p;
      setError(null);
    } catch (e) {
      setError(String(e));
    }
    load();
  };
  const importFolder = async () => {
    const dir = await openDialog({ directory: true, multiple: false, title: t("Choose a folder with an mcp.json") });
    if (typeof dir === "string") run(invoke("mcp_import_folder", { path: dir }));
  };
  const current = servers.find((s) => s.name === open) ?? null;

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-2">
        <OutlineButton onClick={importFolder}>
          <FolderOpen className="size-3.5" />
          {t("Import folder")}
        </OutlineButton>
        <OutlineButton onClick={() => setWizard(true)}>
          <Plus className="size-3.5" />
          {t("New server")}
        </OutlineButton>
        <div className="flex-1" />
        <OutlineButton onClick={() => run(invoke("mcp_open_config"))}>{t("Edit config")}</OutlineButton>
      </div>
      {error && <NoteText tone="error">{t(error)}</NoteText>}
      {servers.length === 0 ? (
        <EmptyState
          icon={Puzzle}
          title={t("No custom tools yet")}
          description={t("Import a folder with an mcp.json, or create a new server and let a coding agent add the tools.")}
          actionLabel={t("New server")}
          onAction={() => setWizard(true)}
        />
      ) : (
        <div className="flex flex-col gap-3">
          <span className="gs-text-tag text-text-subdued">{t("Your servers")}</span>
          {servers.map((s) => (
            <div
              key={s.name}
              role="button"
              tabIndex={0}
              onClick={() => setOpen(s.name)}
              onKeyDown={(e) => e.key === "Enter" && setOpen(s.name)}
              className="flex cursor-default items-center gap-3 rounded-[8px] border border-border-default p-4 transition-colors hover:bg-transparent-secondary"
            >
              <div className="relative flex size-9 shrink-0 items-center justify-center rounded-[6px] bg-transparent-tertiary">
                <Puzzle className="size-[18px] text-text-subdued" />
                {s.status === "failed" && (
                  <TriangleAlert className="absolute -right-1 -bottom-1 size-3.5 fill-background-card text-text-error" />
                )}
              </div>
              <div className="min-w-0 flex-1">
                <p className="gs-text-body-md-medium truncate text-text-default">{s.name}</p>
                {s.description && <p className="gs-text-body-sm-regular truncate text-text-subdued">{s.description}</p>}
                <div className="mt-1.5 flex flex-wrap gap-1.5">
                  {s.tools.slice(0, 3).map((tl) => (
                    <span key={tl.name} className="gs-text-body-sm-regular rounded-[4px] bg-transparent-tertiary px-1.5 py-0.5 font-mono text-[11px] text-text-subdued">
                      {tl.name}
                    </span>
                  ))}
                  {s.tools.length > 3 && (
                    <span className="gs-text-body-sm-regular text-text-disabled">{t("+{n} more", { n: s.tools.length - 3 })}</span>
                  )}
                </div>
              </div>
              <span onClick={(e) => e.stopPropagation()}>
                <Toggle on={s.enabled} onChange={(v) => run(invoke("mcp_set_enabled", { name: s.name, enabled: v }))} label={s.name} />
              </span>
            </div>
          ))}
        </div>
      )}
      {current && <ServerDialog server={current} onChanged={load} onClose={() => setOpen(null)} />}
      {wizard && (
        <NewServerWizard
          onClose={() => {
            setWizard(false);
            load();
          }}
        />
      )}
    </div>
  );
}

const STATUS_COLOR: Record<McpServerInfo["status"], string> = {
  running: "bg-toggle-on",
  starting: "bg-text-disabled animate-pulse",
  stopped: "bg-text-disabled",
  failed: "bg-text-error",
};

function ServerDialog({ server, onChanged, onClose }: { server: McpServerInfo; onChanged: () => void; onClose: () => void }) {
  const [error, setError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const act = async (p: Promise<unknown>, close = false) => {
    try {
      await p;
      setError(null);
      if (close) onClose();
    } catch (e) {
      setError(String(e));
    }
    onChanged();
  };
  const statusLabel = { running: t("Running"), starting: t("Starting…"), stopped: t("Stopped"), failed: t("Failed to start") }[server.status];
  return (
    <Modal onClose={onClose} width="w-[560px] max-w-[92vw]">
      <div className="flex max-h-[80vh] flex-col">
        <div className="flex items-start justify-between gap-3 px-6 pt-5">
          <div className="min-w-0">
            <h2 className="gs-text-heading-md truncate text-text-default">{server.name}</h2>
            <p className="gs-text-body-sm-regular flex items-center gap-2 text-text-subdued">
              <span className={`inline-block size-2 rounded-full ${STATUS_COLOR[server.status]}`} />
              {statusLabel}
            </p>
          </div>
          <IconButton onClick={onClose} title={t("Close")}>
            <X className="size-4" />
          </IconButton>
        </div>
        <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-6 py-4">
          {server.error && (
            <pre className="gs-text-body-sm-regular whitespace-pre-wrap rounded-[6px] bg-transparent-secondary p-3 font-mono text-[12px] text-text-error select-text">
              {server.error}
            </pre>
          )}
          {server.tools.length > 0 && (
            <div className="flex flex-col gap-1">
              <span className="gs-text-tag text-text-subdued">{t("Tools")}</span>
              {server.tools.map((tl) => (
                <div key={tl.name} className="flex items-center justify-between gap-3 py-1.5">
                  <div className="min-w-0">
                    <p className="gs-text-body-md-regular truncate font-mono text-text-default">{tl.name}</p>
                    {tl.description && <p className="gs-text-body-sm-regular line-clamp-2 text-text-subdued">{tl.description}</p>}
                  </div>
                  <Dropdown
                    value={tl.policy}
                    options={[
                      { value: "auto", label: t("Auto") },
                      { value: "ask", label: t("Ask") },
                      { value: "deny", label: t("Deny") },
                    ]}
                    onChange={(v) => v && act(invoke("mcp_set_tool_policy", { name: server.name, tool: tl.name, policy: v as ToolPolicy }))}
                    className="w-[96px]"
                  />
                </div>
              ))}
            </div>
          )}
          {server.folder && <p className="gs-text-body-sm-regular truncate text-text-disabled select-text">{server.folder}</p>}
          {error && <NoteText tone="error">{t(error)}</NoteText>}
        </div>
        <div className="flex items-center gap-2 border-t border-border-default px-6 py-4">
          <OutlineButton
            onClick={() => (confirmDelete ? act(invoke("mcp_delete", { name: server.name }), true) : setConfirmDelete(true))}
          >
            <Trash2 className={`size-3.5 ${confirmDelete ? "text-text-error" : ""}`} />
            {confirmDelete ? t("Remove from OpenGlaido?") : t("Delete")}
          </OutlineButton>
          <div className="flex-1" />
          {server.folder && (
            <OutlineButton onClick={() => act(invoke("mcp_reveal_folder", { path: server.folder }))}>
              <FolderOpen className="size-3.5" />
              {t("Open folder")}
            </OutlineButton>
          )}
          <OutlineButton onClick={() => act(invoke("mcp_refresh", { name: server.name }))}>
            <RefreshCw className="size-3.5" />
            {t("Refresh tools")}
          </OutlineButton>
        </div>
      </div>
    </Modal>
  );
}

function NewServerWizard({ onClose }: { onClose: () => void }) {
  const [step, setStep] = useState<1 | 2 | 3>(1);
  const [location, setLocation] = useState("~/OpenGlaido/servers");
  const [language, setLanguage] = useState<"python" | "typescript">("python");
  const [runtime, setRuntime] = useState<RuntimeCheck | null>(null);
  const [name, setName] = useState("");
  const [result, setResult] = useState<NewServerResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const check = () => invoke<RuntimeCheck>("mcp_check_runtime", { language }).then(setRuntime).catch((e) => setError(String(e)));
  useEffect(() => {
    if (step === 2) check();
  }, [step, language]);

  const pickLocation = async () => {
    const dir = await openDialog({ directory: true, multiple: false });
    if (typeof dir === "string") setLocation(dir);
  };
  const create = async () => {
    try {
      setResult(await invoke<NewServerResult>("mcp_new_server", { location, language, name }));
      setStep(3);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <Modal onClose={onClose} width="w-[560px] max-w-[92vw]">
      <div className="flex flex-col gap-4 p-6">
        <div className="flex items-center justify-between">
          <h2 className="gs-text-heading-md text-text-default">{t("New server")}</h2>
          <IconButton onClick={onClose} title={t("Close")}>
            <X className="size-4" />
          </IconButton>
        </div>

        {step === 1 && (
          <>
            <span className="gs-text-tag text-text-subdued">{t("Location")}</span>
            <div className="flex gap-2">
              <TextInput value={location} onChange={setLocation} className="min-w-0 flex-1" />
              <OutlineButton onClick={pickLocation}>{t("Choose…")}</OutlineButton>
            </div>
            <span className="gs-text-tag text-text-subdued">{t("Language")}</span>
            <div className="grid grid-cols-2 gap-2">
              {(
                [
                  ["python", "Python", "uv + FastMCP"],
                  ["typescript", "TypeScript", "Bun + MCP SDK"],
                ] as const
              ).map(([id, title, sub]) => (
                <button
                  key={id}
                  type="button"
                  onClick={() => setLanguage(id)}
                  className={`flex flex-col rounded-[6px] border px-3 py-2.5 text-left ${
                    language === id ? "border-border-accent bg-transparent-secondary" : "border-border-default hover:bg-transparent-secondary"
                  }`}
                >
                  <span className="gs-text-body-md-medium text-text-default">{title}</span>
                  <span className="gs-text-body-sm-regular text-text-subdued">{sub}</span>
                </button>
              ))}
            </div>
            <div className="flex justify-end">
              <LimeButton onClick={() => setStep(2)}>{t("Next")}</LimeButton>
            </div>
          </>
        )}

        {step === 2 && (
          <>
            {runtime && !runtime.found ? (
              <div className="flex flex-col gap-2">
                <NoteText tone="warning">
                  {t("OpenGlaido starts the server itself, so it needs {runtime}. Install it with:", { runtime: language === "python" ? "uv" : "bun" })}
                </NoteText>
                <pre className="rounded-[6px] bg-transparent-secondary p-3 font-mono text-[12px] text-text-default select-text">{runtime.install_command}</pre>
                <div>
                  <OutlineButton onClick={check}>{t("Check again")}</OutlineButton>
                </div>
              </div>
            ) : (
              runtime && (
                <p className="gs-text-body-sm-regular flex items-center gap-2 text-text-subdued">
                  <Check className="size-4 text-text-accent" />
                  {runtime.path}
                </p>
              )
            )}
            <span className="gs-text-tag text-text-subdued">{t("Name")}</span>
            <TextInput value={name} onChange={setName} placeholder={t("e.g. Notion")} className="w-full" autoFocus />
            <p className="gs-text-body-sm-regular text-text-disabled">{t("The folder is created when you continue.")}</p>
            {error && <NoteText tone="error">{t(error)}</NoteText>}
            <div className="flex justify-between">
              <OutlineButton onClick={() => setStep(1)}>{t("Back")}</OutlineButton>
              <LimeButton onClick={create} disabled={!name.trim() || !runtime?.found}>
                {t("Create it")}
              </LimeButton>
            </div>
          </>
        )}

        {step === 3 && result && (
          <>
            <p className="gs-text-body-md-regular text-text-default">
              {t("The server is ready and empty. Paste this into Claude Code, Codex or another coding agent to add the tools:")}
            </p>
            <pre className="max-h-[200px] overflow-y-auto whitespace-pre-wrap rounded-[6px] bg-transparent-secondary p-3 font-mono text-[12px] text-text-default select-text">
              {result.prompt}
            </pre>
            <div className="flex justify-between gap-2">
              <OutlineButton onClick={() => invoke("mcp_reveal_folder", { path: result.folder }).catch(console.error)}>
                <FolderOpen className="size-3.5" />
                {t("Reveal folder")}
              </OutlineButton>
              <div className="flex gap-2">
                <OutlineButton
                  onClick={() => {
                    navigator.clipboard.writeText(result.prompt);
                    setCopied(true);
                  }}
                >
                  {copied ? <Check className="size-3.5 text-text-accent" /> : <Copy className="size-3.5" />}
                  {copied ? t("Copied") : t("Copy prompt")}
                </OutlineButton>
                <LimeButton onClick={onClose}>{t("Done")}</LimeButton>
              </div>
            </div>
          </>
        )}
      </div>
    </Modal>
  );
}
