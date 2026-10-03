import { expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { CloudSettings, LocalModelDownloadButton, LocalModelLanguageSettings } from "../src/model-settings";
import type { TranscriptionConfig } from "../src/types";

const model = { english_only: false, supported_languages: ["en", "fr", "de"], requires_language: true };

test("models requiring a language keep automatic, multiple and unsupported preferences until explicitly changed", () => {
  let saves = 0;
  for (const languages of [[], ["en", "fr"], ["sl"]]) {
    const html = renderToStaticMarkup(<LocalModelLanguageSettings model={model} languages={languages} save={async () => { saves++; return null; }} />);
    expect(html).toContain("Choose a language</span>");
    expect(html).toContain("Choose a language before dictating.");
    expect(html).not.toContain("English</span>");
  }
  expect(saves).toBe(0);
});

test("a supported single language is shown without the required-language warning", () => {
  const html = renderToStaticMarkup(<LocalModelLanguageSettings model={model} languages={["fr"]} save={async () => null} />);
  expect(html).toContain("French</span>");
  expect(html).not.toContain("Choose a language before dictating.");
  const error = renderToStaticMarkup(<LocalModelLanguageSettings model={model} languages={["en"]} save={async () => null} error="Could not save language" />);
  expect(error).toContain("Could not save language");
});

test("automatic-detection models warn only about unsupported configured languages", () => {
  const automatic = { ...model, requires_language: false };
  const render = (languages: string[], metadata = automatic) => renderToStaticMarkup(
    <LocalModelLanguageSettings model={metadata} languages={languages} save={async () => null} />,
  );
  expect(render([])).toBe("");
  expect(render(["en", "fr"])).toBe("");
  expect(render(["en", "sl"])).toContain("does not support all your dictation languages");
  expect(render(["fr"], { ...automatic, english_only: true })).toContain("only understands English");
  expect(renderToStaticMarkup(<LocalModelLanguageSettings model={{ ...automatic, supported_languages: null }} languages={["sl"]} save={async () => null} />)).toBe("");
});

test("Microsoft settings show the editable resource, preset timing and deployment without model discovery", () => {
  const config = {
    stt_provider: "microsoft", endpoint_url: "https://my-resource.services.ai.azure.com", api_key: "azure-key",
    stt_deployment: "my-transcribe-deployment", model_name: "mai-transcribe-2",
    llm_provider: "openai", llm_endpoint_url: "https://api.openai.com/v1/chat/completions", llm_api_key: "chat-key", llm_model_name: "gpt-5.4-mini",
  } as TranscriptionConfig;
  const render = (model_name: string) => renderToStaticMarkup(
    <CloudSettings kind="stt" config={{ ...config, model_name }} save={async () => null} errorNote={() => null} />,
  );
  const batch = render("mai-transcribe-2");
  expect(batch).toContain("Microsoft Azure");
  expect(batch).toContain("Azure resource URL");
  expect(batch).toContain('value="https://my-resource.services.ai.azure.com"');
  expect(batch).not.toContain("/audio/transcriptions");
  expect(batch).not.toContain("OpenAI-compatible endpoint");
  expect(batch).toContain("MAI Transcribe 2 · After recording");
  expect(batch).toContain("Microsoft MAI transcription is in public preview.");
  expect(batch).toContain("Real-time mode applies replacements locally.");
  expect(batch).not.toContain("Deployment name");
  expect(batch).not.toContain("Loading models");
  const live = render("mai-transcribe-2-streaming");
  expect(live).toContain("MAI Transcribe 2 · Real time");
  expect(live).toContain("Streams while you speak; pastes when you stop.");
  expect(live).toContain("Deployment name");
  expect(live).toContain('value="my-transcribe-deployment"');
  expect(live).not.toContain("Loading models");
  const llm = renderToStaticMarkup(<CloudSettings kind="llm" config={config} save={async () => null} errorNote={() => null} />);
  expect(llm).not.toContain("Microsoft Azure");
  expect(llm).not.toContain("Azure resource URL");
  expect(llm).not.toContain("Deployment name");
});

test("unavailable local runtimes disable downloads and cannot invoke stale download callbacks", () => {
  let downloads = 0;
  const make = (runtime_supported?: boolean, failed = false) => LocalModelDownloadButton({ model: { runtime_supported }, failed, onDownload: () => { downloads++; } });
  for (const failed of [false, true]) {
    const unsupported = make(false, failed);
    expect(renderToStaticMarkup(unsupported)).toContain('disabled=""');
    unsupported.props.onClick();
    expect(downloads).toBe(0);
  }
  for (const flag of [true, undefined]) {
    const supported = make(flag);
    expect(renderToStaticMarkup(supported)).not.toContain('disabled=""');
    supported.props.onClick();
  }
  expect(downloads).toBe(2);
});

test("incomplete Azure settings remain editable without discovery or a false verified key marker", () => {
  const empty = { stt_provider: "microsoft", endpoint_url: "", api_key: "", stt_deployment: "", model_name: "" } as TranscriptionConfig;
  const render = (config: TranscriptionConfig) => renderToStaticMarkup(
    <CloudSettings kind="stt" config={config} save={async () => null} errorNote={() => null} />,
  );
  const first = render(empty);
  expect(first).toContain("Microsoft Azure");
  expect(first).toContain("Azure resource URL");
  expect(first).toContain('placeholder="https://your-resource.services.ai.azure.com"');
  expect(first).toContain('type="password"');
  expect(first).toContain("Choose a model");
  expect(first).not.toContain("Loading models");
  expect(first).not.toContain("API key verified");
  const live = render({ ...empty, endpoint_url: "https://speech.cognitiveservices.azure.com", model_name: "mai-transcribe-2-streaming" });
  expect(live).toContain("Microsoft Azure");
  expect(live).toContain("Deployment name");
  expect(live).toContain('placeholder="MAI-Transcribe-2-Streaming"');
  expect(live).not.toContain("Loading models");
  expect(live).not.toContain("API key verified");
});
