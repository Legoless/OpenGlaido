import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Plus, Trash2 } from "lucide-react";
import { t } from "./i18n";
import type { SnippetItem } from "./types";
import { AddEntryModal, EmptyState, HeaderCard, OutlineButton, SnippetIcon } from "./ui";

export function SnippetsPage({
  snippets,
  onChanged,
  addOpen,
  onAddOpenChange,
}: {
  snippets: SnippetItem[];
  /** Reloads snippets after a change made here. */
  onChanged: () => void;
  /** The "Add snippet" modal (App owns it so ⌘K and Esc can reach it). */
  addOpen: boolean;
  onAddOpenChange: (open: boolean) => void;
}) {
  const [newTrigger, setNewTrigger] = useState("");
  const [newContent, setNewContent] = useState("");

  const addSnippet = async () => {
    if (!newTrigger.trim() || !newContent.trim()) return;
    try {
      await invoke("add_snippet", { trigger: newTrigger.trim(), content: newContent.trim() });
      setNewTrigger("");
      setNewContent("");
      onAddOpenChange(false);
      onChanged();
    } catch (e) {
      console.error(e);
    }
  };
  const deleteSnippet = async (id: string) => {
    try {
      await invoke("delete_snippet", { id });
      onChanged();
    } catch (e) {
      console.error(e);
    }
  };

  return (
    <>
      <HeaderCard
        title={t("Expand short phrases into full text")}
        description={t("Say a short trigger phrase and OpenGlaido expands it into a full block of text.")}
      />
      <div className="flex min-h-0 flex-1 flex-col rounded-[8px] bg-background-card pt-8 pb-8 pl-8">
        <div className="flex min-h-7 shrink-0 items-center gap-3 pr-8">
          <span className="gs-text-tag text-text-default">{t("Snippets")}</span>
          <div className="flex-1" />
          <OutlineButton onClick={() => onAddOpenChange(true)}>
            <Plus className="size-3" />
            {t("Add snippet")}
          </OutlineButton>
        </div>
        <div className="mt-5 flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto pr-[18px] [scrollbar-gutter:stable]">
          {snippets.length === 0 ? (
            <EmptyState
              icon={SnippetIcon}
              title={t("No snippets added")}
              description={t("Add your first snippet to create a shortcut for phrases you use often.")}
              actionLabel={t("Add snippet")}
              onAction={() => onAddOpenChange(true)}
            />
          ) : (
            snippets.map((snip) => (
              <div
                key={snip.id}
                className="group flex items-center justify-between gap-4 rounded-[4px] px-3 py-[14px] transition-colors hover:bg-transparent-primary"
              >
                <div className="flex min-w-0 flex-1 items-center gap-3">
                  <span className="gs-text-body-md-medium shrink-0 truncate text-text-default">
                    {snip.trigger}
                  </span>
                  <span className="shrink-0 text-text-disabled">→</span>
                  <span className="gs-text-body-md-regular truncate text-text-subdued">
                    {snip.content}
                  </span>
                </div>
                <button
                  type="button"
                  onClick={() => deleteSnippet(snip.id)}
                  title={t("Delete snippet")}
                  className="shrink-0 text-text-disabled opacity-0 transition-opacity group-hover:opacity-100 hover:text-text-error"
                >
                  <Trash2 className="size-4" />
                </button>
              </div>
            ))
          )}
        </div>
      </div>

      {addOpen && (
        <AddEntryModal
          title={t("Add snippet")}
          fields={[
            {
              label: t("Trigger phrase"),
              value: newTrigger,
              onChange: setNewTrigger,
              placeholder: t("e.g. my email"),
            },
            {
              label: t("Expanded text"),
              value: newContent,
              onChange: setNewContent,
              placeholder: t("e.g. name@domain.com"),
              multiline: true,
            },
          ]}
          hint={t("Supports dynamic tags: {date}, {time}, {clipboard}.")}
          submitLabel={t("Add snippet")}
          onSubmit={addSnippet}
          onClose={() => onAddOpenChange(false)}
        />
      )}
    </>
  );
}
