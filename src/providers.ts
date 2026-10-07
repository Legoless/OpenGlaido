// Cloud presets for Settings › Model. The config keeps full endpoint URLs:
// `/audio/transcriptions` for speech to text (`/speech-to-text` for ElevenLabs), `/chat/completions` for chat.
// Microsoft keeps the user's Azure resource root; its adapters choose the request path.
// Any other provider id means "custom" (the user's own URL).
import type { LocalModel, TranscriptionConfig } from "./types";

// The original top-level config fields are the cloud transcription ones.
export const MODEL_FIELDS = {
  stt: { provider: "stt_provider", url: "endpoint_url", model: "model_name", key: "api_key", local: "local_stt_model", deployment: "stt_deployment" },
  llm: { provider: "llm_provider", url: "llm_endpoint_url", model: "llm_model_name", key: "llm_api_key", local: "local_llm_model", deployment: null },
  stt_backup_1: { provider: "stt_backup_1_provider", url: "stt_backup_1_endpoint_url", model: "stt_backup_1_model_name", key: "stt_backup_1_api_key", deployment: "stt_backup_1_deployment", enabled: "stt_backup_1_enabled" },
  stt_backup_2: { provider: "stt_backup_2_provider", url: "stt_backup_2_endpoint_url", model: "stt_backup_2_model_name", key: "stt_backup_2_api_key", deployment: "stt_backup_2_deployment", enabled: "stt_backup_2_enabled" },
  stt_backup_3: { provider: "stt_backup_3_provider", url: "stt_backup_3_endpoint_url", model: "stt_backup_3_model_name", key: "stt_backup_3_api_key", deployment: "stt_backup_3_deployment", enabled: "stt_backup_3_enabled" },
  stt_backup_4: { provider: "stt_backup_4_provider", url: "stt_backup_4_endpoint_url", model: "stt_backup_4_model_name", key: "stt_backup_4_api_key", deployment: "stt_backup_4_deployment", enabled: "stt_backup_4_enabled" },
} as const;

export type ModelRole = keyof typeof MODEL_FIELDS;
export type BackupRole = "stt_backup_1" | "stt_backup_2" | "stt_backup_3" | "stt_backup_4";
export const BACKUP_ROLES: BackupRole[] = ["stt_backup_1", "stt_backup_2", "stt_backup_3", "stt_backup_4"];
export const MAX_TRANSCRIPTION_PROVIDERS = 1 + BACKUP_ROLES.length;
export const modelKind = (role: ModelRole): LocalModel["kind"] => role === "llm" ? "llm" : "stt";
export const isBackupRole = (role: ModelRole): role is BackupRole => BACKUP_ROLES.includes(role as BackupRole);

/** Matches the backend's cloud live adapters, including pinned OpenAI model versions. */
export function isCloudLiveModel(model: string): boolean {
  return model === "gpt-live-transcribe" || /^gpt-live-transcribe-\d{4}-\d{2}-\d{2}$/.test(model)
    || model === "scribe_v2_realtime" || model === "mai-transcribe-2-streaming";
}

export function hasLivePrimary(config: TranscriptionConfig): boolean {
  return config.stt_source === "cloud" ? isCloudLiveModel(config.model_name)
    : config.stt_source === "local" && config.local_stt_model === "vibevoice-asr-streaming-7b";
}

export interface Provider {
  id: string;
  name: string;
  baseUrl: string;
  stt: boolean;
  chat: boolean;
  sttModels: string[];
  chatModels: string[];
  defaultStt: string;
  defaultChat: string;
  /** Where to create an API key; "" = the provider needs none (local servers). */
  keyUrl: string;
}

