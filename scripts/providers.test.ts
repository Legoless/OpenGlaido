// bun test scripts/providers.test.ts
import { expect, test } from "bun:test";
import { PROVIDERS, baseFromUrl, endpointUrl, providerModelIds } from "../src/providers";

test("OpenAI keeps completed and live transcription choices after model discovery", () => {
  const openai = PROVIDERS.find((p) => p.id === "openai")!;
  expect(openai.defaultStt).toBe("gpt-transcribe");
  expect(providerModelIds(openai, "stt", undefined, "")).toContain("gpt-live-transcribe");
  const ids = providerModelIds(openai, "stt", ["whisper-1", "another-model"], "custom-model");
  expect(ids).toEqual([...openai.sttModels, "another-model", "custom-model"]);
  expect(providerModelIds(openai, "llm", ["chat-model"], "chat-model")).toEqual(["chat-model"]);
  expect(providerModelIds(PROVIDERS[0], "stt", ["provider-model"], "provider-model")).toEqual(["provider-model"]);
  expect(providerModelIds(undefined, "stt", undefined, "custom-model")).toEqual(["custom-model"]);
});

test("ElevenLabs offers both Scribe modes for transcription only", () => {
  const elevenlabs = PROVIDERS.find((p) => p.id === "elevenlabs")!;
  expect(elevenlabs.stt).toBe(true);
  expect(elevenlabs.chat).toBe(false);
  expect(elevenlabs.chatModels).toEqual([]);
  expect(elevenlabs.defaultStt).toBe("scribe_v2");
  expect(providerModelIds(elevenlabs, "stt", ["another-model"], "scribe_v2_realtime"))
    .toEqual(["scribe_v2", "scribe_v2_realtime", "another-model"]);
});

test("provider endpoint URLs preserve ElevenLabs and OpenAI paths", () => {
  expect(endpointUrl("https://api.elevenlabs.io/v1/", "stt")).toBe("https://api.elevenlabs.io/v1/speech-to-text");
  expect(baseFromUrl(" https://api.elevenlabs.io/v1/speech-to-text/ ")).toBe("https://api.elevenlabs.io/v1");
  expect(endpointUrl("https://api.openai.com/v1/", "stt")).toBe("https://api.openai.com/v1/audio/transcriptions");
  expect(baseFromUrl("https://api.openai.com/v1/audio/transcriptions/")).toBe("https://api.openai.com/v1");
  expect(endpointUrl("https://api.openai.com/v1/audio/transcriptions", "llm")).toBe("https://api.openai.com/v1/chat/completions");
});
