// bun test scripts/providers.test.ts
import { expect, test } from "bun:test";
import { MODEL_FIELDS, PROVIDERS, apiKeyCheckStatus, applyConfigPatch, baseFromUrl, editedModelKeys, endpointUrl, modelKeyScope, providerModelIds, providerPreset, providerSelection, scopedConfigPatch } from "../src/providers";
import type { TranscriptionConfig } from "../src/types";

test("API key checks never carry over to a changed key, provider, endpoint or role", () => {
  const config = { stt_provider: "openai", endpoint_url: "https://api.openai.com/v1/audio/transcriptions" } as TranscriptionConfig;
  const scope = modelKeyScope(config, "stt");
  expect(apiKeyCheckStatus(null, scope, "checked-key")).toBeNull();
  for (const status of ["checking", "verified", "warning"] as const) {
    const check = { scope, key: "checked-key", status };
    expect(apiKeyCheckStatus(check, scope, "checked-key")).toBe(status);
    expect(apiKeyCheckStatus(check, scope, "new-key")).toBeNull();
    expect(apiKeyCheckStatus(check, scope, "")).toBeNull();
    for (const changed of [
      { ...config, stt_provider: "custom" },
      { ...config, endpoint_url: "https://another.example/v1/audio/transcriptions" },
    ]) {
      expect(apiKeyCheckStatus(check, modelKeyScope(changed, "stt"), "checked-key")).toBeNull();
    }
    expect(apiKeyCheckStatus(check, modelKeyScope(config, "llm"), "checked-key")).toBeNull();
    expect(apiKeyCheckStatus({ scope, key: " ", status }, scope, " ")).toBeNull();
  }
});

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

test("Microsoft keeps both MAI modes and uses the account's resource root rather than an OpenAI URL", () => {
  const microsoft = PROVIDERS.find((p) => p.id === "microsoft")!;
  expect(microsoft.stt).toBe(true);
  expect(microsoft.chat).toBe(false);
  expect(microsoft.chatModels).toEqual([]);
  expect(microsoft.defaultStt).toBe("mai-transcribe-2");
  expect(providerModelIds(microsoft, "stt", ["another-model"], "mai-transcribe-2-streaming"))
    .toEqual(["mai-transcribe-2", "mai-transcribe-2-streaming", "another-model"]);
  expect(endpointUrl("https://resource.services.ai.azure.com/", "stt", "microsoft"))
    .toBe("https://resource.services.ai.azure.com");
  const before = {
    stt_provider: "openai", endpoint_url: endpointUrl("https://api.openai.com/v1", "stt"), api_key: "openai-key", stt_deployment: "old-deployment",
    llm_source: "off", llm_provider: "openai", llm_api_key: "llm-key",
  } as TranscriptionConfig;
  const patch = providerSelection(before, "stt", "microsoft")!;
  expect(patch).toEqual({ stt_provider: "microsoft", endpoint_url: "", model_name: "mai-transcribe-2", api_key: "", stt_deployment: "" });
  expect(editedModelKeys(patch)).toEqual([]);
  const selected = { ...before, ...patch, endpoint_url: "https://resource.services.ai.azure.com", api_key: "azure-key", stt_deployment: "custom-deployment" };
  expect(providerPreset(selected, "stt")?.id).toBe("microsoft");
  expect(providerSelection(selected, "stt", "microsoft")).toBeNull();
  expect(selected.llm_source).toBe("off");
  expect(selected.llm_api_key).toBe("llm-key");
  const scope = modelKeyScope(selected, "stt");
  expect(modelKeyScope({ ...selected, stt_deployment: "new-deployment" }, "stt")).toBe(scope);
  const newResource = { ...selected, endpoint_url: "https://another.services.ai.azure.com" };
  expect(providerPreset(newResource, "stt")?.id).toBe("microsoft");
  expect(modelKeyScope(newResource, "stt")).not.toBe(scope);
  expect(applyConfigPatch(newResource, scopedConfigPatch(selected, { api_key: "pending-old-resource-key" }))).toBeNull();
});

test("provider endpoint URLs preserve ElevenLabs and OpenAI paths", () => {
  expect(endpointUrl("https://api.elevenlabs.io/v1/", "stt")).toBe("https://api.elevenlabs.io/v1/speech-to-text");
  expect(baseFromUrl(" https://api.elevenlabs.io/v1/speech-to-text/ ")).toBe("https://api.elevenlabs.io/v1");
  expect(endpointUrl("https://api.openai.com/v1/", "stt")).toBe("https://api.openai.com/v1/audio/transcriptions");
  expect(baseFromUrl("https://api.openai.com/v1/audio/transcriptions/")).toBe("https://api.openai.com/v1");
  expect(endpointUrl("https://api.openai.com/v1/audio/transcriptions", "llm")).toBe("https://api.openai.com/v1/chat/completions");
});

