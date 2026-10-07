import { expect, test } from "bun:test";
import { homeKeyChecks, homeKeyWarnings, type HomeKeyResult } from "../src/home-key-warnings";
import type { TranscriptionConfig } from "../src/types";

const config = {
  stt_source: "cloud", stt_provider: "openai", endpoint_url: "https://api.openai.com/v1/audio/transcriptions", model_name: "gpt-transcribe", api_key: "speech-key",
  llm_source: "cloud", llm_provider: "openrouter", llm_endpoint_url: "https://openrouter.ai/api/v1/chat/completions", llm_api_key: "chat-key",
} as TranscriptionConfig;

const liveConfig = {
  ...config, model_name: "gpt-live-transcribe",
  stt_backup_1_enabled: true, stt_backup_1_provider: "elevenlabs", stt_backup_1_endpoint_url: "https://api.elevenlabs.io/v1/speech-to-text",
  stt_backup_1_model_name: "scribe_v2_realtime", stt_backup_1_api_key: "backup-key",
  stt_backup_2_enabled: true, stt_backup_2_provider: "openai", stt_backup_2_endpoint_url: "https://api.openai.com/v1/audio/transcriptions",
  stt_backup_2_model_name: "gpt-live-transcribe", stt_backup_2_api_key: "third-key",
} as TranscriptionConfig;

const fiveLiveConfig = {
  ...liveConfig,
  stt_backup_3_enabled: true, stt_backup_3_provider: "elevenlabs", stt_backup_3_endpoint_url: "https://api.elevenlabs.io/v1/speech-to-text",
  stt_backup_3_model_name: "scribe_v2_realtime", stt_backup_3_api_key: "fourth-key",
  stt_backup_4_enabled: true, stt_backup_4_provider: "openai", stt_backup_4_endpoint_url: "https://api.openai.com/v1/audio/transcriptions",
  stt_backup_4_model_name: "gpt-live-transcribe", stt_backup_4_api_key: "fifth-key",
} as TranscriptionConfig;

test("Home checks only active, nonempty cloud credentials at authenticated providers", () => {
  expect(homeKeyChecks(null)).toEqual([]);
  expect(homeKeyChecks(config).map((check) => check.provider)).toEqual(["OpenAI", "OpenRouter"]);
  expect(homeKeyChecks({ ...config, stt_source: "local", llm_source: "off" })).toEqual([]);
  expect(homeKeyChecks({ ...config, api_key: " ", llm_api_key: "" })).toEqual([]);
  expect(homeKeyChecks({ ...config, endpoint_url: "https://custom.example/v1/audio/transcriptions", llm_endpoint_url: "http://localhost:11434/v1/chat/completions" })).toEqual([]);
});

test("failed credentials warn once across refreshes; successful verification resets the warning", () => {
  const checks = homeKeyChecks(config);
  const failed: HomeKeyResult[] = checks.map(({ id }) => ({ id, result: "failed" }));
  const first = homeKeyWarnings(checks, failed, new Set(), true);
  expect(first.providers).toEqual(["OpenAI", "OpenRouter"]);
  expect(homeKeyWarnings(checks, failed, first.warned, true).providers).toEqual([]);
  const verified = homeKeyWarnings(checks, [{ id: checks[0].id, result: "verified" }], first.warned, true);
  expect(homeKeyWarnings(checks, failed, verified.warned, true).providers).toEqual(["OpenAI"]);
});

test("old provider/key results never warn and changed credentials can warn again", () => {
  const old = homeKeyChecks(config);
  const checks = homeKeyChecks({ ...config, api_key: "new-key", llm_source: "off" });
  const result = homeKeyWarnings(checks, old.map(({ id }) => ({ id, result: "failed" })), new Set(old.map(({ id }) => id)), true);
  expect(result.providers).toEqual([]);
  expect(result.warned.size).toBe(0);
  expect(homeKeyWarnings(checks, [{ id: checks[0].id, result: "failed" }], result.warned, true).providers).toEqual(["OpenAI"]);
});

test("unsupported checks are neutral and dictation toasts leave key warnings available for a later refresh", () => {
  const checks = homeKeyChecks(config);
  expect(homeKeyWarnings(checks, checks.map(({ id }) => ({ id, result: "unsupported" })), new Set(), true).providers).toEqual([]);
  const failed: HomeKeyResult[] = checks.map(({ id }) => ({ id, result: "failed" }));
  const deferred = homeKeyWarnings(checks, failed, new Set(), false);
  expect(deferred.providers).toEqual([]);
  expect(deferred.warned.size).toBe(0);
  expect(homeKeyWarnings(checks, failed, deferred.warned, true).providers).toEqual(["OpenAI", "OpenRouter"]);
});

test("a provider shared by both active roles gets one warning", () => {
  const checks = homeKeyChecks({ ...config, llm_provider: "openai", llm_endpoint_url: "https://api.openai.com/v1/chat/completions", llm_api_key: config.api_key });
  const failed: HomeKeyResult[] = checks.map(({ id }) => ({ id, result: "failed" }));
  expect(homeKeyWarnings(checks, failed, new Set(), true).providers).toEqual(["OpenAI"]);
});

