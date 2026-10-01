// Cloud presets for Settings › Model. The config keeps full endpoint URLs:
// `/audio/transcriptions` for speech to text (`/speech-to-text` for ElevenLabs), `/chat/completions` for chat.
// Any other provider id means "custom" (the user's own URL).
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
  const pinned = kind === "stt" && (provider?.id === "openai" || provider?.id === "elevenlabs") ? presets : [];
  return [...new Set([...pinned, ...(discovered?.length ? discovered : presets), current].filter(Boolean))];
}

/** ".../v1/chat/completions" or ".../v1/audio/transcriptions/" → ".../v1". */
export function baseFromUrl(url: string): string {
  return url
    .trim()
    .replace(/\/+$/, "")
    .replace(/\/(audio\/transcriptions|chat\/completions|speech-to-text)$/, "");
}

export function endpointUrl(baseUrl: string, kind: "stt" | "llm"): string {
  const base = baseFromUrl(baseUrl);
  const path = kind === "llm" ? "chat/completions" : base === "https://api.elevenlabs.io/v1" ? "speech-to-text" : "audio/transcriptions";
  return `${base}/${path}`;
}
