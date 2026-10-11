import { useState } from "react";
import { History, X } from "lucide-react";
import { version } from "../package.json";
import { changelog, type Changelog } from "./changelog";
import { locale, t } from "./i18n";
import { IconButton, Modal, OutlineButton, SettingsRow } from "./ui";

function ChangeList({ changes }: { changes: string[] }) {
  return (
    <ul className="gs-text-body-md-regular mt-3 list-disc space-y-2 pl-5 text-text-subdued select-text">
      {changes.map((change, index) => <li key={index} className="[overflow-wrap:anywhere]">{change}</li>)}
    </ul>
  );
}

/** Bundled, plain-text release notes. Authored changes stay in English; labels and dates follow the app language. */
export function ChangelogContent({
  currentVersion = version,
  data = changelog,
}: {
  currentVersion?: string;
  data?: Changelog;
}) {
  const dates = new Intl.DateTimeFormat(locale(), { dateStyle: "long", timeZone: "UTC" });
  const current = data.releases.findIndex((release) => release.version === currentVersion);
  // Drafts and newer prepared entries are never part of the in-app history.
  const releases = current < 0 ? [] : data.releases.slice(current);
  return (
    <div className="space-y-6">
      {releases.map((release) => (
        <section key={release.version}>
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <h3 className="gs-text-heading-sm text-text-default">{t("Version {v}", { v: release.version })}</h3>
            {release.version === currentVersion && (
              <span className="gs-text-body-xs-regular rounded-[4px] bg-transparent-primary px-2 py-1 text-text-accent">{t("Current version")}</span>
            )}
          </div>
          <time dateTime={release.date} className="gs-text-body-sm-regular mt-1 block text-text-disabled">
            {dates.format(new Date(`${release.date}T00:00:00Z`))}
          </time>
          <ChangeList changes={release.changes} />
        </section>
      ))}
    </div>
  );
}

export function ChangelogModal({
  currentVersion,
  data,
  onClose,
}: {
  currentVersion?: string;
  data?: Changelog;
  onClose: () => void;
}) {
  return (
    <Modal onClose={onClose} width="flex max-h-[min(680px,calc(100vh-40px))] w-[600px] max-w-[92vw] flex-col">
      <div className="flex shrink-0 items-center justify-between px-6 pt-6 pb-4">
        <h2 className="text-[20px] leading-7 font-medium tracking-[0.01em] text-text-default">{t("Changelog")}</h2>
        <IconButton onClick={onClose} title={t("Close")}>
          <X className="size-3.5" strokeWidth={1.5} />
        </IconButton>
      </div>
      <div tabIndex={0} role="region" aria-label={t("Changelog")} className="min-h-0 overflow-y-auto px-6 pb-6 [scrollbar-gutter:stable]">
        <ChangelogContent currentVersion={currentVersion} data={data} />
      </div>
    </Modal>
  );
}

export function ChangelogSettings({ currentVersion }: { currentVersion?: string }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <SettingsRow icon={History} title={t("Changelog")} description={t("See what’s new in OpenGlaido")}>
        <OutlineButton onClick={() => setOpen(true)}>{t("View changelog")}</OutlineButton>
      </SettingsRow>
      {open && <ChangelogModal currentVersion={currentVersion} onClose={() => setOpen(false)} />}
    </>
  );
}
