import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const dir = mkdtempSync(join(tmpdir(), "rvow-lease-"));
const bin = join(dir, "rvow");
writeFileSync(
  bin,
  `#!/bin/sh
case "$3" in
  lease-bind) cat > "$RVOW_FAKE/bind.in"; cat "$RVOW_FAKE/bind.json" ;;
  pre-tool-use) cat > "$RVOW_FAKE/pre.in" ;;
  lease-end) cat > "$RVOW_FAKE/end.in" ;;
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
  const notes: { text: string; level: unknown }[] = [];
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
    ui: {
      notify: (text: string, level: unknown) => void notes.push({ text, level }),
      setStatus: () => {},
      setWidget: () => {},
    },
  };
  const fire = (name: string, event: unknown = {}) => handlers.get(name)?.(event, ctx);
  const answer = (body: object) => writeFileSync(join(dir, "bind.json"), JSON.stringify(body));
  const sent = (file: string) => JSON.parse(readFileSync(join(dir, file), "utf8"));
  const prompt = (id: string) => fire("before_agent_start", { prompt: `lease: ${id}\nGo.` });
  const edit = () => fire("tool_call", { toolName: "bash", input: { command: "ls" } });
  return { fire, notes, answer, sent, prompt, edit };
}

test("a later lease line binds the next lease when the hook says bound", async () => {
  const t = setup();
  t.answer({ bound: true, task: "TSK-aaaaaa" });
  await t.prompt("TSK-aaaaaa");
  await t.edit();
  assert.equal(t.sent("pre.in").agent_id, "pi-sess");

  t.answer({ bound: true, task: "TSK-bbbbbb" });
  await t.prompt("TSK-bbbbbb");
  assert.match(t.sent("bind.in").prompt, /^lease: TSK-bbbbbb/);
  assert.equal(t.sent("bind.in").agent_id, "pi-sess");
  await t.edit();
  assert.equal(t.sent("pre.in").agent_id, "pi-sess");
  assert.deepEqual(t.notes, []);

  await t.fire("session_shutdown");
  assert.equal(t.sent("end.in").agent_id, "pi-sess", "the lease ends with the session");
});

test("a refused bind with a reason tells the user and keeps the old binding", async () => {
  const t = setup();
  t.answer({ bound: true, task: "TSK-aaaaaa" });
  await t.prompt("TSK-aaaaaa");

  const reason =
    "this session still holds the lease of TSK-aaaaaa; prove or block TSK-aaaaaa, or start a new session for TSK-bbbbbb";
  t.answer({ bound: false, task: "TSK-bbbbbb", reason });
  await t.prompt("TSK-bbbbbb");
  assert.deepEqual(t.notes, [{ text: reason, level: "warning" }]);
  await t.edit();
  assert.equal(t.sent("pre.in").agent_id, "pi-sess", "still bound to the first lease");

  await t.fire("session_shutdown");
  assert.equal(t.sent("end.in").agent_id, "pi-sess");
});