test("Home checks each enabled live backup and distinguishes its credentials from the primary", () => {
  const checks = homeKeyChecks(liveConfig);
  expect(checks.map(({ provider, apiKey }) => [provider, apiKey])).toEqual([
    ["OpenAI", "speech-key"], ["OpenRouter", "chat-key"], ["ElevenLabs", "backup-key"], ["OpenAI", "third-key"],
  ]);
  const shared = homeKeyChecks({ ...liveConfig, llm_source: "off", stt_backup_2_api_key: config.api_key });
  expect(shared[0].id).not.toBe(shared[2].id);
  expect(homeKeyWarnings(shared, shared.map(({ id }) => ({ id, result: "failed" })), new Set(), true).providers).toEqual(["OpenAI", "ElevenLabs"]);
});

test("backup key checks stay inactive for completed-recording models and disabled or non-live slots", () => {
  expect(homeKeyChecks({ ...liveConfig, model_name: "gpt-transcribe" }).map(({ provider }) => provider)).toEqual(["OpenAI", "OpenRouter"]);
  expect(homeKeyChecks({ ...liveConfig, stt_backup_1_enabled: false, stt_backup_2_model_name: "gpt-transcribe" }).map(({ provider }) => provider)).toEqual(["OpenAI", "OpenRouter"]);
  expect(homeKeyChecks({ ...liveConfig, stt_backup_1_api_key: " ", stt_backup_2_endpoint_url: "https://custom.example/v1/audio/transcriptions" }).map(({ provider }) => provider)).toEqual(["OpenAI", "OpenRouter"]);
  expect(homeKeyChecks({ ...liveConfig, model_name: "gpt-live-transcribe-2026-09-01" })).toHaveLength(4);
});

test("a local live primary checks its cloud backups; disabling one discards stale warnings", () => {
  const local = { ...liveConfig, stt_source: "local", local_stt_model: "vibevoice-asr-streaming-7b", llm_source: "off" } as TranscriptionConfig;
  const checks = homeKeyChecks(local);
  expect(checks.map(({ provider }) => provider)).toEqual(["ElevenLabs", "OpenAI"]);
  const failed: HomeKeyResult[] = checks.map(({ id }) => ({ id, result: "failed" }));
  const warned = homeKeyWarnings(checks, failed, new Set(), true).warned;
  const remaining = homeKeyChecks({ ...local, stt_backup_1_enabled: false });
  expect(homeKeyWarnings(remaining, failed, warned, true).warned).toEqual(new Set([checks[1].id]));
  expect(homeKeyChecks({ ...local, local_stt_model: "whisper-tiny" })).toEqual([]);
});

test("fourth and fifth provider keys are checked in priority order with independent scopes", () => {
  const checks = homeKeyChecks({ ...fiveLiveConfig, llm_source: "off" });
  expect(checks.map(({ apiKey }) => apiKey)).toEqual(["speech-key", "backup-key", "third-key", "fourth-key", "fifth-key"]);
  expect(new Set(checks.map(({ id }) => id)).size).toBe(5);
  const shared = homeKeyChecks({ ...fiveLiveConfig, llm_source: "off", stt_backup_3_api_key: "backup-key", stt_backup_4_api_key: "speech-key" });
  expect(shared[1].id).not.toBe(shared[3].id);
  expect(shared[0].id).not.toBe(shared[4].id);
  const failed: HomeKeyResult[] = shared.map(({ id }) => ({ id, result: "failed" }));
  expect(homeKeyWarnings(shared, failed, new Set(), true).providers).toEqual(["OpenAI", "ElevenLabs"]);
});

test("nonconsecutive enabled backup slots retain their exact key scopes and discard disabled warnings", () => {
  const checks = homeKeyChecks({ ...fiveLiveConfig, llm_source: "off" });
  const failed: HomeKeyResult[] = checks.map(({ id }) => ({ id, result: "failed" }));
  const warned = homeKeyWarnings(checks, failed, new Set(), true).warned;
  const remaining = homeKeyChecks({ ...fiveLiveConfig, llm_source: "off", stt_backup_1_enabled: false, stt_backup_3_enabled: false });
  expect(remaining.map(({ apiKey }) => apiKey)).toEqual(["speech-key", "third-key", "fifth-key"]);
  expect(homeKeyWarnings(remaining, failed, warned, true).warned).toEqual(new Set([checks[0].id, checks[2].id, checks[4].id]));
  expect(homeKeyChecks({ ...fiveLiveConfig, llm_source: "off", stt_backup_3_model_name: "scribe_v2", stt_backup_4_api_key: " " }).map(({ apiKey }) => apiKey)).toEqual(["speech-key", "backup-key", "third-key"]);
});

test("all four backup credential checks stay inactive for primary completed-recording models", () => {
  expect(homeKeyChecks({ ...fiveLiveConfig, model_name: "gpt-transcribe" }).map(({ apiKey }) => apiKey)).toEqual(["speech-key", "chat-key"]);
  expect(homeKeyChecks({ ...fiveLiveConfig, stt_source: "local", local_stt_model: "whisper-tiny", llm_source: "off" })).toEqual([]);
  const local = { ...fiveLiveConfig, stt_source: "local", local_stt_model: "vibevoice-asr-streaming-7b", llm_source: "off" } as TranscriptionConfig;
  expect(homeKeyChecks(local).map(({ apiKey }) => apiKey)).toEqual(["backup-key", "third-key", "fourth-key", "fifth-key"]);
});
