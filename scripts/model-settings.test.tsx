import { expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { LocalModelLanguageSettings } from "../src/model-settings";

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
