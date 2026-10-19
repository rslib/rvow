import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import type { ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";

const dir = mkdtempSync(join(tmpdir(), "rvow-confirm-ask-"));
const bin = join(dir, "rvow");
const LIST =
  "AMD-a1 for CHG-a\n  weaken: TSK-1 verify changed\n  reason (from the agent): one\n" +
  "AMD-b2 for CHG-a\n  weaken: TSK-2 verify changed\n  reason (from the agent): two\n" +
  "digest def456\n";
writeFileSync(join(dir, "list.txt"), LIST);
writeFileSync(
  bin,
  `#!/bin/sh
echo "$*" >> "$RVOW_FAKE/argv.log"
case "$1" in
  confirm) if [ "$#" -eq 1 ] || [ "$2" = "--full" ]; then cat "$RVOW_FAKE/list.txt"; else echo "$2: decided"; fi ;;
  clarify) echo '{"questions":[],"remaining":0}' ;;
  *) ;;
esac
`,
);
chmodSync(bin, 0o755);
process.env.RVOW_BIN = bin;
process.env.RVOW_FAKE = dir;
const { default: extension } = await import("./rvow.ts");

const tools = new Map<string, ToolDefinition>();
const handlers = new Map<string, (event: unknown, ctx: unknown) => unknown>();
extension({
  on: (name: string, h: (event: unknown, ctx: unknown) => unknown) => handlers.set(name, h),
  registerCommand: () => {},
  registerTool: (value: ToolDefinition) => tools.set(value.name, value),
  registerShortcut: () => {},
  sendMessage: () => {},
  sendUserMessage: () => {},
  appendEntry: () => {},
} as unknown as ExtensionAPI);

await handlers.get("session_start")?.(
  {},
  {
    cwd: dir,
    hasUI: true,
    mode: "print",
    sessionManager: { getEntries: () => [], getSessionId: () => "pi-sess" },
    ui: { notify: () => {}, setStatus: () => {}, setWidget: () => {} },
  },
);

const argv = () => {
  try {
    return readFileSync(join(dir, "argv.log"), "utf8")
      .split("\n")
      .filter((l) => /^(confirm|clarify)/.test(l));
  } catch {
    return [];
  }
};
const clear = () => writeFileSync(join(dir, "argv.log"), "");

function call(name: string, input: object, answer = true, asked: string[] = []) {
  const value = tools.get(name);
  assert.ok(value, name);
  const ctx = {
    cwd: dir,
    hasUI: true,
    ui: {
      confirm: async (_title: string, message: string) => {
        asked.push(message);
        return answer;
      },
    },
  };
  return value.execute(
    "call",
    input,
    new AbortController().signal,
    undefined,
    ctx as unknown as Parameters<typeof value.execute>[4],
  );
}

test("confirm with an id asks only about that amendment and decides it by id", async () => {
  clear();
  const asked: string[] = [];
  const out = await call("rvow_confirm", { decision: "reject", id: "AMD-b2" }, true, asked);
  assert.equal(asked.length, 1);
  assert.match(asked[0] ?? "", /AMD-b2/);
  assert.doesNotMatch(asked[0] ?? "", /AMD-a1/);
  assert.deepEqual(argv(), ["confirm --full", "confirm AMD-b2 --reject"]);
  assert.match(JSON.stringify(out.content), /AMD-b2: decided/);
});

test("confirm with an id the user declines decides nothing", async () => {
  clear();
  const out = await call("rvow_confirm", { decision: "accept", id: "AMD-a1" }, false);
  assert.deepEqual(argv(), ["confirm --full"]);
  assert.match(JSON.stringify(out.content), /AMD-a1, so it is still pending/);
});

test("confirm with an id that is not pending says so and asks nothing", async () => {
  clear();
  const asked: string[] = [];
  const out = await call("rvow_confirm", { decision: "accept", id: "AMD-zz" }, true, asked);
  assert.deepEqual(asked, []);
  assert.match(JSON.stringify(out.content), /AMD-zz is not pending/);
});

test("confirm without an id keeps the digest path", async () => {
  clear();
  const asked: string[] = [];
  await call("rvow_confirm", { decision: "accept" }, true, asked);
  assert.match(asked[0] ?? "", /Accept all of these\?/);
  assert.deepEqual(argv(), ["confirm --full", "confirm --accept --all def456"]);
});

test("confirm refuses an id that is not an AMD id", async () => {
  await assert.rejects(call("rvow_confirm", { decision: "accept", id: "x; rm" }), /AMD id/);
});

test("ask with a change runs clarify with that id", async () => {
  clear();
  await call("rvow_ask", { change: "CHG-b" });
  assert.deepEqual(argv(), ["clarify CHG-b --json"]);
});

test("ask without a change runs clarify as before", async () => {
  clear();
  await call("rvow_ask", {});
  assert.deepEqual(argv(), ["clarify --json"]);
});

test("confirm with no input runs the plain short listing", async () => {
  clear();
  await call("rvow_confirm", {}, true);
  assert.deepEqual(argv(), ["confirm"]);
});

test("confirm with an id and no decision reads only that amendment", async () => {
  clear();
  await call("rvow_confirm", { id: "AMD-b2" }, true);
  assert.deepEqual(argv(), ["confirm AMD-b2"]);
});
