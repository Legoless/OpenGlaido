// bun test scripts/providers.test.ts
import { expect, test } from "bun:test";
import { BACKUP_ROLES, MODEL_FIELDS, PROVIDERS, apiKeyCheckStatus, applyConfigPatch, baseFromUrl, editedModelKeys, endpointUrl, hasLivePrimary, isCloudLiveModel, modelKeyScope, providerModelIds, providerPreset, providerSelection, scopedConfigPatch } from "../src/providers";
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

test("backup provider switches choose live models and isolate every slot's key and deployment", () => {
  const config = {
    stt_provider: "openai", endpoint_url: endpointUrl("https://api.openai.com/v1", "stt"), api_key: "primary-key", model_name: "gpt-live-transcribe",
    stt_backup_1_enabled: true, stt_backup_1_provider: "elevenlabs", stt_backup_1_endpoint_url: endpointUrl("https://api.elevenlabs.io/v1", "stt"),
    stt_backup_1_model_name: "scribe_v2_realtime", stt_backup_1_api_key: "second-key", stt_backup_1_deployment: "",
    stt_backup_2_enabled: true, stt_backup_2_provider: "openai", stt_backup_2_endpoint_url: endpointUrl("https://api.openai.com/v1", "stt"),
    stt_backup_2_model_name: "gpt-live-transcribe", stt_backup_2_api_key: "third-key", stt_backup_2_deployment: "",
    stt_backup_3_enabled: true, stt_backup_3_provider: "elevenlabs", stt_backup_3_endpoint_url: endpointUrl("https://api.elevenlabs.io/v1", "stt"),
    stt_backup_3_model_name: "scribe_v2_realtime", stt_backup_3_api_key: "fourth-key", stt_backup_3_deployment: "",
    stt_backup_4_enabled: true, stt_backup_4_provider: "openai", stt_backup_4_endpoint_url: endpointUrl("https://api.openai.com/v1", "stt"),
    stt_backup_4_model_name: "gpt-live-transcribe", stt_backup_4_api_key: "fifth-key", stt_backup_4_deployment: "",
  } as TranscriptionConfig;
  for (const role of BACKUP_ROLES) {
    const f = MODEL_FIELDS[role];
    for (const provider of ["openai", "elevenlabs", "microsoft"]) {
      const before = { ...config, [f.provider]: "custom" };
      const patch = providerSelection(before, role, provider)!;
      const after = { ...before, ...patch };
      expect(isCloudLiveModel(after[f.model])).toBe(true);
      expect(after[f.key]).toBe("");
      expect(after.api_key).toBe("primary-key");
      for (const otherRole of BACKUP_ROLES.filter((other) => other !== role)) {
        const other = MODEL_FIELDS[otherRole];
        expect(after[other.key]).toBe(before[other.key]);
      }
      expect(editedModelKeys(patch)).toEqual([]);
      if (provider === "microsoft") {
        expect(patch[f.deployment]).toBe("");
        expect(patch[f.url]).toBe("");
      }
    }
    expect(providerSelection(config, role, config[f.provider])).toBeNull();
  }
});

test("backup key drafts and queued key edits remain attached to their provider and slot", () => {
  const config = {
    stt_provider: "openai", endpoint_url: "https://api.openai.com/v1/audio/transcriptions",
    stt_backup_1_provider: "openai", stt_backup_1_endpoint_url: "https://api.openai.com/v1/audio/transcriptions", stt_backup_1_api_key: "second-key",
    stt_backup_2_provider: "openai", stt_backup_2_endpoint_url: "https://api.openai.com/v1/audio/transcriptions", stt_backup_2_api_key: "third-key",
    stt_backup_3_provider: "openai", stt_backup_3_endpoint_url: "https://api.openai.com/v1/audio/transcriptions", stt_backup_3_api_key: "fourth-key",
    stt_backup_4_provider: "openai", stt_backup_4_endpoint_url: "https://api.openai.com/v1/audio/transcriptions", stt_backup_4_api_key: "fifth-key",
  } as TranscriptionConfig;
  const scopes = (["stt", ...BACKUP_ROLES] as const).map((role) => modelKeyScope(config, role));
  expect(new Set(scopes).size).toBe(5);
  for (const role of BACKUP_ROLES) {
    const f = MODEL_FIELDS[role];
    for (const key of ["new-secret", ""]) {
      const patch = { [f.key]: key };
      expect(editedModelKeys(patch)).toEqual([f.key]);
      expect(applyConfigPatch(config, scopedConfigPatch(config, patch))?.[f.key]).toBe(key);
      expect(applyConfigPatch({ ...config, [f.url]: "https://other.example/v1/audio/transcriptions" }, scopedConfigPatch(config, patch))).toBeNull();
      expect(applyConfigPatch({ ...config, [f.provider]: "custom" }, scopedConfigPatch(config, patch))).toBeNull();
    }
    expect(editedModelKeys({ [f.url]: "https://other.example/v1", [f.key]: "" })).toEqual([]);
    expect(editedModelKeys({ [f.enabled]: false })).toEqual([]);
    expect(modelKeyScope({ ...config, [f.model]: "gpt-live-transcribe-2026-06-01", [f.deployment]: "new-deployment" }, role)).toBe(modelKeyScope(config, role));
  }
});

test("backup availability follows live primary models across cloud and local transcription", () => {
  for (const model_name of ["gpt-live-transcribe", "gpt-live-transcribe-2026-06-01", "scribe_v2_realtime", "mai-transcribe-2-streaming"]) {
    expect(hasLivePrimary({ stt_source: "cloud", model_name } as TranscriptionConfig)).toBe(true);
  }
  for (const model_name of ["gpt-transcribe", "scribe_v2", "mai-transcribe-2", "gpt-live-transcribe-invalid", "whisper-1"]) {
    expect(hasLivePrimary({ stt_source: "cloud", model_name } as TranscriptionConfig)).toBe(false);
  }
  expect(hasLivePrimary({ stt_source: "local", local_stt_model: "vibevoice-asr-streaming-7b" } as TranscriptionConfig)).toBe(true);
  expect(hasLivePrimary({ stt_source: "local", local_stt_model: "whisper-tiny", model_name: "gpt-live-transcribe" } as TranscriptionConfig)).toBe(false);
});
