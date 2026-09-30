import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown, Plus, Search, TriangleAlert, createLucideIcon } from "lucide-react";
import type { IconType } from "./types";
import { t } from "./i18n";

export const IS_MAC = /Mac/i.test(navigator.userAgent);

// Lucide-based nav glyphs closer to Glaido's. The outline comes first so the inner strokes paint
// on top of it when the active icon is filled.
export const HouseDoor = createLucideIcon("house-door", [
  [
    "path",
    {
      d: "M3 10a2 2 0 0 1 .709-1.528l7-6a2 2 0 0 1 2.582 0l7 6A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
      key: "outline",
    },
  ],
  ["path", { d: "M14 21v-5a1 1 0 0 0-1-1h-2a1 1 0 0 0-1 1v5", key: "door" }],
]);
export const BookSpine = createLucideIcon("book-spine", [
  [
    "path",
    {
      d: "M20.001 19A2 2 0 0022 17V5a2 2 0 00-1.999-2L16 3.002A5 5 0 0012 5a5 5 0 00-4-2H4a2 2 0 00-2 2v12a2 2 0 001.999 2H8a5 5 0 014 2 5 5 0 014-2z",
      key: "outline",
    },
  ],
  ["path", { d: "M12 5v16", key: "spine" }],
]);
// Angular bolt (lucide's pre-1.0 zap), closer to Glaido's stat icon than the rounded 1.x one.
export const Bolt = createLucideIcon("bolt-angular", [
  ["polygon", { points: "13 2 3 14 12 14 11 22 21 10 12 10 13 2", key: "bolt" }],
]);
export const SnippetIcon = createLucideIcon("snippet", [
  ["rect", { width: "18", height: "18", x: "3", y: "3", rx: "2", key: "outline" }],
  ["path", { d: "M8 10h7", key: "line-1" }],
  ["path", { d: "M8 14h5", key: "line-2" }],
]);
// Commands nav glyph (lucide square-terminal with the outline first, see above).
export const CommandsIcon = createLucideIcon("commands", [
  ["rect", { width: "18", height: "18", x: "3", y: "3", rx: "2", key: "outline" }],
  ["path", { d: "m7 11 2-2-2-2", key: "prompt" }],
  ["path", { d: "M11 13h4", key: "cursor" }],
]);

// ------------------------------------------------------------------
// Small shared primitives
// ------------------------------------------------------------------
export function Toggle({ on, onChange, label }: { on: boolean; onChange: (v: boolean) => void; label?: string }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      onClick={() => onChange(!on)}
      className={`inline-flex h-5 w-10 shrink-0 items-center rounded-full p-[3px] transition-colors ${
        on ? "bg-toggle-on" : "bg-toggle-off"
      }`}
    >
      <span
        className={`block h-3.5 w-5 rounded-full bg-white transition-transform ${on ? "translate-x-[14px]" : ""}`}
      />
    </button>
  );
}

export function OutlineButton({
  onClick,
  children,
}: {
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="gs-text-body-md-medium inline-flex h-7 shrink-0 items-center gap-2.5 whitespace-nowrap rounded-[2px] border border-border-default pr-2.5 pl-3 text-text-subdued transition-colors hover:text-text-default"
    >
      {children}
    </button>
  );
}

export function LimeButton({
  onClick,
  children,
  disabled = false,
}: {
  onClick: () => void;
  children: React.ReactNode;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className="gs-text-body-md-medium inline-flex h-7 shrink-0 items-center gap-2.5 whitespace-nowrap rounded-[2px] bg-button-primary px-2.5 text-text-dark transition-colors hover:bg-text-accent/85 disabled:opacity-40"
    >
      {children}
    </button>
  );
}

export function IconButton({
  onClick,
  title,
  subdued,
  children,
}: {
  onClick: () => void;
  title?: string;
  subdued?: boolean;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      className={`flex size-7 items-center justify-center rounded-[2px] p-1 transition-colors hover:text-text-default ${
        subdued ? "text-text-disabled" : "text-text-subdued"
      }`}
    >
      {children}
    </button>
  );
}

export function SearchField({
  value,
  onChange,
  placeholder,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder: string;
}) {
  return (
    <div className="flex h-[30px] w-[200px] shrink-0 items-center gap-2.5 rounded-[4px] bg-transparent-primary px-[13px]">
      <Search className="size-3.5 shrink-0 text-text-subdued" />
      <input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="gs-text-body-sm-regular w-full min-w-0 bg-transparent text-text-default placeholder:text-text-disabled focus:outline-none"
      />
    </div>
  );
}

