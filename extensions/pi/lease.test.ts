import assert from "node:assert/strict";
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const dir = mkdtempSync(join(tmpdir(), "rvow-lease-"));
after(() => rmSync(dir, { recursive: true, force: true }));
const bin = join(dir, "rvow");
writeFileSync(
  bin,
  `#!/bin/sh
case "$1" in
  prove) cat > /dev/null; echo "task: $2"; echo "result: pass"; echo "tests: 1"; echo "ms: 5"; echo "event: 0123456789abcdef"; exit 0 ;;
esac
case "$3" in
  lease-bind) cat > "$RVOW_FAKE/bind.in"; cat "$RVOW_FAKE/bind.json" ;;
  pre-tool-use) cat > "$RVOW_FAKE/pre.in"; cat "$RVOW_FAKE/pre.out" 2>/dev/null || true ;;
  lease-end) cat > "$RVOW_FAKE/end.in" ;;
  session-start) cat > "$RVOW_FAKE/brief.in" ;;
  *) cat > /dev/null ;;
esac
`,
);
chmodSync(bin, 0o755);
process.env.RVOW_BIN = bin;
process.env.RVOW_FAKE = dir;
const { default: extension } = await import("./rvow.ts");

type Handler = (event: unknown, ctx: unknown) => unknown;
type Tool = {
  name: string;
  execute: (
    id: string,
    params: unknown,
    signal: undefined,
    update: undefined,
    ctx: unknown,
  ) => unknown;
};

function setup() {
  const handlers = new Map<string, Handler>();
  const tools = new Map<string, Tool>();
  const notes: { text: string; level: unknown }[] = [];
  extension({
    on: (name: string, h: Handler) => handlers.set(name, h),
    registerCommand: () => {},
    registerTool: (t: Tool) => tools.set(t.name, t),
    getActiveTools: () => [],
    setActiveTools: () => {},
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
  const prove = (task: string) =>
    tools.get("rvow_prove")?.execute("id", { task }, undefined, undefined, ctx) as Promise<{
      content: { text: string }[];
      isError?: boolean;
    }>;
  return { fire, notes, answer, sent, prompt, edit, prove };
}

// The brief runs after the hook returns, so the fake rvow reads its input a moment later.
async function briefSent() {
  for (let i = 0; i < 200; i++) {
    // The file can exist before the fake rvow finishes writing it.
    if (existsSync(join(dir, "brief.in")))
      try {
        return JSON.parse(readFileSync(join(dir, "brief.in"), "utf8")) as Record<string, unknown>;
      } catch {}
    await new Promise((r) => setTimeout(r, 10));
  }
  throw new Error("the session-start brief never reached the fake rvow");
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

test("rvow_prove asks the hook first with the lease agent and runs only when it allows", async () => {
  const t = setup();
  await t.fire("session_start");
  t.answer({ bound: true, task: "TSK-a" });
  await t.prompt("TSK-a");
  const reason = "rvow: TSK-b is outside the lease of TSK-a";
  writeFileSync(
    join(dir, "pre.out"),
    JSON.stringify({
      hookSpecificOutput: { permissionDecision: "deny", permissionDecisionReason: reason },
    }),
  );
  const refused = await t.prove("TSK-b");
  const asked = t.sent("pre.in");
  assert.equal(asked.tool_name, "Bash");
  assert.equal(asked.tool_input.command, "rvow prove TSK-b");
  assert.equal(asked.session_id, "pi-sess");
  assert.equal(asked.agent_id, "pi-sess");
  assert.equal(refused.content[0]?.text, reason);
  assert.equal(refused.isError, true);

  writeFileSync(join(dir, "pre.out"), "");
  const ran = await t.prove("TSK-a");
  assert.match(ran.content[0]?.text ?? "", /result: pass/);
  assert.equal(ran.isError, undefined);
});

test("a session-start with no lease sends no agent id", async () => {
  const t = setup();
  await t.fire("session_start");
  const asked = await briefSent();
  assert.equal(asked.session_id, "pi-sess");
  assert.equal("agent_id" in asked, false, "no lease is bound, so the brief carries no agent id");
});

test("a session-start after a bind sends the lease agent id", async () => {
  const t = setup();
  t.answer({ bound: true, task: "TSK-aaaaaa" });
  await t.prompt("TSK-aaaaaa");
  rmSync(join(dir, "brief.in"), { force: true });
  await t.fire("session_start");
  const asked = await briefSent();
  assert.equal(asked.session_id, "pi-sess");
  assert.equal(asked.agent_id, "pi-sess");
  assert.equal(asked.source, "startup");
});

test("a compact brief after a bind sends the lease agent id", async () => {
  const t = setup();
  t.answer({ bound: true, task: "TSK-aaaaaa" });
  await t.prompt("TSK-aaaaaa");
  rmSync(join(dir, "brief.in"), { force: true });
  await t.fire("session_compact", {});
  await t.fire("context", { messages: [] });
  const asked = await briefSent();
  assert.equal(asked.agent_id, "pi-sess");
  assert.equal(asked.source, "compact");
});
