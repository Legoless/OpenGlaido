// Cloud presets for Settings › Model. The config keeps full endpoint URLs:
// `/audio/transcriptions` for speech to text (`/speech-to-text` for ElevenLabs), `/chat/completions` for chat.
// Microsoft keeps the user's Azure resource root; its adapters choose the request path.
// Any other provider id means "custom" (the user's own URL).
import type { LocalModel, TranscriptionConfig } from "./types";

// The original top-level config fields are the cloud transcription ones.
export const MODEL_FIELDS = {
  stt: { provider: "stt_provider", url: "endpoint_url", model: "model_name", key: "api_key", local: "local_stt_model" },
  llm: { provider: "llm_provider", url: "llm_endpoint_url", model: "llm_model_name", key: "llm_api_key", local: "local_llm_model" },
} as const;

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
export function providerPreset(config: TranscriptionConfig, kind: LocalModel["kind"]): Provider | undefined {
  const f = MODEL_FIELDS[kind];
  return PROVIDERS.find((p) => p.id === config[f.provider] && (kind === "stt" ? p.stt : p.chat)
    && (p.id === "microsoft" || endpointUrl(p.baseUrl, kind) === config[f.url]?.trim()));
}

/** Clear the visible old key immediately; save_config restores this provider's remembered key. */
export function providerSelection(config: TranscriptionConfig, kind: LocalModel["kind"], id: string): Partial<TranscriptionConfig> | null {
  const f = MODEL_FIELDS[kind];
  const current = providerPreset(config, kind);
  if (id === (current?.id ?? "custom")) return null;
  const provider = PROVIDERS.find((p) => p.id === id);
  return provider
    ? {
        [f.provider]: id,
        [f.url]: endpointUrl(provider.baseUrl, kind, provider.id),
        [f.model]: kind === "stt" ? provider.defaultStt : provider.defaultChat,
        [f.key]: "",
        ...(kind === "stt" && id === "microsoft" ? { stt_deployment: "" } : {}),
      }
    : { [f.provider]: "custom", [f.key]: "" };
}

/** Distinguish an explicit key edit (including deletion) from a temporary blank while switching. */
export function editedModelKeys(patch: Partial<TranscriptionConfig>): string[] {
  return Object.keys(patch).filter((key) => {
    const kind = key === "api_key" ? "stt" : key === "llm_api_key" ? "llm" : null;
    if (!kind) return false;
    const f = MODEL_FIELDS[kind];
    return typeof patch[f.key] === "string" && !(f.provider in patch) && !(f.url in patch) && !(`${kind}_source` in patch);
  });
}

/** Also used to discard an uncommitted password draft when its destination changes. */
export function modelKeyScope(config: TranscriptionConfig, kind: LocalModel["kind"]): string {
  const f = MODEL_FIELDS[kind];
  return JSON.stringify([kind, config[f.provider], config[f.url] ?? ""]);
}

export type ApiKeyCheck = { scope: string; key: string; status: "checking" | "verified" | "warning" };

/** Every verification state belongs only to the exact provider, endpoint and key checked. */
export function apiKeyCheckStatus(check: ApiKeyCheck | null, scope: string, key: string): ApiKeyCheck["status"] | null {
  return key.trim() && check?.scope === scope && check.key === key ? check.status : null;
}

export function scopedConfigPatch(config: TranscriptionConfig, patch: Partial<TranscriptionConfig>) {
  const keyScopes = editedModelKeys(patch).map((key) => {
    const kind = key === "api_key" ? "stt" : "llm";
    return [kind, modelKeyScope(config, kind)] as const;
  });
  return { patch, keyScopes };
}

/** A failed provider switch must not rebase its queued key edit onto the previous provider. */
export function applyConfigPatch(config: TranscriptionConfig, pending: ReturnType<typeof scopedConfigPatch>): TranscriptionConfig | null {
  if (pending.keyScopes.some(([kind, scope]) => modelKeyScope(config, kind) !== scope)) return null;
  return { ...config, ...pending.patch };
}
