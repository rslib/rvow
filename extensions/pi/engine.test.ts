// The engine contract, as the pi extension uses it: `rvow daemon --print-hello` names the socket its
// session may call, `/v1/hook` runs one hook and answers with the text `rvow hook` would print, and
// `/v1/ask` lets the host program that draws rvow questions put them to the user on its own screen.
// Every call the fake engine below does not answer must fall back to `rvow hook` and to the dialog.
import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import type { ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";

const dir = mkdtempSync(join(tmpdir(), "rvow-engine-"));
const sock = join(dir, "e.sock");
const dead = join(dir, "dead.sock");
const argv = join(dir, "argv");
const hello = join(dir, "hello.json");
const HELLO = { proto_major: 1, proto_minor: 0, build: "0.0.0", env: "abc123", ui: "pane" };
writeFileSync(argv, "");
writeFileSync(join(dir, "hello.json"), "");
process.env.RVOW_FAKE = dir;
process.env.RVOW_BIN = join(dir, "rvow");
writeFileSync(
  process.env.RVOW_BIN,
  `#!/bin/sh
echo "$*" >> "$RVOW_FAKE/argv"
case "$1" in
  daemon) cat "$RVOW_FAKE/hello.json" 2>/dev/null; echo "daemon done" >> "$RVOW_FAKE/argv" ;;
  hook)
    case "$3" in
      view) cat "$RVOW_FAKE/view.json" 2>/dev/null ;;
      status) cat "$RVOW_FAKE/status.json" 2>/dev/null ;;
      pre-tool-use) cat > "$RVOW_FAKE/pre.in"; cat "$RVOW_FAKE/pre.json" 2>/dev/null ;;
      session-start) cat > "$RVOW_FAKE/start.in" ;;
      *) cat > /dev/null ;;
    esac ;;
  review) echo "the checklist is empty" ;;
  accept) echo "accepted CHG-a1" ;;
  confirm) echo "AMD-a1 for CHG-a1 weaken T1 verify changed" ;;
esac
exit 0
`,
);
chmodSync(process.env.RVOW_BIN, 0o755);
const { default: extension } = await import("./rvow.ts");

/** The hello of one engine call: what the session sends back with every call. */
type HookCall = { harness: string; event: string; input: Record<string, unknown> };
type AskBody = {
  hello: Record<string, unknown>;
  kind: string;
  change: string;
  cwd: string;
  session: string | null;
};
/** One answer of the fake engine: the status line and the body. */
type Answer = { status: number; body: string };

const hooks: { hello: unknown; call: HookCall }[] = [];
const asks: AskBody[] = [];
/** How many calls the engine's socket reset without an answer. */
let refused = 0;
let refusal = false;
/** What the engine answers a hook call and a question; a test changes them as it needs. */
let answerHook: (call: HookCall) => Answer = () => ({
  status: 200,
  body: JSON.stringify({ reply: { output: null }, mismatch: null }),
});
let answerAsk: (ask: AskBody) => Answer = () => ({
  status: 200,
  body: JSON.stringify({ outcome: "no-host", text: "" }),
});

const engine = createServer((req, res) => {
  let body = "";
  req.setEncoding("utf8");
  req.on("data", (chunk: string) => {
    body += chunk;
  });
  req.on("end", () => {
    if (refusal) {
      refused++;
      req.socket.destroy();
      return;
    }
    const parsed = JSON.parse(body) as Record<string, unknown>;
    let answer: Answer;
    if (req.url === "/v1/ask") {
      const ask = parsed as unknown as AskBody;
      asks.push(ask);
      answer = answerAsk(ask);
    } else {
      const call = parsed.call as HookCall;
      hooks.push({ hello: parsed.hello, call });
      answer = answerHook(call);
    }
    res.writeHead(answer.status, { "content-type": "application/json" });
    res.end(answer.body);
  });
});
await new Promise<void>((resolve) => engine.listen(sock, resolve));

after(() => {
  engine.close();
  rmSync(dir, { recursive: true, force: true });
});

/** Point the next session's `rvow daemon --print-hello` at `socket`. */
function point(socket: string): void {
  writeFileSync(hello, JSON.stringify({ socket, hello: HELLO }));
}

/** The argv of every rvow process the session ran, one line each. */
const argvLines = (): string[] =>
  readFileSync(argv, "utf8")
    .split("\n")
    .filter((line) => line !== "");
const count = (line: string): number => argvLines().filter((l) => l === line).length;

async function waitFor(cond: () => boolean): Promise<void> {
  for (let i = 0; i < 500 && !cond(); i++) await new Promise((r) => setTimeout(r, 10));
  assert.ok(cond(), "the session-start chain never ran `rvow daemon --print-hello`");
}

type Handler = (event: unknown, ctx: unknown) => unknown;

/** One pi session: the handlers it registered, its tools, and the dialogs it opened. */
function setup(answer = true) {
  writeFileSync(argv, "");
  // The process path answers a pre-tool-use from the fixture a test writes, so a test starts clean.
  rmSync(join(dir, "pre.json"), { force: true });
  hooks.length = 0;
  asks.length = 0;
  refused = 0;
  refusal = false;
  point(sock);
  answerHook = () => ({
    status: 200,
    body: JSON.stringify({ reply: { output: null }, mismatch: null }),
  });
  answerAsk = () => ({ status: 200, body: JSON.stringify({ outcome: "no-host", text: "" }) });
  const handlers = new Map<string, Handler>();
  const tools = new Map<string, ToolDefinition>();
  const asked: string[] = [];
  extension({
    on: (name: string, handler: Handler) => handlers.set(name, handler),
    registerCommand: () => {},
    registerTool: (tool: ToolDefinition) => tools.set(tool.name, tool),
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
    sessionManager: { getEntries: () => [], getSessionId: () => "pi-session-1" },
    ui: {
      notify: () => {},
      setStatus: () => {},
      setWidget: () => {},
      confirm: async (_title: string, message: string) => {
        asked.push(message);
        return answer;
      },
    },
  };
  return {
    tools,
    asked,
    ctx,
    fire: (name: string, event: unknown) => handlers.get(name)?.(event, ctx),
    call: (name: string, input: object) => {
      const tool = tools.get(name);
      assert.ok(tool, `${name} is registered`);
      const context = ctx as unknown as Parameters<typeof tool.execute>[4];
      return tool.execute("call", input, new AbortController().signal, undefined, context);
    },
  };
}

type Session = ReturnType<typeof setup>;

/** Fire session-start and wait until the session has run `rvow daemon --print-hello` and kept the engine. */
async function booted(session: Session): Promise<void> {
  const before = argvLines().length;
  await session.fire("session_start", {});
  await waitFor(() => argvLines().slice(before).includes("daemon done"));
  // The child has exited; `run` answers the caller and the session keeps the engine a tick later.
  await new Promise((resolve) => setTimeout(resolve, 50));
}

const WRITE = { toolName: "write", input: { path: "a.ts" } };
const DENY = JSON.stringify({
  hookSpecificOutput: {
    permissionDecision: "deny",
    permissionDecisionReason: "rvow: CHG-a1 is not accepted",
  },
});
const PROCESS = JSON.stringify({
  hookSpecificOutput: {
    permissionDecision: "deny",
    permissionDecisionReason: "rvow: the process decided",
  },
});
const kindOf = (out: { details: unknown }): string => (out.details as { kind: string }).kind;

test("a pre-tool-use the engine answers denies the write and runs no rvow process", async () => {
  const t = setup();
  answerHook = () => ({
    status: 200,
    body: JSON.stringify({ reply: { output: DENY }, mismatch: null }),
  });
  await booted(t);
  const blocked = (await t.fire("tool_call", WRITE)) as { block: boolean; reason: string };
  assert.equal(blocked.block, true);
  assert.equal(blocked.reason, "rvow: CHG-a1 is not accepted");
  const call = hooks.filter((h) => h.call.event === "pre-tool-use").at(-1);
  // The engine gets the harness, the event and the input `rvow hook` reads on stdin, with cwd.
  assert.equal(call?.call.harness, "claude");
  assert.deepEqual(call?.call.input.cwd, dir);
  assert.deepEqual(call?.call.input.tool_input, { path: "a.ts", file_path: "a.ts" });
  assert.deepEqual(call?.hello, HELLO);
  assert.deepEqual(
    argvLines().filter((l) => l.startsWith("hook claude pre-tool-use")),
    [],
  );
});

test("a connection the engine refuses falls back to `rvow hook`", async () => {
  const t = setup();
  await booted(t);
  // The engine answers nothing from here on: its socket resets, as a daemon that died would.
  writeFileSync(join(dir, "pre.json"), PROCESS);
  refusal = true;
  const blocked = (await t.fire("tool_call", WRITE)) as { block: boolean; reason: string };
  assert.equal(blocked.reason, "rvow: the process decided");
  assert.equal(refused, 1);
  assert.equal(count("hook claude pre-tool-use"), 1);
  // Nothing listening on the socket at all falls back the same way, in a fresh session.
  refusal = false;
  point(dead);
  await booted(t);
  assert.equal(count("hook claude pre-tool-use"), 1);
  const again = (await t.fire("tool_call", WRITE)) as { block: boolean; reason: string };
  assert.equal(again.reason, "rvow: the process decided");
  assert.equal(count("hook claude pre-tool-use"), 2);
});

test("a mismatch makes the rest of the session run `rvow hook`", async () => {
  const t = setup();
  const mismatch = JSON.stringify({
    reply: null,
    mismatch: { proto_major: 2, proto_minor: 0, build: "9.9.9", env: "other", pid: 7 },
  });
  answerHook = (call) =>
    call.event === "pre-tool-use"
      ? { status: 200, body: mismatch }
      : { status: 200, body: JSON.stringify({ reply: { output: null }, mismatch: null }) };
  await booted(t);
  writeFileSync(join(dir, "pre.json"), PROCESS);
  for (const _ of [1, 2]) {
    const blocked = (await t.fire("tool_call", WRITE)) as { block: boolean; reason: string };
    assert.equal(blocked.reason, "rvow: the process decided");
  }
  assert.equal(hooks.filter((h) => h.call.event === "pre-tool-use").length, 1);
  assert.equal(count("hook claude pre-tool-use"), 2);
});

test("an ask the engine answers 409 puts the question back in the dialog", async () => {
  const t = setup();
  await booted(t);
  // 409 Conflict is the engine of another environment: no host drew this question.
  answerAsk = () => ({ status: 409, body: JSON.stringify(HELLO) });
  const out = await t.call("rvow_accept", { change: "CHG-a1" });
  assert.equal(t.asked.length, 1);
  assert.match(t.asked[0] ?? "", /Accept CHG-a1\?/);
  assert.equal(kindOf(out), "decision");
  // And the rest of the session runs `rvow hook`, as it does after a hook mismatch.
  assert.equal(await t.fire("tool_call", WRITE), undefined);
  assert.equal(hooks.filter((h) => h.call.event === "pre-tool-use").length, 0);
  assert.equal(count("hook claude pre-tool-use"), 1);
});

test("session-start never goes through the engine", async () => {
  const t = setup();
  await booted(t);
  assert.equal(hooks.filter((h) => h.call.event === "session-start").length, 0);
  assert.equal(count("hook claude session-start"), 1);
  // The same session still sends every other event to the engine.
  assert.equal(await t.fire("tool_call", WRITE), undefined);
  assert.equal(hooks.filter((h) => h.call.event === "pre-tool-use").length, 1);
  assert.deepEqual(
    argvLines().filter((l) => l.startsWith("hook claude pre-tool-use")),
    [],
  );
});

test("an ask no host draws keeps today's dialog, and one a host answers opens nothing", async () => {
  const t = setup();
  await booted(t);
  // No host draws rvow questions: the tool shows its own confirm and runs the accept, as always.
  let out = await t.call("rvow_accept", { change: "CHG-a1" });
  assert.equal(t.asked.length, 1);
  assert.match(t.asked[0] ?? "", /Accept CHG-a1\?/);
  assert.equal(kindOf(out), "decision");
  assert.equal(count("review CHG-a1"), 1);
  assert.equal(count("accept CHG-a1 --yes"), 1);
  // A host asked the user on its own screen: its text is the result and nothing opens here.
  answerAsk = () => ({
    status: 200,
    body: JSON.stringify({ outcome: "done", text: "CHG-a1 is accepted" }),
  });
  out = await t.call("rvow_accept", { change: "CHG-a1" });
  assert.equal(t.asked.length, 1);
  assert.equal(kindOf(out), "decision");
  assert.match(JSON.stringify(out.content), /CHG-a1 is accepted/);
  assert.equal(count("review CHG-a1"), 1);
  assert.equal(count("accept CHG-a1 --yes"), 1);
  // A decline is the host's text too, and the tool opens nothing for it either.
  answerAsk = () => ({
    status: 200,
    body: JSON.stringify({ outcome: "declined", text: "the user said no" }),
  });
  out = await t.call("rvow_accept", { change: "CHG-a1" });
  assert.equal(t.asked.length, 1);
  assert.equal(kindOf(out), "failure");
  assert.match(JSON.stringify(out.content), /the user said no/);
  // Every question carried the hello, the change and the session of the tool call.
  assert.deepEqual(asks, [
    { hello: HELLO, kind: "accept", change: "CHG-a1", cwd: dir, session: "pi-session-1" },
    { hello: HELLO, kind: "accept", change: "CHG-a1", cwd: dir, session: "pi-session-1" },
    { hello: HELLO, kind: "accept", change: "CHG-a1", cwd: dir, session: "pi-session-1" },
  ]);
});