test("provider switching cannot expose the previous or other section's key to the new endpoint", () => {
  const config = {
    stt_provider: "groq", endpoint_url: endpointUrl(PROVIDERS[0].baseUrl, "stt"), api_key: "speech-secret",
    llm_provider: "groq", llm_endpoint_url: endpointUrl(PROVIDERS[0].baseUrl, "llm"), llm_api_key: "chat-secret",
  } as TranscriptionConfig;
  for (const kind of ["stt", "llm"] as const) {
    const f = MODEL_FIELDS[kind];
    const other = MODEL_FIELDS[kind === "stt" ? "llm" : "stt"];
    // Even a key from the other section for the newly selected provider must wait for backend resolution.
    const before = { ...config, [other.provider]: "openai" };
    const changed = { ...before, ...providerSelection(before, kind, "openai") };
    expect(changed[f.url]).toBe(endpointUrl("https://api.openai.com/v1", kind));
    expect(changed[f.key]).toBe("");
    expect(changed[other.key]).toBe(before[other.key]);
    const custom = { ...before, ...providerSelection(before, kind, "custom") };
    expect(custom[f.provider]).toBe("custom");
    expect(custom[f.key]).toBe("");
  }
});

test("reselecting the current provider keeps its saved key and chosen model", () => {
  const config = {
    stt_provider: "openai", endpoint_url: endpointUrl("https://api.openai.com/v1", "stt"), api_key: "speech-secret", model_name: "gpt-live-transcribe",
    llm_provider: "custom", llm_endpoint_url: "https://example.com/v1/chat/completions", llm_api_key: "custom-secret", llm_model_name: "my-model",
  } as TranscriptionConfig;
  expect(providerSelection(config, "stt", "openai")).toBeNull();
  expect(providerSelection(config, "llm", "custom")).toBeNull();
});

test("only explicit key edits can update or delete remembered credentials", () => {
  expect(editedModelKeys({ api_key: "new-key" })).toEqual(["api_key"]);
  expect(editedModelKeys({ llm_api_key: "" })).toEqual(["llm_api_key"]);
  expect(editedModelKeys({ api_key: "", llm_api_key: "new-key" })).toEqual(["api_key", "llm_api_key"]);
  const config = { stt_provider: "custom", llm_provider: "custom" } as TranscriptionConfig;
  for (const kind of ["stt", "llm"] as const) {
    const f = MODEL_FIELDS[kind];
    expect(editedModelKeys(providerSelection(config, kind, "openai")!)).toEqual([]);
    expect(editedModelKeys({ [f.url]: "https://example.com/v1", [f.key]: "" })).toEqual([]);
    expect(editedModelKeys({ [`${kind}_source`]: "cloud", [f.key]: "" })).toEqual([]);
    expect(editedModelKeys({ [f.model]: "another-model" })).toEqual([]);
    expect(editedModelKeys({ [f.key]: undefined })).toEqual([]);
  }
  expect(editedModelKeys({ theme: "light" })).toEqual([]);
});

test("a failed provider switch never applies its queued key edit or deletion to the previous provider", () => {
  const before = {
    stt_provider: "groq", endpoint_url: endpointUrl(PROVIDERS[0].baseUrl, "stt"), api_key: "speech-secret",
    llm_provider: "groq", llm_endpoint_url: endpointUrl(PROVIDERS[0].baseUrl, "llm"), llm_api_key: "chat-secret",
  } as TranscriptionConfig;
  for (const kind of ["stt", "llm"] as const) {
    const f = MODEL_FIELDS[kind];
    const switchProvider = scopedConfigPatch(before, providerSelection(before, kind, "openai")!);
    const switched = applyConfigPatch(before, switchProvider)!;
    for (const value of ["openai-secret", ""]) {
      const edit = scopedConfigPatch(switched, { [f.key]: value });
      expect(applyConfigPatch(switched, edit)?.[f.key]).toBe(value);
      expect(applyConfigPatch(before, edit)).toBeNull();
      // Once the failed switch is removed, optimistic state excludes the orphaned edit too.
      const shown = [edit].reduce((config, pending) => applyConfigPatch(config, pending) ?? config, before);
      expect(shown[f.key]).toBe(before[f.key]);
      expect(shown[f.url]).toBe(before[f.url]);
    }
    // Different custom hosts remain separate even with the same provider id.
    const custom = { ...before, [f.provider]: "custom" };
    const otherHost = { ...custom, [f.url]: "https://another.example/v1" };
    expect(applyConfigPatch(custom, scopedConfigPatch(otherHost, { [f.key]: "other-secret" }))).toBeNull();
  }
});

test("password draft identity changes with provider or endpoint even when both saved keys are empty", () => {
  const config = { stt_provider: "groq", endpoint_url: "https://example.com/v1", api_key: "" } as TranscriptionConfig;
  const scope = modelKeyScope(config, "stt");
  expect(modelKeyScope({ ...config, stt_provider: "custom" }, "stt")).not.toBe(scope);
  expect(modelKeyScope({ ...config, endpoint_url: "https://another.example/v1" }, "stt")).not.toBe(scope);
  expect(modelKeyScope({ ...config, api_key: "new-secret" }, "stt")).toBe(scope);
  expect(modelKeyScope({ ...config, model_name: "another-model" }, "stt")).toBe(scope);
});
