import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const dir = mkdtempSync(join(tmpdir(), "rvow-provenance-"));
const bin = join(dir, "rvow");
writeFileSync(
  bin,
  `#!/bin/sh
case "$3" in
  lease-bind) cat "$RVOW_FAKE/bind.json" ;;
  pre-tool-use) cat > "$RVOW_FAKE/pre.in" ;;
  post-tool-use) cat > "$RVOW_FAKE/post.in" ;;
  *) cat > /dev/null ;;
esac
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
  const sent = (file: string) => JSON.parse(readFileSync(join(dir, file), "utf8"));
  return { fire, sent };
}

test("a tool call reports the same call id before and after it runs", async () => {
  const t = setup();
  await t.fire("tool_call", { toolName: "bash", toolCallId: "t1", input: { command: "echo hi" } });
  const pre = t.sent("pre.in");
  assert.deepEqual(pre.tool_use_id, "t1");
  assert.deepEqual(pre.session_id, "pi-sess");
  await t.fire("tool_result", {
    toolName: "bash",
    toolCallId: "t1",
    input: { command: "echo hi" },
  });
  const post = t.sent("post.in");
  assert.deepEqual(post.tool_use_id, "t1");
  assert.deepEqual(post.session_id, "pi-sess");
});

test("a leased subagent's call id reaches post-tool-use with the agent id", async () => {
  const t = setup();
  // Bind a lease so leaseAgent is set, as a subagent session does.
  // The fake rvow answer is not needed for this path; leaseAgent is set on
  // before_agent_start only through the lease-bind answer, so set it by
  // writing the fake answer file.
  const dt = dir;
  writeFileSync(join(dt, "bind.json"), JSON.stringify({ bound: true, task: "TSK-aaaaaa" }));
  await t.fire("before_agent_start", { prompt: "lease: TSK-aaaaaa\nGo." });
  await t.fire("tool_call", { toolName: "bash", toolCallId: "t9", input: { command: "ls" } });
  await t.fire("tool_result", { toolName: "bash", toolCallId: "t9", input: { command: "ls" } });
  assert.deepEqual(t.sent("pre.in").agent_id, "pi-sess");
  assert.deepEqual(t.sent("post.in").tool_use_id, "t9");
  assert.deepEqual(t.sent("post.in").agent_id, "pi-sess");
});
