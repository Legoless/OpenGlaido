import { expect, test } from "bun:test";
import { Children, isValidElement, type ComponentProps, type ReactElement, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { BackupTranscriptionSettings, CloudSettings, LocalModelDownloadButton, LocalModelLanguageSettings } from "../src/model-settings";
import { BACKUP_ROLES, MODEL_FIELDS } from "../src/providers";
import { IconButton, OutlineButton } from "../src/ui";
import type { SaveConfig, TranscriptionConfig } from "../src/types";

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

test("live backup settings explain simultaneous audio use and preserve optional providers", () => {
  const config = {
    stt_source: "cloud", model_name: "gpt-live-transcribe",
    stt_backup_1_enabled: false, stt_backup_1_provider: "elevenlabs", stt_backup_1_endpoint_url: "https://api.elevenlabs.io/v1/speech-to-text",
    stt_backup_1_model_name: "scribe_v2_realtime", stt_backup_1_api_key: "second-key", stt_backup_1_deployment: "",
    stt_backup_2_enabled: false, stt_backup_2_provider: "openai", stt_backup_2_endpoint_url: "https://api.openai.com/v1/audio/transcriptions",
    stt_backup_2_model_name: "gpt-live-transcribe", stt_backup_2_api_key: "third-key", stt_backup_2_deployment: "",
  } as TranscriptionConfig;
  let saves = 0;
  const render = (changes: Partial<TranscriptionConfig>) => renderToStaticMarkup(
    <BackupTranscriptionSettings config={{ ...config, ...changes }} save={async () => { saves++; return null; }} errorNote={() => null} />,
  );
  const disabled = render({});
  expect(disabled).toContain("Add provider");
  expect(disabled).toContain('aria-label="1 of 5 providers"');
  expect(disabled).not.toContain("<details");
  expect(disabled).toContain("receive your audio simultaneously and may each charge");
  expect(disabled).not.toContain('value="second-key"');
  expect(disabled).not.toContain('value="third-key"');
  expect(disabled).not.toContain("Choose a real-time primary");
  const active = render({ stt_backup_1_enabled: true, stt_backup_2_enabled: true });
  expect(active).toContain("Provider 2");
  expect(active).toContain("Provider 3");
  expect(active).toMatch(/<details[^>]*name="live-transcription-providers"[^>]*open=""/);
  expect(active).toContain('title="Remove provider 2"');
  expect(active).toContain('value="second-key"');
  expect(active).toContain('value="third-key"');
  expect(active).toContain("Scribe v2 · Real time");
  expect(active).toContain("GPT Live Transcribe · Real time");
  expect(active).not.toContain("After recording");
  expect(active).not.toContain("Groq");
  expect(active).not.toContain("Mistral");
  expect(active).not.toContain("Loading models");
  const batch = render({ model_name: "gpt-transcribe", stt_backup_1_enabled: true });
  expect(batch).toContain("Choose a real-time primary transcription model to use backups.");
  expect(batch).toContain('value="second-key"');
  expect(saves).toBe(0);
});

test("backup Azure settings use their own resource, key and deployment fields", () => {
  const config = {
    stt_deployment: "primary-deployment",
    stt_backup_2_provider: "microsoft", stt_backup_2_endpoint_url: "https://backup.services.ai.azure.com",
    stt_backup_2_model_name: "mai-transcribe-2-streaming", stt_backup_2_api_key: "backup-key", stt_backup_2_deployment: "backup-deployment",
  } as TranscriptionConfig;
  const fields: string[] = [];
  const html = renderToStaticMarkup(<CloudSettings kind="stt_backup_2" config={config} save={async () => null} errorNote={(field) => { fields.push(field); return null; }} />);
  expect(html).toContain('value="https://backup.services.ai.azure.com"');
  expect(html).toContain('value="backup-key"');
  expect(html).toContain('value="backup-deployment"');
  expect(html).not.toContain("primary-deployment");
  expect(html).not.toContain("After recording");
  expect(fields).toEqual(["stt_backup_2_provider", "stt_backup_2_endpoint_url", "stt_backup_2_api_key", "stt_backup_2_model_name", "stt_backup_2_deployment"]);
});

function findElement<Props>(node: ReactNode, matches: (element: ReactElement<Props>) => boolean): ReactElement<Props> | null {
  if (!isValidElement<Props & { children?: ReactNode }>(node)) return null;
  if (matches(node)) return node;
  for (const child of Children.toArray(node.props.children)) {
    const found = findElement(child, matches);
    if (found) return found;
  }
  return null;
}

function backupAdd(config: TranscriptionConfig, save: SaveConfig) {
  const section = BackupTranscriptionSettings({ config, save, errorNote: () => null });
  const button = findElement<ComponentProps<typeof OutlineButton>>(section, (element) => element.type === OutlineButton);
  if (!button) throw new Error("Missing Add provider button");
  return OutlineButton(button.props);
}

function backupRemove(config: TranscriptionConfig, number: number, save: SaveConfig) {
  const section = BackupTranscriptionSettings({ config, save, errorNote: () => null });
  const button = findElement<ComponentProps<typeof IconButton>>(section, (element) => element.type === IconButton && element.props.title === `Remove provider ${number}`);
  if (!button) throw new Error(`Missing Remove provider ${number} button`);
  return IconButton(button.props);
}

function backupConfig() {
  return {
    stt_source: "cloud", model_name: "gpt-live-transcribe", api_key: "primary-key",
    ...Object.fromEntries(BACKUP_ROLES.flatMap((role, index) => {
      const f = MODEL_FIELDS[role];
      return [[f.enabled, false], [f.provider, "openai"], [f.url, "https://api.openai.com/v1/audio/transcriptions"],
        [f.model, "gpt-live-transcribe"], [f.key, `key-${index + 2}`], [f.deployment, `deployment-${index + 2}`]];
    })),
  } as TranscriptionConfig;
}

test("Add provider fills four optional slots, opens their settings and caps the total at five", async () => {
  const original = backupConfig();
  let current = { ...original };
  const patches: Partial<TranscriptionConfig>[] = [];
  const save: SaveConfig = async (patch) => { patches.push(patch); current = { ...current, ...patch }; return null; };
  const stale = backupAdd(current, save).props.onClick!;
  await stale();
  await stale();
  expect(patches).toEqual([{ stt_backup_1_enabled: true }]);
  for (const role of BACKUP_ROLES.slice(1)) {
    await backupAdd(current, save).props.onClick!();
    expect(patches.at(-1)).toEqual({ [MODEL_FIELDS[role].enabled]: true });
  }
  expect(current).toEqual({ ...original, stt_backup_1_enabled: true, stt_backup_2_enabled: true, stt_backup_3_enabled: true, stt_backup_4_enabled: true });
  const full = backupAdd(current, save);
  expect(full.props.disabled).toBe(true);
  await full.props.onClick!();
  expect(patches).toHaveLength(4);
  const html = renderToStaticMarkup(<BackupTranscriptionSettings config={current} save={save} errorNote={() => null} />);
  expect(html).toContain('aria-label="5 of 5 providers"');
  expect(html.match(/<details[^>]*name="live-transcription-providers"[^>]*open=""/g)).toHaveLength(4);
  expect(html).toContain("Provider 5");
});

test("Remove provider preserves every slot's settings and Add reuses the first unused slot", async () => {
  const original = { ...backupConfig(), stt_backup_1_enabled: true, stt_backup_2_enabled: true, stt_backup_3_enabled: true, stt_backup_4_enabled: true };
  let current = { ...original };
  const patches: Partial<TranscriptionConfig>[] = [];
  const save: SaveConfig = async (patch) => { patches.push(patch); current = { ...current, ...patch }; return null; };
  await backupRemove(current, 3, save).props.onClick();
  expect(patches).toEqual([{ stt_backup_2_enabled: false }]);
  expect(current).toEqual({ ...original, stt_backup_2_enabled: false });
  const html = renderToStaticMarkup(<BackupTranscriptionSettings config={current} save={save} errorNote={() => null} />);
  expect(html).toContain('aria-label="4 of 5 providers"');
  expect(html).not.toContain('value="key-3"');
  expect(html.indexOf('value="key-2"')).toBeLessThan(html.indexOf('value="key-4"'));
  expect(html.indexOf('value="key-4"')).toBeLessThan(html.indexOf('value="key-5"'));
  await backupAdd(current, save).props.onClick!();
  expect(patches.at(-1)).toEqual({ stt_backup_2_enabled: true });
  expect(current).toEqual(original);
});

test("each provider card removes only its own enabled field", async () => {
  const original = backupConfig();
  for (const role of BACKUP_ROLES) {
    const field = MODEL_FIELDS[role].enabled;
    let current = { ...original, [field]: true };
    const patches: Partial<TranscriptionConfig>[] = [];
    const save: SaveConfig = async (patch) => { patches.push(patch); current = { ...current, ...patch }; return null; };
    await backupRemove(current, 2, save).props.onClick();
    expect(patches).toEqual([{ [field]: false }]);
    expect(current).toEqual(original);
  }
});

test("adding a reused middle slot opens that provider's card after rendering", async () => {
  const documentDescriptor = Object.getOwnPropertyDescriptor(globalThis, "document");
  const frameDescriptor = Object.getOwnPropertyDescriptor(globalThis, "requestAnimationFrame");
  const card = { tagName: "DETAILS", open: false };
  const frames: (() => void)[] = [];
  const lookups: string[] = [];
  Object.defineProperty(globalThis, "document", { configurable: true, value: { getElementById: (id: string) => { lookups.push(id); return card; } } });
  Object.defineProperty(globalThis, "requestAnimationFrame", { configurable: true, value: (callback: () => void) => { frames.push(callback); return frames.length; } });
  try {
    const config = { ...backupConfig(), stt_backup_1_enabled: true, stt_backup_3_enabled: true, stt_backup_4_enabled: true };
    await backupAdd(config, async () => null).props.onClick!();
    expect(card.open).toBe(false);
    expect(frames).toHaveLength(1);
    frames[0]();
    expect(card.open).toBe(true);
    expect(lookups).toEqual(["live-transcription-stt_backup_2"]);
  } finally {
    if (documentDescriptor) Object.defineProperty(globalThis, "document", documentDescriptor);
    else delete (globalThis as { document?: unknown }).document;
    if (frameDescriptor) Object.defineProperty(globalThis, "requestAnimationFrame", frameDescriptor);
    else delete (globalThis as { requestAnimationFrame?: unknown }).requestAnimationFrame;
  }
});
