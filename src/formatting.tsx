import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AppWindow, Globe, Plus, Search, Trash2, X } from "lucide-react";
import { BundleIcon } from "./home";
import { t } from "./i18n";
import type { AppInfo, AppRef, FormattingRule, FormattingStyle, SaveConfig, TranscriptionConfig } from "./types";
import { HeaderCard, IconButton, LimeButton, Modal, NoteText, OutlineButton, TextInput, Toggle } from "./ui";

const PROMPT_MAX = 500;

const STYLES: { id: FormattingStyle; title: string; desc: string }[] = [
  { id: "standard", title: "Standard", desc: "Full capitals and punctuation" },
  { id: "casual", title: "Casual", desc: "Capitals stay, fewer full stops" },
  { id: "lowercase", title: "Lowercase", desc: "No capitals, minimal punctuation" },
];

/** "https://www.github.com/x" → "github.com" (same normalisation as the backend). */
export function hostOf(input: string): string {
  let rest = input.trim();
  const scheme = rest.indexOf("://");
  if (scheme >= 0) rest = rest.slice(scheme + 3);
  rest = rest.split(/[/?#]/)[0];
  rest = rest.split("@").pop() ?? "";
  rest = rest.split(":")[0].toLowerCase();
  return rest.startsWith("www.") ? rest.slice(4) : rest;
}

const SITE_NAMES: Record<string, string> = {
  "mail.google.com": "Gmail",
  "outlook.live.com": "Outlook",
  "outlook.office.com": "Outlook",
};

function placeLabels(rule: FormattingRule): string[] {
  return [...rule.apps.map((a) => a.name), ...rule.websites.map((w) => SITE_NAMES[hostOf(w)] ?? hostOf(w))];
}

function Chips({ labels }: { labels: string[] }) {
  const shown = labels.slice(0, 3);
  return (
    <span className="flex shrink-0 items-center gap-1.5">
      {shown.map((l, i) => (
        <span key={l + i} className="gs-text-body-sm-regular rounded-[4px] bg-transparent-tertiary px-2 py-1 text-text-subdued">
          {l}
        </span>
      ))}
      {labels.length > shown.length && (
        <span className="gs-text-body-sm-regular px-1 text-text-disabled">+{labels.length - shown.length}</span>
      )}
    </span>
  );
}

type Editing = { kind: "all" } | { kind: "email" } | { kind: "custom"; rule: FormattingRule | null };

export function FormattingPage({
  config,
  onSave,
  addRuleOpen,
  onAddRuleOpenChange,
}: {
  config: TranscriptionConfig;
  onSave: SaveConfig;
  /** Opened from ⌘K "Add formatting rule". */
  addRuleOpen: boolean;
  onAddRuleOpenChange: (open: boolean) => void;
}) {
  const [editing, setEditing] = useState<Editing | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (addRuleOpen) {
      setEditing({ kind: "custom", rule: null });
      onAddRuleOpenChange(false);
    }
  }, [addRuleOpen]);

  const save = async (patch: Partial<TranscriptionConfig>) => {
    const err = await onSave(patch);
    setError(err);
    return err;
  };
  const toggleRule = (rule: FormattingRule, enabled: boolean) =>
    rule.id === "email"
      ? save({ email_rule: { ...rule, enabled } })
      : save({ custom_rules: config.custom_rules.map((r) => (r.id === rule.id ? { ...r, enabled } : r)) });
  const deleteRule = (id: string) => save({ custom_rules: config.custom_rules.filter((r) => r.id !== id) });

  const styleName = (s: FormattingStyle) => t(STYLES.find((x) => x.id === s)?.title ?? "Standard");
  const allAppsSummary = config.raw_text ? t("Raw text") : styleName(config.style);

  return (
    <>
      <HeaderCard
        title={t("Make dictations sound like you")}
        description={t("One style for all apps, a built-in rule for email, and custom rules for the places that need their own.")}
      />
      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto rounded-[8px] bg-background-card p-8">
        <div className="flex min-h-7 shrink-0 items-center justify-between gap-1.5">
          <span className="gs-text-tag text-text-default">{t("Formatting")}</span>
          <OutlineButton onClick={() => setEditing({ kind: "custom", rule: null })}>
            <Plus className="size-3" />
            {t("Add rule")}
          </OutlineButton>
        </div>

        <div className="mt-5 flex flex-col">
          {/* All apps: the floor every rule stands on (no switch, no places, no delete). */}
          <button
            type="button"
            onClick={() => setEditing({ kind: "all" })}
            className="flex min-h-[60px] items-center justify-between gap-4 rounded-[4px] px-3 py-2.5 text-left transition-colors hover:bg-transparent-secondary"
          >
            <div className="flex min-w-0 flex-col">
              <span className="gs-text-body-md-regular text-text-default">{t("All apps")}</span>
              <span className="gs-text-body-md-regular truncate text-text-subdued">
                {t("The style every dictation gets, in every app.")}
              </span>
            </div>
            <span className="gs-text-body-sm-regular shrink-0 text-text-disabled">{allAppsSummary}</span>
          </button>

          <RuleRow
            rule={config.email_rule}
            title={t("Email")}
            description={t("A built-in rule for dictations in your email apps and webmail.")}
            onOpen={() => setEditing({ kind: "email" })}
            onToggle={(v) => toggleRule(config.email_rule, v)}
          />

          <div className="mx-3 mt-6 h-px bg-border-default" />
          <span className="gs-text-tag mt-6 px-3 text-text-subdued">{t("Custom rules")}</span>
          {config.custom_rules.length === 0 ? (
            <p className="gs-text-body-md-regular mt-4 px-3 text-text-default">
              {t("No custom rules yet. Add one for the apps that need their own style.")}
            </p>
          ) : (
            <div className="mt-2 flex flex-col">
              {config.custom_rules.map((rule) => (
                <RuleRow
                  key={rule.id}
                  rule={rule}
                  title={rule.name}
                  description={rule.raw_text ? t("Raw text") : styleName(rule.style)}
                  onOpen={() => setEditing({ kind: "custom", rule })}
                  onToggle={(v) => toggleRule(rule, v)}
                  onDelete={() => deleteRule(rule.id)}
                />
              ))}
            </div>
          )}
          {error && <p className="gs-text-body-sm-regular px-3 pt-3 text-text-error select-text">{t(error)}</p>}
        </div>
      </div>

      {editing && <RuleEditor editing={editing} config={config} onSave={save} onClose={() => setEditing(null)} />}
    </>
  );
}

