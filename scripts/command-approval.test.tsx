import { expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { ApprovalCard } from "../src/command-window";

test("private approval cards disable persistent consent without disabling Allow once or hiding the actual arguments", () => {
  for (const allow_always of [false, true]) {
    const html = renderToStaticMarkup(<ApprovalCard approval={{
      request_id: "test-call", server: "OpenGlaido", tool: "files_apps",
      summary: '{"action":"read","path":"~/Documents/example.txt"}', expires_in_s: 60, allow_always,
    }} />);
    const button = (label: string) => html.match(new RegExp(`<button([^>]*)>${label}</button>`))?.[1];
    expect(button("Always allow")).toBeDefined();
    expect(button("Always allow")!.includes("disabled")).toBe(!allow_always);
    expect(button("Allow once")).toBeDefined();
    expect(button("Allow once")!).not.toContain("disabled");
    expect(button("Deny")!).not.toContain("disabled");
    expect(html).toContain("~/Documents/example.txt");
  }
});
