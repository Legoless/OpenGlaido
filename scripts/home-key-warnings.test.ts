import { expect, test } from "bun:test";
import { homeKeyChecks, homeKeyWarnings, type HomeKeyResult } from "../src/home-key-warnings";
import type { TranscriptionConfig } from "../src/types";

const config = {
  stt_source: "cloud", stt_provider: "openai", endpoint_url: "https://api.openai.com/v1/audio/transcriptions", api_key: "speech-key",
  llm_source: "cloud", llm_provider: "openrouter", llm_endpoint_url: "https://openrouter.ai/api/v1/chat/completions", llm_api_key: "chat-key",
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