function RuleRow({
  rule,
  title,
  description,
  onOpen,
  onToggle,
  onDelete,
}: {
  rule: FormattingRule;
  title: string;
  description: string;
  onOpen: () => void;
  onToggle: (v: boolean) => void;
  onDelete?: () => void;
}) {
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onOpen}
      onKeyDown={(e) => e.key === "Enter" && onOpen()}
      className="group flex min-h-[60px] cursor-default items-center justify-between gap-4 rounded-[4px] px-3 py-2.5 transition-colors hover:bg-transparent-secondary"
    >
      <div className="flex min-w-0 flex-col">
        <span className="gs-text-body-md-regular truncate text-text-default">{title}</span>
        <span className="gs-text-body-md-regular truncate text-text-subdued">{description}</span>
      </div>
      <div className="flex shrink-0 items-center gap-3" onClick={(e) => e.stopPropagation()}>
        <Chips labels={placeLabels(rule)} />
        {onDelete && (
          <span className="opacity-0 transition-opacity group-hover:opacity-100">
            <IconButton onClick={onDelete} title={t("Delete rule")}>
              <Trash2 className="size-4" />
            </IconButton>
          </span>
        )}
        <Toggle on={rule.enabled} onChange={onToggle} label={title} />
      </div>
    </div>
  );
}