// Esc closes only the topmost overlay (modal, palette, nested picker), never the one below it.
const escapeStack: { current: () => void; own: boolean }[] = [];
let escapeListening = false;

/** Registers an overlay's close handler while it is mounted; Esc calls the newest one.
 *  `own`: the component reads Esc itself (hotkey recorder), so nothing below it closes. */
export function useEscape(onClose: () => void, active = true, own = false) {
  const ref = useRef(onClose);
  ref.current = onClose;
  useEffect(() => {
    if (!active) return;
    if (!escapeListening) {
      escapeListening = true;
      window.addEventListener(
        "keydown",
        (e) => {
          const top = escapeStack[escapeStack.length - 1];
          if (e.key !== "Escape" || e.isComposing || e.keyCode === 229 || !top || top.own) return;
          e.preventDefault();
          e.stopImmediatePropagation();
          top.current();
        },
        true,
      );
    }
    const entry = { current: () => ref.current(), own };
    escapeStack.push(entry);
    return () => {
      const i = escapeStack.indexOf(entry);
      if (i >= 0) escapeStack.splice(i, 1);
    };
  }, [active]);
}

/** Enter that isn't committing an IME composition (Japanese/Chinese input). */
export function isPlainEnter(e: React.KeyboardEvent): boolean {
  return e.key === "Enter" && !e.nativeEvent.isComposing && e.keyCode !== 229;
}

export function Modal({
  onClose,
  width,
  children,
}: {
  onClose: () => void;
  width: string;
  children: React.ReactNode;
}) {
  useEscape(onClose);
  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/30"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        className={`${width} rounded-[12px] border border-border-default bg-modal-surface shadow-2xl backdrop-blur-xl`}
      >
        {children}
      </div>
    </div>
  );
}

export function TextInput({
  value,
  onChange,
  onCommit,
  placeholder,
  type = "text",
  className = "",
  autoFocus = false,
}: {
  value: string;
  onChange: (v: string) => void;
  onCommit?: () => void;
  placeholder?: string;
  type?: string;
  className?: string;
  autoFocus?: boolean;
}) {
  return (
    <input
      type={type}
      value={value}
      autoFocus={autoFocus}
      onChange={(e) => onChange(e.target.value)}
      onBlur={onCommit}
      onKeyDown={(e) => {
        if (isPlainEnter(e)) e.currentTarget.blur();
      }}
      placeholder={placeholder}
      className={`gs-text-body-sm-regular h-[28px] rounded-[2px] border border-border-default bg-background-input px-2.5 text-text-default placeholder:text-text-disabled focus:outline-none ${className}`}
    />
  );
}

/** Outlined select (Glaido style) or, with `buttonLabel`, an outlined button that opens the same menu. */
export function Dropdown({
  value,
  options,
  onChange,
  className = "",
  buttonLabel,
}: {
  value: string | null;
  options: { value: string | null; label: string }[];
  onChange: (v: string | null) => void;
  className?: string;
  buttonLabel?: string;
}) {
  // Portalled fixed-position menu: never clipped by the scrolling settings column, and not
  // positioned relative to the modal (its backdrop-filter is a containing block for fixed children).
  const [menu, setMenu] = useState<React.CSSProperties | null>(null);
  const openMenu = (e: React.MouseEvent<HTMLButtonElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    const right = window.innerWidth - r.right;
    const openUp = r.bottom + 248 > window.innerHeight;
    setMenu(
      openUp
        ? { right, bottom: window.innerHeight - r.top + 4, minWidth: r.width }
        : { right, top: r.bottom + 4, minWidth: r.width },
    );
  };
  const current = options.find((o) => o.value === value)?.label ?? value ?? "";
  return (
    <>
      {buttonLabel ? (
        <button
          type="button"
          onClick={openMenu}
          className="gs-text-body-md-medium h-[29px] shrink-0 rounded-[2px] border border-border-default px-[9px] whitespace-nowrap text-text-subdued transition-colors hover:text-text-default"
        >
          {buttonLabel}
        </button>
      ) : (
        <button
          type="button"
          onClick={openMenu}
          className={`gs-text-body-md-regular flex h-[29px] items-center justify-between gap-2 rounded-[4px] border border-border-default bg-background-input pr-2 pl-2.5 text-left text-text-default ${className}`}
        >
          <span className="truncate">{current}</span>
          <ChevronDown className="size-3 shrink-0 text-text-subdued" />
        </button>
      )}
      {menu &&
        createPortal(
          <>
            <div className="fixed inset-0 z-[70]" onMouseDown={() => setMenu(null)} />
            <div
              style={menu}
              className="fixed z-[71] flex max-h-[240px] flex-col overflow-y-auto rounded-[6px] border border-border-default bg-menu-surface p-1 shadow-2xl"
            >
              {options.map((o) => (
                <button
                  key={o.value ?? ""}
                  type="button"
                  onClick={() => {
                    setMenu(null);
                    if (o.value !== value) onChange(o.value);
                  }}
                  className="gs-text-body-md-regular flex h-8 shrink-0 items-center justify-between gap-4 rounded-[4px] px-2.5 text-left whitespace-nowrap text-text-default hover:bg-transparent-secondary"
                >
                  {o.label}
                  {o.value === value && <Check className="size-3.5 text-text-accent" />}
                </button>
              ))}
            </div>
          </>,
          document.body,
        )}
    </>
  );
}

