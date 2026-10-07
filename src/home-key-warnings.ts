import { BACKUP_ROLES, MODEL_FIELDS, PROVIDERS, baseFromUrl, hasLivePrimary, isBackupRole, isCloudLiveModel, modelKeyScope, type ModelRole } from "./providers";
import type { TranscriptionConfig } from "./types";

export type HomeKeyCheck = { id: string; provider: string; baseUrl: string; apiKey: string };
export type HomeKeyResult = { id: string; result: "verified" | "unsupported" | "failed" };

/** Check only active cloud credentials with a known authenticated check. */
export function homeKeyChecks(config: TranscriptionConfig | null): HomeKeyCheck[] {
  if (!config) return [];
  const roles: ModelRole[] = ["stt", "llm", ...(hasLivePrimary(config) ? BACKUP_ROLES : [])];
  return roles.flatMap((role) => {
    if (isBackupRole(role)) {
      const f = MODEL_FIELDS[role];
      if (!config[f.enabled] || !isCloudLiveModel(config[f.model])) return [];
    } else if (config[`${role}_source`] !== "cloud") return [];
    const f = MODEL_FIELDS[role];
    const apiKey = config[f.key];
    const baseUrl = baseFromUrl(config[f.url] ?? "");
    const provider = PROVIDERS.find((p) => p.baseUrl === baseUrl && p.keyUrl);
    if (!apiKey.trim() || !provider) return [];
    return [{ id: JSON.stringify([modelKeyScope(config, role), apiKey]), provider: provider.name, baseUrl, apiKey }];
  });
}

/** Old credentials and unsupported checks are neutral; success allows a later failure to notify again. */
export function homeKeyWarnings(checks: HomeKeyCheck[], results: HomeKeyResult[], warned: Set<string>, canNotify: boolean) {
  const active = new Map(checks.map((check) => [check.id, check]));
  const nextWarned = new Set([...warned].filter((id) => active.has(id)));
  const providers = new Set<string>();
  for (const { id, result } of results) {
    const check = active.get(id);
    if (!check) continue;
    if (result === "verified") nextWarned.delete(id);
    if (result === "failed" && canNotify && !nextWarned.has(id)) {
      nextWarned.add(id);
      providers.add(check.provider);
    }
  }
  return { warned: nextWarned, providers: [...providers] };
}
