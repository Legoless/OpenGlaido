import { useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { BookOpen, Plus, Trash2 } from "lucide-react";
import { t } from "./i18n";
import type { DictionaryItem } from "./types";
import { AddEntryModal, EmptyState, HeaderCard, OutlineButton, SearchField } from "./ui";

export function DictionaryPage({
  dictionary,
  onChanged,
  addOpen,
  onAddOpenChange,
}: {
  dictionary: DictionaryItem[];
  /** Reloads the dictionary after a change made here. */
  onChanged: () => void;
  /** The "Add word" modal (App owns it so ⌘K and Esc can reach it). */
  addOpen: boolean;
  onAddOpenChange: (open: boolean) => void;
}) {
  const [dictQuery, setDictQuery] = useState("");
  const [newPhrase, setNewPhrase] = useState("");
  const [newReplacement, setNewReplacement] = useState("");

  const addWord = async () => {
    if (!newPhrase.trim()) return;
    try {
      await invoke("add_dictionary_entry", {
        phrase: newPhrase.trim(),
        replacement: newReplacement.trim(),
      });
      setNewPhrase("");
      setNewReplacement("");
      onAddOpenChange(false);
      onChanged();
    } catch (e) {
      console.error(e);
    }
  };
  const deleteWord = async (id: string) => {
    try {
      await invoke("delete_dictionary_entry", { id });
      onChanged();
    } catch (e) {
      console.error(e);
    }
  };

  const filteredDictionary = useMemo(() => {
    const q = dictQuery.trim().toLowerCase();
    if (!q) return dictionary;
    return dictionary.filter(
      (d) => d.phrase.toLowerCase().includes(q) || d.replacement.toLowerCase().includes(q),
    );
  }, [dictionary, dictQuery]);

  return (
    <>
      <HeaderCard
        title={t("Get every word right")}
        description={t("Add names, jargon, and spelling corrections so OpenGlaido gets them right every time.")}
      />
      <div className="flex min-h-0 flex-1 flex-col rounded-[8px] bg-background-card pt-8 pb-8 pl-8">
        <div className="flex min-h-7 shrink-0 items-center gap-3 pr-8">
          <span className="gs-text-tag text-text-default">{t("Dictionary")}</span>
          <div className="flex-1" />
          <SearchField
            value={dictQuery}
            onChange={setDictQuery}
            placeholder={t("Search words...")}
          />
          <OutlineButton onClick={() => onAddOpenChange(true)}>
            <Plus className="size-3" />
            {t("Add word")}
          </OutlineButton>
        </div>
        <div className="mt-5 flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto pr-[18px] [scrollbar-gutter:stable]">
          {filteredDictionary.length === 0 ? (
            <EmptyState
              icon={BookOpen}
              title={t("No words added")}
              description={t("Add names, jargon, and spelling corrections so OpenGlaido gets them right every time.")}
              actionLabel={t("Add word")}
              onAction={() => onAddOpenChange(true)}
            />
          ) : (
            filteredDictionary.map((item) => {
              const hasReplacement =
                item.replacement.trim() !== "" && item.replacement !== item.phrase;
              return (
                <div
                  key={item.id}
                  className="group flex items-center justify-between gap-4 rounded-[4px] px-3 py-[14px] transition-colors hover:bg-transparent-primary"
                >
                  <div className="flex min-w-0 flex-col gap-0.5">
                    <span className="gs-text-body-md-regular truncate text-text-default">
                      {hasReplacement ? item.replacement : item.phrase}
                    </span>
                    {hasReplacement && (
                      <span className="gs-text-body-xs-regular w-full truncate text-text-subdued">
                        {t("Replaces “{phrase}”", { phrase: item.phrase })}
                      </span>
                    )}
                  </div>
                  <button
                    type="button"
                    onClick={() => deleteWord(item.id)}
                    title={t("Delete word")}
                    className="shrink-0 text-text-disabled opacity-0 transition-opacity group-hover:opacity-100 hover:text-text-error"
                  >
                    <Trash2 className="size-4" />
                  </button>
                </div>
              );
            })
          )}
        </div>
      </div>

      {addOpen && (
        <AddEntryModal
          title={t("Add word")}
          fields={[
            {
              label: t("Word or phrase"),
              value: newPhrase,
              onChange: setNewPhrase,
              placeholder: t("e.g. Kubernetes"),
            },
            {
              label: t("Replace with (optional)"),
              value: newReplacement,
              onChange: setNewReplacement,
              placeholder: t("Leave empty to keep the word as-is"),
            },
          ]}
          submitLabel={t("Add word")}
          onSubmit={addWord}
          onClose={() => onAddOpenChange(false)}
        />
      )}
    </>
  );
}