// ------------------------------------------------------------------
// Settings rows
// ------------------------------------------------------------------
export function SettingsRow({
  icon: Icon,
  title,
  description,
  note,
  children,
}: {
  icon: IconType;
  title: string;
  description: string;
  /** Error / warning / hint shown under the row. */
  note?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="flex flex-col">
      {/* Wraps the control onto its own line when the window is too narrow. */}
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2 px-3 py-4">
        <div className="flex size-10 shrink-0 items-center justify-center rounded-[4px] bg-transparent-primary text-text-default">
          <Icon className="size-[18px]" />
        </div>
        <div className="flex min-w-[140px] flex-1 flex-col gap-1">
          <span className="gs-text-body-md-regular text-text-default">{title}</span>
          <span className="gs-text-body-sm-regular text-text-subdued">{description}</span>
        </div>
        <div className="ml-auto flex max-w-full items-center justify-end">{children}</div>
      </div>
      {note && <div className="-mt-2 flex flex-col gap-1 pr-3 pb-2 pl-[68px]">{note}</div>}
    </div>
  );
}

export function SettingsSection({ label }: { label: string }) {
  return <span className="gs-text-tag px-3 pt-6 pb-2 text-text-subdued select-none">{label}</span>;
}

export function NoteText({ tone, children }: { tone: "error" | "warning" | "hint"; children: React.ReactNode }) {
  return (
    <p
      className={`gs-text-body-sm-regular flex items-start gap-1.5 select-text ${
        tone === "error" ? "text-text-error" : "text-text-subdued"
      }`}
    >
      {tone === "warning" && <TriangleAlert className="mt-px size-3.5 shrink-0 text-text-accent" />}
      {children}
    </p>
  );
}

// Hotkey binding strings: see the doc comment in src-tauri/src/hotkeys.rs.
export const MODIFIER_CODES = [
  "Fn",
  "ControlLeft",
  "ControlRight",
  "AltLeft",
  "AltRight",
  "ShiftLeft",
  "ShiftRight",
  "MetaLeft",
  "MetaRight",
];

export const KEY_LABELS: Record<string, string> = {
  Fn: "fn",
  ControlLeft: "Left ^",
  ControlRight: "Right ^",
  AltLeft: "Left ⌥",
  AltRight: "Right ⌥",
  ShiftLeft: "Left ⇧",
  ShiftRight: "Right ⇧",
  MetaLeft: "Left ⌘",
  MetaRight: "Right ⌘",
  Control: IS_MAC ? "^" : "Ctrl",
  Alt: IS_MAC ? "⌥" : "Alt",
  Shift: "⇧",
  Meta: IS_MAC ? "⌘" : "Win",
  Space: "Spacebar",
  Enter: "Enter",
};

export function bindingLabels(binding: string): string[] {
  return binding
    .split("+")
    .filter(Boolean)
    .map((code) => KEY_LABELS[code] ?? code.replace(/^(Key|Digit)(?=.$)/, ""));
}