export const PROVIDERS: Provider[] = [
  {
    id: "groq",
    name: "Groq",
    baseUrl: "https://api.groq.com/openai/v1",
    stt: true,
    chat: true,
    sttModels: ["whisper-large-v3-turbo", "whisper-large-v3"],
    chatModels: ["openai/gpt-oss-20b", "openai/gpt-oss-120b"],
    defaultStt: "whisper-large-v3-turbo",
    defaultChat: "openai/gpt-oss-20b",
    keyUrl: "https://console.groq.com/keys",
  },
  {
    id: "openai",
    name: "OpenAI",
    baseUrl: "https://api.openai.com/v1",
    stt: true,
    chat: true,
    sttModels: ["gpt-transcribe", "gpt-live-transcribe", "gpt-4o-mini-transcribe", "whisper-1"],
    chatModels: ["gpt-5.4-nano", "gpt-5.4-mini"],
    defaultStt: "gpt-transcribe",
    defaultChat: "gpt-5.4-nano",
    keyUrl: "https://platform.openai.com/api-keys",
  },
  {
    id: "elevenlabs",
    name: "ElevenLabs",
    baseUrl: "https://api.elevenlabs.io/v1",
    stt: true,
    chat: false,
    sttModels: ["scribe_v2", "scribe_v2_realtime"],
    chatModels: [],
    defaultStt: "scribe_v2",
    defaultChat: "",
    keyUrl: "https://elevenlabs.io/app/developers/api-keys",
  },
  {
    id: "microsoft",
    name: "Microsoft Azure",
    baseUrl: "",
    stt: true,
    chat: false,
    sttModels: ["mai-transcribe-2", "mai-transcribe-2-streaming"],
    chatModels: [],
    defaultStt: "mai-transcribe-2",
    defaultChat: "",
    keyUrl: "https://ai.azure.com/",
  },
  {
    id: "openrouter",
    name: "OpenRouter",
    baseUrl: "https://openrouter.ai/api/v1",
    stt: true,
    chat: true,
    sttModels: ["openai/whisper-large-v3-turbo"],
    chatModels: ["google/gemini-3.5-flash-lite", "openai/gpt-oss-120b"],
    defaultStt: "openai/whisper-large-v3-turbo",
    defaultChat: "google/gemini-3.5-flash-lite",
    keyUrl: "https://openrouter.ai/settings/keys",
  },
  {
    id: "mistral",
    name: "Mistral",
    baseUrl: "https://api.mistral.ai/v1",
    stt: true,
    chat: true,
    sttModels: ["voxtral-mini-latest"],
    chatModels: ["mistral-small-latest"],
    defaultStt: "voxtral-mini-latest",
    defaultChat: "mistral-small-latest",
    keyUrl: "https://console.mistral.ai/api-keys",
  },
  {
    id: "gemini",
    name: "Google Gemini",
    baseUrl: "https://generativelanguage.googleapis.com/v1beta/openai",
    stt: false,
    chat: true,
    sttModels: [],
    chatModels: ["gemini-3.5-flash-lite"],
    defaultStt: "",
    defaultChat: "gemini-3.5-flash-lite",
    keyUrl: "https://aistudio.google.com/apikey",
  },
  {
    id: "ollama",
    name: "Ollama",
    baseUrl: "http://localhost:11434/v1",
    stt: false,
    chat: true,
    sttModels: [],
    chatModels: [],
    defaultStt: "",
    defaultChat: "",
    keyUrl: "",
  },
  {
    id: "lmstudio",
    name: "LM Studio",
    baseUrl: "http://localhost:1234/v1",
    stt: false,
    chat: true,
    sttModels: [],
    chatModels: [],
    defaultStt: "",
    defaultChat: "",
    keyUrl: "",
  },
];

/** Keep completed and live choices available even when model discovery omits them. */
export function providerModelIds(provider: Provider | undefined, kind: "stt" | "llm", discovered: string[] | undefined, current: string): string[] {
  const presets = (kind === "stt" ? provider?.sttModels : provider?.chatModels) ?? [];
  const pinned = kind === "stt" && ["openai", "elevenlabs", "microsoft"].includes(provider?.id ?? "") ? presets : [];
  return [...new Set([...pinned, ...(discovered?.length ? discovered : presets), current].filter(Boolean))];
}

/** ".../v1/chat/completions" or ".../v1/audio/transcriptions/" → ".../v1". */
export function baseFromUrl(url: string): string {
  return url
    .trim()
    .replace(/\/+$/, "")
    .replace(/\/(audio\/transcriptions|chat\/completions|speech-to-text)$/, "");
}

