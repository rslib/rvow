import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const dir = mkdtempSync(join(tmpdir(), "rvow-readpost-"));
const bin = join(dir, "rvow");
writeFileSync(
  bin,
  `#!/bin/sh
cat > /dev/null
echo "$3" >> "$RVOW_FAKE/events"
`,
);
chmodSync(bin, 0o755);
process.env.RVOW_BIN = bin;
process.env.RVOW_FAKE = dir;
const { default: extension } = await import("./rvow.ts");

type Handler = (event: unknown, ctx: unknown) => unknown;

function setup() {
  const handlers = new Map<string, Handler>();
  extension({
    on: (name: string, h: Handler) => handlers.set(name, h),
    registerCommand: () => {},
    registerTool: () => {},
    registerShortcut: () => {},
    sendMessage: () => {},
    sendUserMessage: () => {},
    appendEntry: () => {},
  } as unknown as ExtensionAPI);
  const ctx = {
    cwd: dir,
    hasUI: true,
    mode: "print",
    sessionManager: { getEntries: () => [], getSessionId: () => "pi-sess" },
    ui: { notify: () => {}, setStatus: () => {}, setWidget: () => {} },
  };
  const fire = (name: string, event: unknown = {}) => handlers.get(name)?.(event, ctx);
  const posts = () =>
    readFileSync(join(dir, "events"), "utf8")
      .split("\n")
      .filter((e) => e === "post-tool-use").length;
  return { fire, posts };
}

test("a read runs no post-tool-use hook, and an edit does", async () => {
  const t = setup();
  for (const toolName of ["read", "grep", "find", "ls"]) {
    await t.fire("tool_call", { toolName, toolCallId: `r-${toolName}`, input: { path: "a.ts" } });
    await t.fire("tool_result", { toolName, toolCallId: `r-${toolName}`, input: { path: "a.ts" } });
  }
  assert.equal(t.posts(), 0);
  await t.fire("tool_call", { toolName: "edit", toolCallId: "e1", input: { path: "a.ts" } });
  await t.fire("tool_result", { toolName: "edit", toolCallId: "e1", input: { path: "a.ts" } });
  assert.equal(t.posts(), 1);
});