// ------------------------------------------------------------------
// Rule editor: All apps / Email / custom rule
// ------------------------------------------------------------------
function RuleEditor({
  editing,
  config,
  onSave,
  onClose,
}: {
  editing: Editing;
  config: TranscriptionConfig;
  onSave: (patch: Partial<TranscriptionConfig>) => Promise<string | null>;
  onClose: () => void;
}) {
  const source: FormattingRule =
    editing.kind === "all"
      ? { id: "all", name: "", enabled: true, apps: [], websites: [], style: config.style, raw_text: config.raw_text, custom_prompt: config.custom_prompt }
      : editing.kind === "email"
        ? config.email_rule
        : (editing.rule ?? { id: crypto.randomUUID(), name: "", enabled: true, apps: [], websites: [], style: "standard", raw_text: false, custom_prompt: "" });
  const [draft, setDraft] = useState<FormattingRule>(source);
  const [site, setSite] = useState("");
  const [pickerOpen, setPickerOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const isNew = editing.kind === "custom" && !editing.rule;
  const hasPlaces = editing.kind !== "all";
  const tooLong = draft.custom_prompt.length > PROMPT_MAX;

  // Apps and websites other rules already own (one rule per place).
  const owners = useMemo(() => {
    const map = new Map<string, string>();
    for (const r of [config.email_rule, ...config.custom_rules]) {
      if (r.id === draft.id) continue;
      for (const a of r.apps) map.set(`app:${a.bundle_id}`, r.name);
      for (const w of r.websites) map.set(`site:${hostOf(w)}`, r.name);
    }
    return map;
  }, [config, draft.id]);

  const set = (patch: Partial<FormattingRule>) => setDraft((d) => ({ ...d, ...patch }));
  const addSite = () => {
    const host = hostOf(site);
    if (!host) return;
    const owner = owners.get(`site:${host}`);
    if (owner) {
      setError(t("In your “{rule}” rule. Edit it there.", { rule: owner }));
      return;
    }
    if (!draft.websites.some((w) => hostOf(w) === host)) set({ websites: [...draft.websites, host] });
    setSite("");
    setError(null);
  };

  const save = async () => {
    if (editing.kind === "custom" && !draft.name.trim()) {
      setError(t("Give the rule a name"));
      return;
    }
    const rule = { ...draft, name: draft.name.trim() };
    const patch: Partial<TranscriptionConfig> =
      editing.kind === "all"
        ? { style: rule.style, raw_text: rule.raw_text, custom_prompt: rule.custom_prompt }
        : editing.kind === "email"
          ? { email_rule: rule }
          : {
              custom_rules: isNew
                ? [...config.custom_rules, rule]
                : config.custom_rules.map((r) => (r.id === rule.id ? rule : r)),
            };
    const err = await onSave(patch);
    if (err) setError(err);
    else onClose();
  };

  const title = editing.kind === "all" ? t("All apps") : editing.kind === "email" ? t("Email") : isNew ? t("New rule") : draft.name;

  return (
    <Modal onClose={onClose} width="w-[560px] max-w-[92vw]">
      <div className="flex max-h-[82vh] flex-col">
        <div className="flex items-center justify-between px-6 pt-5">
          <h2 className="gs-text-heading-md truncate text-text-default">{title}</h2>
          <IconButton onClick={onClose} title={t("Close")}>
            <X className="size-4" />
          </IconButton>
        </div>

        <div className="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-6 pt-5 pb-2">
          {editing.kind === "custom" && (
            <Field label={t("Name")}>
              <TextInput value={draft.name} onChange={(v) => set({ name: v })} placeholder={t("e.g. Chat apps")} className="w-full" autoFocus={isNew} />
            </Field>
          )}

          {hasPlaces && (
            <Field label={t("Where it applies")}>
              <div className="flex flex-wrap gap-1.5">
                {draft.apps.map((a) => (
                  <PlaceChip key={a.bundle_id} icon={<BundleIcon bundleId={a.bundle_id} size={16} fallback={<AppWindow className="size-4 text-text-subdued" />} />} label={a.name} onRemove={() => set({ apps: draft.apps.filter((x) => x.bundle_id !== a.bundle_id) })} />
                ))}
                {draft.websites.map((w) => (
                  <PlaceChip key={w} icon={<Globe className="size-4 text-text-subdued" />} label={hostOf(w)} onRemove={() => set({ websites: draft.websites.filter((x) => x !== w) })} />
                ))}
                {draft.apps.length + draft.websites.length === 0 && (
                  <span className="gs-text-body-sm-regular text-text-disabled">{t("No apps or websites yet.")}</span>
                )}
              </div>
              <div className="mt-2.5 flex items-center gap-2">
                <OutlineButton onClick={() => setPickerOpen((v) => !v)}>
                  <Plus className="size-3" />
                  {t("Add app")}
                </OutlineButton>
                <TextInput value={site} onChange={setSite} onCommit={() => site && addSite()} placeholder={t("Add a website, e.g. github.com")} className="min-w-0 flex-1" />
              </div>
              {pickerOpen && (
                <AppPicker
                  selected={draft.apps}
                  owners={owners}
                  onPick={(app) => {
                    set({ apps: [...draft.apps, { bundle_id: app.bundle_id, name: app.name }] });
                  }}
                  onRemove={(bundleId) => set({ apps: draft.apps.filter((a) => a.bundle_id !== bundleId) })}
                />
              )}
              <p className="gs-text-body-sm-regular mt-2 text-text-disabled">
                {t("Browsers aren't in the app list: add the website itself.")}
              </p>
            </Field>
          )}

          <Field label={t("Style")}>
            <div className="grid grid-cols-3 gap-2">
              {STYLES.map((s) => {
                const active = draft.style === s.id;
                return (
                  <button
                    key={s.id}
                    type="button"
                    onClick={() => set({ style: s.id })}
                    className={`flex flex-col gap-1 rounded-[6px] border px-3 py-2.5 text-left transition-colors ${
                      active ? "border-border-accent bg-transparent-secondary" : "border-border-default hover:bg-transparent-secondary"
                    }`}
                  >
                    <span className="gs-text-body-md-medium text-text-default">{t(s.title)}</span>
                    <span className="gs-text-body-sm-regular text-text-subdued">{t(s.desc)}</span>
                  </button>
                );
              })}
            </div>
          </Field>

          <div className="flex items-center justify-between gap-4">
            <div className="flex min-w-0 flex-col">
              <span className="gs-text-body-md-regular text-text-default">{t("Raw text")}</span>
              <span className="gs-text-body-sm-regular text-text-subdued">
                {t("Exactly what you said, with no AI cleanup. Your style still applies.")}
              </span>
            </div>
            <Toggle on={draft.raw_text} onChange={(v) => set({ raw_text: v })} label={t("Raw text")} />
          </div>

          {!draft.raw_text && (
            <Field label={t("Custom prompt")}>
              <textarea
                value={draft.custom_prompt}
                onChange={(e) => set({ custom_prompt: e.target.value })}
                rows={3}
                placeholder={t("e.g. Use British English spelling.")}
                className="gs-text-body-md-regular w-full resize-none rounded-[4px] border border-border-default bg-background-input px-2.5 py-2 text-text-default placeholder:text-text-disabled focus:outline-none"
              />
              <div className={`gs-text-body-sm-regular mt-1 text-right tabular-nums ${tooLong ? "text-text-error" : "text-text-disabled"}`}>
                {draft.custom_prompt.length} / {PROMPT_MAX}
              </div>
              {editing.kind !== "all" && (
                <p className="gs-text-body-sm-regular text-text-disabled">{t("Added on top of the All apps prompt.")}</p>
              )}
            </Field>
          )}
          {error && <NoteText tone="error">{t(error)}</NoteText>}
        </div>

        <div className="flex items-center justify-end gap-2 border-t border-border-default px-6 py-4">
          <OutlineButton onClick={onClose}>{t("Cancel")}</OutlineButton>
          <LimeButton onClick={save} disabled={tooLong}>
            {t("Save")}
          </LimeButton>
        </div>
      </div>
    </Modal>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col">
      <span className="gs-text-tag mb-2 text-text-subdued">{label}</span>
      {children}
    </div>
  );
}

function PlaceChip({ icon, label, onRemove }: { icon: React.ReactNode; label: string; onRemove: () => void }) {
  return (
    <span className="flex items-center gap-1.5 rounded-[4px] bg-transparent-tertiary py-1 pr-1 pl-2">
      {icon}
      <span className="gs-text-body-sm-regular text-text-default">{label}</span>
      <button type="button" onClick={onRemove} title={t("Remove")} className="text-text-disabled transition-colors hover:text-text-default">
        <X className="size-3.5" />
      </button>
    </span>
  );
}

/** "Most used" (recent_apps) then "All applications" (list_apps); owned apps are greyed. */
function AppPicker({
  selected,
  owners,
  onPick,
  onRemove,
}: {
  selected: AppRef[];
  owners: Map<string, string>;
  onPick: (app: AppInfo) => void;
  onRemove: (bundleId: string) => void;
}) {
  const [recent, setRecent] = useState<AppInfo[]>([]);
  const [all, setAll] = useState<AppInfo[] | null>(null);
  const [query, setQuery] = useState("");
  useEffect(() => {
    invoke<AppInfo[]>("recent_apps").then(setRecent).catch(console.error);
    invoke<AppInfo[]>("list_apps").then(setAll).catch(() => setAll([]));
  }, []);
  const q = query.trim().toLowerCase();
  const match = (a: AppInfo) => !q || a.name.toLowerCase().includes(q);
  const recentIds = new Set(recent.map((a) => a.bundle_id));
  const groups = [
    { label: t("Most used"), apps: recent.filter(match) },
    { label: t("All applications"), apps: (all ?? []).filter((a) => !recentIds.has(a.bundle_id) && match(a)) },
  ].filter((g) => g.apps.length > 0);

  return (
    <div className="mt-2.5 flex max-h-[240px] flex-col overflow-hidden rounded-[6px] border border-border-default bg-background-input">
      <div className="flex items-center gap-2 border-b border-border-default px-2.5 py-1.5">
        <Search className="size-3.5 text-text-subdued" />
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={t("Search apps...")}
          autoFocus
          className="gs-text-body-md-regular min-w-0 flex-1 bg-transparent text-text-default placeholder:text-text-disabled focus:outline-none"
        />
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto py-1">
        {all === null && <p className="gs-text-body-sm-regular px-3 py-2 text-text-disabled">{t("Loading apps…")}</p>}
        {groups.map((g) => (
          <div key={g.label}>
            <div className="gs-text-tag px-3 pt-2 pb-1 text-text-disabled">{g.label}</div>
            {g.apps.map((app) => {
              const owner = owners.get(`app:${app.bundle_id}`);
              const checked = selected.some((s) => s.bundle_id === app.bundle_id);
              return (
                <button
                  key={app.bundle_id}
                  type="button"
                  disabled={!!owner}
                  title={owner ? t("In your “{rule}” rule. Edit it there.", { rule: owner }) : undefined}
                  onClick={() => (checked ? onRemove(app.bundle_id) : onPick(app))}
                  className="flex w-full items-center gap-2.5 px-3 py-1.5 text-left transition-colors enabled:hover:bg-transparent-secondary disabled:opacity-40"
                >
                  <input type="checkbox" readOnly checked={checked || !!owner} disabled={!!owner} className="accent-[var(--color-toggle-on)]" />
                  <BundleIcon bundleId={app.bundle_id} size={20} fallback={<AppWindow className="size-5 text-text-subdued" />} />
                  <span className="gs-text-body-md-regular truncate text-text-default">{app.name}</span>
                </button>
              );
            })}
          </div>
        ))}
      </div>
    </div>
  );
}