export function endpointUrl(baseUrl: string, kind: "stt" | "llm", providerId?: string): string {
  const base = baseFromUrl(baseUrl);
  if (kind === "stt" && providerId === "microsoft") return base;
  const path = kind === "llm" ? "chat/completions" : base === "https://api.elevenlabs.io/v1" ? "speech-to-text" : "audio/transcriptions";
  return `${base}/${path}`;
}

/** Azure resource endpoints vary per account and stay editable without becoming Custom. */
export function providerPreset(config: TranscriptionConfig, role: ModelRole): Provider | undefined {
  const f = MODEL_FIELDS[role];
  const kind = modelKind(role);
  return PROVIDERS.find((p) => p.id === config[f.provider] && (kind === "stt" ? p.stt : p.chat)
    && (p.id === "microsoft" || endpointUrl(p.baseUrl, kind) === config[f.url]?.trim()));
}

/** Clear the visible old key immediately; save_config restores this provider's remembered key. */
export function providerSelection(config: TranscriptionConfig, role: ModelRole, id: string): Partial<TranscriptionConfig> | null {
  const f = MODEL_FIELDS[role];
  const kind = modelKind(role);
  const current = providerPreset(config, role);
  if (id === (current?.id ?? "custom")) return null;
  const provider = PROVIDERS.find((p) => p.id === id);
  return provider
    ? {
        [f.provider]: id,
        [f.url]: endpointUrl(provider.baseUrl, kind, provider.id),
        [f.model]: isBackupRole(role) ? provider.sttModels.find(isCloudLiveModel) ?? "gpt-live-transcribe"
          : kind === "stt" ? provider.defaultStt : provider.defaultChat,
        [f.key]: "",
        ...(f.deployment && id === "microsoft" ? { [f.deployment]: "" } : {}),
      }
    : { [f.provider]: "custom", [f.key]: "" };
}

/** Distinguish an explicit key edit (including deletion) from a temporary blank while switching. */
export function editedModelKeys(patch: Partial<TranscriptionConfig>): string[] {
  return Object.keys(patch).filter((key) => {
    const role = (Object.keys(MODEL_FIELDS) as ModelRole[]).find((role) => MODEL_FIELDS[role].key === key);
    if (!role) return false;
    const f = MODEL_FIELDS[role];
    return typeof patch[f.key] === "string" && !(f.provider in patch) && !(f.url in patch) && !(`${role}_source` in patch);
  });
}

/** Also used to discard an uncommitted password draft when its destination changes. */
export function modelKeyScope(config: TranscriptionConfig, role: ModelRole): string {
  const f = MODEL_FIELDS[role];
  return JSON.stringify([role, config[f.provider], config[f.url] ?? ""]);
}

export type ApiKeyCheck = { scope: string; key: string; status: "checking" | "verified" | "warning" };

/** Every verification state belongs only to the exact provider, endpoint and key checked. */
export function apiKeyCheckStatus(check: ApiKeyCheck | null, scope: string, key: string): ApiKeyCheck["status"] | null {
  return key.trim() && check?.scope === scope && check.key === key ? check.status : null;
}

export function scopedConfigPatch(config: TranscriptionConfig, patch: Partial<TranscriptionConfig>) {
  const keyScopes = editedModelKeys(patch).map((key) => {
    const role = (Object.keys(MODEL_FIELDS) as ModelRole[]).find((role) => MODEL_FIELDS[role].key === key)!;
    return [role, modelKeyScope(config, role)] as const;
  });
  return { patch, keyScopes };
}

/** A failed provider switch must not rebase its queued key edit onto the previous provider. */
export function applyConfigPatch(config: TranscriptionConfig, pending: ReturnType<typeof scopedConfigPatch>): TranscriptionConfig | null {
  if (pending.keyScopes.some(([kind, scope]) => modelKeyScope(config, kind) !== scope)) return null;
  return { ...config, ...pending.patch };
}
