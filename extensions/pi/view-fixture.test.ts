import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { registerTypedTools, VIEW_KEYS, viewArgs } from "./rvow/typed-tools.ts";

// Captured from `rvow hook view` of a real binary in a throwaway repository with one backlog plan.
const text = readFileSync(new URL("./fixtures/hook-view.json", import.meta.url), "utf8");
const fixture: Record<string, unknown> = JSON.parse(text);

function viewTool(execute: Parameters<typeof registerTypedTools>[1]) {
  const tools = new Map<string, ToolDefinition>();
  registerTypedTools(
    { registerTool: (t: ToolDefinition) => tools.set(t.name, t) } as unknown as ExtensionAPI,
    execute,
  );
  const tool = tools.get("rvow_view");
  assert.ok(tool);
  return tool;
}

const call = (tool: ToolDefinition, input: object) =>
  tool.execute("c", input, new AbortController().signal, undefined, {
    cwd: "/tmp",
  } as Parameters<typeof tool.execute>[4]);

test("view asks rvow for a form that exists and returns the real output as data", async () => {
  assert.deepEqual(viewArgs({}), ["hook", "view"]);
  assert.deepEqual(viewArgs({ target: "status" }), ["status", "--json"]);
  const seen: string[][] = [];
  const result = await call(
    viewTool(async (args) => {
      seen.push(args);
      return { code: 0, out: text };
    }),
    {},
  );
  assert.deepEqual(seen, [["hook", "view"]]);
  assert.deepEqual(result.details, { kind: "view", code: 0, data: fixture });
  assert.deepEqual(result.structuredContent, result.details);
});

test("an empty answer from a project with nothing yet is a null view", async () => {
  const result = await call(
    viewTool(async () => ({ code: 0, out: "" })),
    {},
  );
  assert.deepEqual(result.details, { kind: "view", code: 0, data: null });
});

test("the fixture carries every key the tool reads, and only keys rvow documents", () => {
  for (const key of VIEW_KEYS) assert.ok(key in fixture, `fixture lacks ${key}`);
  // `specs` and `plans` are hook-only extras the extension does not read as view fields.
  assert.deepEqual(Object.keys(fixture).sort(), [...VIEW_KEYS, "plans", "specs"].sort());
});
