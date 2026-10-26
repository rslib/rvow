import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const dir = mkdtempSync(join(tmpdir(), "rvow-unseen-"));
after(() => rmSync(dir, { recursive: true, force: true }));
const bin = join(dir, "rvow");
writeFileSync(
  bin,
  `#!/bin/sh
case "$1 $2 $3" in
  "hook claude pre-tool-use") cat > "$RVOW_FAKE/pre.in"; exit 0 ;;
  "hook claude view") cat > /dev/null; cat "$RVOW_FAKE/view.json"; exit 0 ;;
esac
cat > /dev/null
case "$1" in
  review) echo "review text" ;;
  accept) echo "accepted" ;;
  yolo) printf 'tier: %s\\nminutes: 30\\n' "$2" ;;
esac
`,
);
chmodSync(bin, 0o755);
process.env.RVOW_BIN = bin;
process.env.RVOW_FAKE = dir;
writeFileSync(join(dir, "view.json"), JSON.stringify({ changes: [] }));
const loaded = Date.now();
const { default: extension } = await import("./rvow.ts");

type Handler = (event: unknown, ctx: unknown) => unknown;
type Command = { handler: (args: string, ctx: unknown) => Promise<void> };
type Tool = {
  name: string;
  execute: (
    id: string,
    params: unknown,
    signal: undefined,
    update: undefined,
    ctx: unknown,
  ) => Promise<unknown>;
};

function setup() {
  const handlers = new Map<string, Handler>();
  const commands = new Map<string, Command>();
  const tools = new Map<string, Tool>();
  const notes: string[] = [];
  extension({
    on: (name: string, h: Handler) => handlers.set(name, h),
    registerCommand: (name: string, c: Command) => commands.set(name, c),
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
      confirm: async () => true,
      notify: (text: string) => void notes.push(text),
      setStatus: () => {},
      setWidget: () => {},
    },
  };
  const fire = (name: string, event: unknown = {}) => handlers.get(name)?.(event, ctx);
  // The agent's tool call: the hook sees it, and its stdin is what the hook was sent.
  const call = async (toolName: string, input: object) => {
    await fire("tool_call", { toolName, input, toolCallId: "t1" });
    return JSON.parse(readFileSync(join(dir, "pre.in"), "utf8")) as {
      user_made: {
        since: number;
        changes: string[];
        yolo: { tier: string; at: number; until: number }[];
      };
    };
  };
  const command = (name: string, args: string) => commands.get(name)?.handler(args, ctx);
  const tool = (name: string, params: object) =>
    tools.get(name)?.execute("id", params, undefined, undefined, ctx);
  return { fire, call, command, tool, notes };
}

const probe = { command: "ls" };

test("every pre-tool-use call carries user_made with the load time", async () => {
  const t = setup();
  await t.fire("session_start");
  const sent = await t.call("bash", probe);
  assert.ok(sent.user_made.since >= loaded);
  assert.ok(sent.user_made.since <= Date.now());
  assert.deepEqual(sent.user_made.changes, []);
  assert.deepEqual(sent.user_made.yolo, []);
});

test("a change the user accepts in a dialog is in the record", async () => {
  const t = setup();
  await t.fire("session_start");
  await t.command("rvow-accept", "CHG-aaa");
  assert.deepEqual((await t.call("bash", probe)).user_made.changes, ["CHG-aaa"]);
  await t.tool("rvow_accept", { change: "CHG-bbb" });
  assert.deepEqual((await t.call("bash", probe)).user_made.changes, ["CHG-aaa", "CHG-bbb"]);
});

test("the accept command on an accepted change records it and says so", async () => {
  writeFileSync(join(dir, "view.json"), JSON.stringify({ changes: [{ id: "CHG-old" }] }));
  const t = setup();
  await t.fire("session_start");
  await t.command("rvow-accept", "CHG-old");
  writeFileSync(join(dir, "view.json"), JSON.stringify({ changes: [] }));
  assert.deepEqual((await t.call("bash", probe)).user_made.changes, ["CHG-old"]);
  assert.ok(t.notes.some((n) => n.includes("CHG-old") && n.includes("recorded")));
});

test("a yolo tier the user turns on is in the record", async () => {
  const t = setup();
  await t.fire("session_start");
  const before = Date.now();
  await t.command("rvow-yolo", "auto 30m");
  await t.tool("rvow_yolo", { tier: "bypass" });
  const [cmd, dialog] = (await t.call("bash", probe)).user_made.yolo;
  assert.equal(cmd?.tier, "auto");
  assert.ok((cmd?.at ?? 0) >= before);
  assert.equal((cmd?.until ?? 0) - (cmd?.at ?? 0), 30 * 60_000);
  assert.equal(dialog?.tier, "bypass");
  assert.equal((dialog?.until ?? 0) - (dialog?.at ?? 0), 2 * 3_600_000);
});

test("an agent that runs rvow accept in a shell adds nothing", async () => {
  const t = setup();
  await t.fire("session_start");
  await t.call("bash", { command: "rvow accept CHG-zzz --yes" });
  await t.call("bash", { command: "rvow yolo auto" });
  const sent = await t.call("bash", probe);
  assert.deepEqual(sent.user_made.changes, []);
  assert.deepEqual(sent.user_made.yolo, []);
});
