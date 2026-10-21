import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

// BIN is read at import, so set it before the module loads. The fake hangs on a hook call and
// sleeps for its first argument otherwise.
const dir = mkdtempSync(join(tmpdir(), "rvow-timeout-"));
after(() => rmSync(dir, { recursive: true, force: true }));
const bin = join(dir, "rvow");
writeFileSync(
  bin,
  `#!/bin/sh
case "$1" in
  hook) cat > /dev/null; exec sleep 30 ;;
  *) exec sleep "$1" ;;
esac
`,
);
chmodSync(bin, 0o755);
process.env.RVOW_BIN = bin;
process.env.RVOW_FAST_MS = "300";
process.env.RVOW_FOCUS_SHORTCUT = "not a chord";

test("a run that times out says so instead of returning nothing", async () => {
  const { run } = await import("./rvow/tools.ts");
  const r = await run(["5"], process.cwd(), "", 200);
  assert.equal(r?.code, 1);
  assert.match(r?.out ?? "", /rvow 5: stopped after 0 s with no result; run it in a terminal/);
});

test("a pre-tool-use hook that times out blocks a write and allows a read", {
  timeout: 5000,
}, async () => {
  const { default: extension } = await import("./rvow.ts");
  const handlers = new Map<string, (event: unknown, ctx: unknown) => unknown>();
  extension({
    on: (name: string, h: (event: unknown, ctx: unknown) => unknown) => handlers.set(name, h),
    registerCommand: () => {},
    registerTool: () => {},
    registerShortcut: () => {},
    sendMessage: () => {},
    sendUserMessage: () => {},
    appendEntry: () => {},
  } as unknown as ExtensionAPI);
  const ctx = {
    cwd: dir,
    hasUI: false,
    mode: "print",
    sessionManager: { getEntries: () => [] },
    ui: { notify: () => {}, setStatus: () => {}, setWidget: () => {} },
  };
  const fire = (name: string, event: unknown) => handlers.get(name)?.(event, ctx);
  const blocked = (await fire("tool_call", { toolName: "write", input: { path: "a.ts" } })) as {
    block: boolean;
    reason: string;
  };
  assert.equal(blocked.block, true);
  assert.match(blocked.reason, /^rvow: the hook failed, so tools that write stay denied/);
  assert.equal(await fire("tool_call", { toolName: "grep", input: { pattern: "x" } }), undefined);
});