/** Canonical binding: modifiers in a fixed order, then at most one key. */
export function toBinding(codes: string[]): string {
  const key = codes.find((c) => !MODIFIER_CODES.includes(c));
  return [...MODIFIER_CODES.filter((m) => codes.includes(m)), ...(key ? [key] : [])].join("+");
}

export function Keycaps({ labels }: { labels: string[] }) {
  return (
    <span className="flex items-center gap-1.5">
      {labels.map((label, i) => (
        <kbd
          key={i}
          className="inline-flex h-6 min-w-6 items-center justify-center rounded-[2px] bg-transparent-tertiary px-[9px] font-mono text-[12px] leading-4 whitespace-nowrap text-text-subdued"
        >
          {label}
        </kbd>
      ))}
    </span>
  );
}

// ------------------------------------------------------------------
// Add-entry modal (dictionary word / snippet)
// ------------------------------------------------------------------
export function AddEntryModal({
  title,
  fields,
  hint,
  submitLabel,
  onSubmit,
  onClose,
}: {
  title: string;
  fields: {
    label: string;
    value: string;
    onChange: (v: string) => void;
    placeholder: string;
    multiline?: boolean;
  }[];
  hint?: string;
  submitLabel: string;
  onSubmit: () => void;
  onClose: () => void;
}) {
  return (
    <Modal onClose={onClose} width="w-[420px] max-w-[90vw]">
      <div className="flex flex-col gap-4 p-6">
        <h3 className="gs-text-heading-md text-text-default">{title}</h3>
        {fields.map((f, i) => (
          <div key={i} className="flex flex-col gap-1.5">
            <label className="gs-text-body-sm-regular text-text-subdued">{f.label}</label>
            {f.multiline ? (
              <textarea
                value={f.value}
                onChange={(e) => f.onChange(e.target.value)}
                placeholder={f.placeholder}
                rows={3}
                autoFocus={i === 0}
                className="gs-text-body-sm-regular w-full resize-none rounded-[2px] border border-border-default bg-background-input px-2.5 py-2 text-text-default placeholder:text-text-disabled focus:outline-none"
              />
            ) : (
              <input
                value={f.value}
                onChange={(e) => f.onChange(e.target.value)}
                placeholder={f.placeholder}
                autoFocus={i === 0}
                onKeyDown={(e) => {
                  if (isPlainEnter(e)) onSubmit();
                }}
                className="gs-text-body-sm-regular h-[28px] w-full rounded-[2px] border border-border-default bg-background-input px-2.5 text-text-default placeholder:text-text-disabled focus:outline-none"
              />
            )}
          </div>
        ))}
        {hint && <p className="gs-text-body-xs-regular text-text-disabled">{hint}</p>}
        <div className="flex items-center justify-end gap-2 pt-1">
          <OutlineButton onClick={onClose}>{t("Cancel")}</OutlineButton>
          <LimeButton onClick={onSubmit}>
            <Plus className="size-3" />
            {submitLabel}
          </LimeButton>
        </div>
      </div>
    </Modal>
  );
}

// ------------------------------------------------------------------
// Empty state (04-snippets.png)
// ------------------------------------------------------------------
export function EmptyState({
  icon: Icon,
  title,
  description,
  actionLabel,
  onAction,
}: {
  icon: IconType;
  title: string;
  description: string;
  actionLabel: string;
  onAction: () => void;
}) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center pt-6 pb-8">
      <div className="flex size-12 items-center justify-center rounded-[6px] bg-transparent-primary">
        <Icon className="size-5 text-text-subdued" strokeWidth={1.5} />
      </div>
      <span className="gs-text-heading-md mt-[17px] text-text-default">{title}</span>
      <p className="gs-text-body-md-regular mt-[7px] max-w-[320px] text-center text-text-subdued">
        {description}
      </p>
      <div className="mt-5">
        <LimeButton onClick={onAction}>
          <Plus className="size-3" />
          {actionLabel}
        </LimeButton>
      </div>
    </div>
  );
}

// ------------------------------------------------------------------
// Header card shared by Formatting / Dictionary / Snippets
// ------------------------------------------------------------------
export function HeaderCard({ title, description }: { title: string; description: string }) {
  return (
    <div className="flex shrink-0 flex-col gap-2 rounded-[8px] bg-background-card p-8">
      <h2 className="gs-text-heading-md text-text-default">{title}</h2>
      <p className="gs-text-body-md-regular text-text-subdued">{description}</p>
    </div>
  );
}
