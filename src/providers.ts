// Cloud presets for Settings › Model (OpenAI-compatible APIs). The config keeps full endpoint URLs:
// `${baseUrl}/audio/transcriptions` for speech to text and `${baseUrl}/chat/completions` for chat.
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
    sttModels: ["gpt-transcribe", "gpt-4o-mini-transcribe", "whisper-1"],
    chatModels: ["gpt-5.4-nano", "gpt-5.4-mini"],
    defaultStt: "gpt-transcribe",
    defaultChat: "gpt-5.4-nano",
    keyUrl: "https://platform.openai.com/api-keys",
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

/** ".../v1/chat/completions" or ".../v1/audio/transcriptions/" → ".../v1". */
export function baseFromUrl(url: string): string {
  return url
    .trim()
    .replace(/\/+$/, "")
    .replace(/\/(audio\/transcriptions|chat\/completions)$/, "");
}

export function endpointUrl(baseUrl: string, kind: "stt" | "llm"): string {
  return `${baseFromUrl(baseUrl)}/${kind === "stt" ? "audio/transcriptions" : "chat/completions"}`;
}
