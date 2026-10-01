// Command window (/#command, label "command"): never takes focus; Enter/Esc/⌘C arrive as hotkeys.
import { Fragment, useEffect, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Check, CircleAlert, ClipboardList, Copy, LoaderCircle, Sparkles, X } from "lucide-react";
import { resolveLocale, setLocale, t } from "./i18n";
import type { CommandState, TranscriptionConfig } from "./types";
import { IS_MAC, Keycaps, bindingLabels } from "./ui";

// ------------------------------------------------------------------
// Markdown → React elements. Never HTML: the text comes from a model and this webview has IPC.
// ------------------------------------------------------------------
function inline(text: string, key: string): ReactNode[] {
  const out: ReactNode[] = [];
  const re = /(`[^`]+`)|(\*\*[^*]+\*\*)|(\*[^*\s][^*]*\*)|(\[[^\]]+\]\((https?:\/\/[^)\s]+)\))/g;
  let last = 0;
  let m: RegExpExecArray | null;
  let i = 0;
  while ((m = re.exec(text))) {
    if (m.index > last) out.push(text.slice(last, m.index));
    const k = `${key}-${i++}`;
    if (m[1]) out.push(<code key={k} className="rounded-[3px] bg-transparent-tertiary px-1 font-mono text-[12px]">{m[1].slice(1, -1)}</code>);
    else if (m[2]) out.push(<strong key={k} className="font-semibold">{m[2].slice(2, -2)}</strong>);
    else if (m[3]) out.push(<em key={k}>{m[3].slice(1, -1)}</em>);
    else if (m[4]) {
      const label = m[4].slice(1, m[4].indexOf("]("));
      const url = m[5];
      out.push(
        <button key={k} type="button" onClick={() => openUrl(url).catch(console.error)} className="text-text-accent hover:underline">
          {label}
        </button>,
      );
    }
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

export function Markdown({ text }: { text: string }) {
  const blocks: ReactNode[] = [];
  const lines = text.replace(/\r\n?|\u2028|\u2029/g, "\n").split("\n");
  let i = 0;
  let n = 0; // unique block keys
  while (i < lines.length) {
    const line = lines[i];
    if (line.startsWith("```")) {
      const code: string[] = [];
      i++;
      while (i < lines.length && !lines[i].startsWith("```")) code.push(lines[i++]);
      i++;
      blocks.push(
        <pre key={`b${n++}`} className="overflow-x-auto rounded-[6px] bg-transparent-tertiary p-2.5 font-mono text-[12px] leading-5">
          {code.join("\n")}
        </pre>,
      );
      continue;
    }
    const heading = /^(#{1,3})\s+(.*)$/.exec(line);
    if (heading) {
      blocks.push(<p key={`b${n++}`} className="gs-text-body-md-medium text-text-default">{inline(heading[2], `h${i}`)}</p>);
      i++;
      continue;
    }
    if (/^\s*([-*•]|\d+[.)])\s+/.test(line)) {
      const items: string[] = [];
      const ordered = /^\s*\d/.test(line);
      while (i < lines.length && /^\s*([-*•]|\d+[.)])\s+/.test(lines[i])) items.push(lines[i++].replace(/^\s*([-*•]|\d+[.)])\s+/, ""));
      const List = ordered ? "ol" : "ul";
      blocks.push(
        <List key={`b${n++}`} className={`flex flex-col gap-1 pl-5 ${ordered ? "list-decimal" : "list-disc"}`}>
          {items.map((it, j) => <li key={j}>{inline(it, `l${i}-${j}`)}</li>)}
        </List>,
      );
      continue;
    }
    if (!line.trim()) {
      i++;
      continue;
    }
    const para: string[] = [];
    while (i < lines.length && lines[i].trim() && !/^(```|#{1,3}\s|\s*([-*•]|\d+[.)])\s)/.test(lines[i])) para.push(lines[i++]);
    // Always make progress (a line no rule above consumed becomes a paragraph of its own).
    if (para.length === 0) para.push(lines[i++]);
    blocks.push(
      <p key={`b${n++}`}>
        {para.map((p, j) => (
          <Fragment key={j}>
            {j > 0 && <br />}
            {inline(p, `p${i}-${j}`)}
          </Fragment>
        ))}
      </p>,
    );
  }
  return <div className="gs-text-body-md-regular flex flex-col gap-2.5 text-text-default select-text">{blocks}</div>;
}

// ------------------------------------------------------------------
// Window
// ------------------------------------------------------------------
/** Progress label in the app language (MCP tools keep "server: tool"). */
function toolLabel(tool: CommandState["tools"][number]): string {
  const query = tool.detail;
  switch (tool.name) {
    case "web_search":
      return t("Searching the web for “{query}”", { query });
    case "read_page":
      return t("Reading the page");
    case "youtube":
      return t("Reading the video transcript");
    case "deep_research":
      return t("Starting deep research");
    case "math_dates":
      return t("Working it out");
    case "files_apps":
      return tool.action === "find" ? t("Looking for “{query}”", { query }) : tool.action === "read" ? t("Reading the file") : t("Opening");
    case "history_search":
      return t("Searching your history for “{query}”", { query });
    case "docs_search":
      return t("Reading the documentation");
    default:
      return tool.label;
  }
}
const WORKING: Record<string, string> = {
  listening: "Listening…",
  transcribing: "Transcribing…",
  thinking: "Thinking…",
  tool: "Working…",
};

export function CommandWindow() {
  const [state, setState] = useState<CommandState | null>(null);
  const [copied, setCopied] = useState(false);
  const [, setLocaleTick] = useState(0);

  useEffect(() => {
    const applyConfig = (c: TranscriptionConfig) => {
      setLocale(resolveLocale(c.app_language));
      setLocaleTick((n) => n + 1);
    };
    invoke<TranscriptionConfig>("get_config").then(applyConfig).catch(console.error);
    const unlistenConfig = listen<TranscriptionConfig>("config-changed", (e) => applyConfig(e.payload));
    const unlistenState = listen<CommandState>("command-state", (e) => setState(e.payload));
    return () => {
      unlistenConfig.then((f) => f());
      unlistenState.then((f) => f());
    };
  }, []);

  const copy = () => {
    invoke("command_copy")
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1500);
      })
      .catch(console.error);
  };

  const phase = state?.phase ?? "listening";
  const working = phase in WORKING;
  const ready = !!state?.answer_ready;

  return (
    <div className="flex h-screen w-screen items-end justify-center bg-transparent p-1 select-none">
      <div className="flex max-h-full w-full flex-col overflow-hidden rounded-[12px] bg-[#0d0d0d] text-[#ffffff] shadow-[inset_0_0_0_1px_rgba(255,255,255,0.1),0_8px_24px_rgba(0,0,0,0.35)]">
        {/* Header: drag region with the instruction */}
        <div data-tauri-drag-region className="flex shrink-0 items-start gap-2.5 px-4 pt-3.5 pb-2.5">
          <Sparkles className="pointer-events-none mt-0.5 size-4 shrink-0 text-text-accent" />
          <p data-tauri-drag-region className="gs-text-body-md-regular line-clamp-2 min-w-0 flex-1 text-text-subdued">
            {state?.instruction || t(working ? WORKING[phase] : "Command")}
          </p>
          {state?.has_selection && (
            <span title={t("Your selection went along")} className="mt-0.5 shrink-0 text-text-subdued">
              <ClipboardList className="size-4" />
            </span>
          )}
          <button type="button" onClick={() => invoke("command_close").catch(console.error)} title={t("Close")} className="shrink-0 text-text-disabled transition-colors hover:text-text-default">
            <X className="size-4" />
          </button>
        </div>
        <div className="mx-4 h-px shrink-0 bg-border-default" />

        {/* Body */}
        <div className="flex min-h-[64px] flex-1 flex-col gap-3 overflow-y-auto px-4 py-3">
          {state?.tools.map((tool, i) => (
            <div key={`${tool.name}${i}`} className="gs-text-body-sm-regular flex items-center gap-2 text-text-subdued">
              {tool.status === "running" ? (
                <LoaderCircle className="size-3.5 animate-spin text-text-accent" />
              ) : tool.status === "done" ? (
                <Check className="size-3.5 text-text-accent" />
              ) : (
                <CircleAlert className="size-3.5 text-text-error" />
              )}
              <span className="truncate">{toolLabel(tool)}</span>
            </div>
          ))}
          {state?.approval && <ApprovalCard approval={state.approval} />}
          {state?.text ? (
            <Markdown text={state.text === "What can I help you with?" ? t(state.text) : state.text} />
          ) : (
            working && <p className="gs-text-body-md-regular gs-waveform-processing text-text-subdued">{t(WORKING[phase])}</p>
          )}
          {state?.error && (
            <p className="gs-text-body-md-regular flex items-center gap-2 text-text-error">
              <CircleAlert className="size-4 shrink-0" />
              {t(state.error)}
            </p>
          )}
        </div>

        {/* Footer */}
        <div className="flex shrink-0 items-center gap-4 border-t border-border-default px-4 py-2.5">
          {state?.refine_keys && (
            <span className="gs-text-body-sm-regular flex items-center gap-1.5 text-text-subdued">
              <Keycaps labels={bindingLabels(state.refine_keys)} />
              {t("Refine")}
            </span>
          )}
          <span className={`gs-text-body-sm-regular flex items-center gap-1.5 ${ready || state?.approval ? "text-text-subdued" : "text-text-disabled"}`}>
            <Keycaps labels={["ENTER"]} />
            {state?.approval ? t("Allow once") : t("Paste content")}
          </span>
          <div className="flex-1" />
          <button
            type="button"
            onClick={copy}
            disabled={!ready}
            title={IS_MAC ? t("Copy (⌘C)") : t("Copy (Ctrl+C)")}
            className="gs-text-body-sm-regular flex items-center gap-1.5 rounded-[4px] px-2 py-1 text-text-subdued transition-colors enabled:hover:bg-transparent-secondary enabled:hover:text-text-default disabled:opacity-40"
          >
            {copied ? <Check className="size-3.5 text-text-accent" /> : <Copy className="size-3.5" />}
            {copied ? t("Copied") : t("Copy")}
          </button>
        </div>
      </div>
    </div>
  );
}

export function ApprovalCard({ approval }: { approval: NonNullable<CommandState["approval"]> }) {
  const [left, setLeft] = useState(approval.expires_in_s);
  useEffect(() => {
    setLeft(approval.expires_in_s);
    const timer = window.setInterval(() => setLeft((s) => Math.max(0, s - 1)), 1000);
    return () => window.clearInterval(timer);
  }, [approval.request_id]);
  const decide = (decision: "once" | "always" | "deny") =>
    invoke("command_approve", { requestId: approval.request_id, decision }).catch(console.error);
  return (
    <div className="flex flex-col gap-2.5 rounded-[8px] border border-border-default bg-transparent-secondary p-3">
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0">
          <p className="gs-text-body-md-medium truncate font-mono text-text-default">{approval.tool}</p>
          <p className="gs-text-body-sm-regular truncate text-text-subdued">{approval.server}</p>
        </div>
        <span className="gs-text-body-sm-regular shrink-0 tabular-nums text-text-disabled">{left}s</span>
      </div>
      {approval.summary && approval.summary !== "{}" && (
        <pre className="gs-text-body-sm-regular max-h-28 overflow-auto whitespace-pre-wrap break-all font-mono text-text-subdued">{approval.summary}</pre>
      )}
      <div className="flex justify-end gap-2">
        <button type="button" onClick={() => decide("deny")} className="gs-text-body-sm-regular rounded-[4px] border border-border-default px-2.5 py-1 text-text-subdued hover:text-text-default">
          {t("Deny")}
        </button>
        <button type="button" disabled={!approval.allow_always} onClick={() => decide("always")} className="gs-text-body-sm-regular rounded-[4px] border border-border-default px-2.5 py-1 text-text-subdued hover:text-text-default">
          {t("Always allow")}
        </button>
        <button type="button" onClick={() => decide("once")} className="gs-text-body-sm-medium rounded-[4px] bg-button-primary px-2.5 py-1 text-text-dark">
          {t("Allow once")}
        </button>
      </div>
    </div>
  );
}
