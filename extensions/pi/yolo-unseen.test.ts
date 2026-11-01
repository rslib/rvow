import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const dir = mkdtempSync(join(tmpdir(), "rvow-yolo-unseen-"));
// A deferred status refresh can still write in the folder, so the delete tries again.
after(() => rmSync(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 }));
const bin = join(dir, "rvow");
// Each hook call of a case answers with `pre.json` (or `stop.json`), or with `pre-2.json` when the
// case gave the same call's second reply one, so a case controls what each call of the hook says.
writeFileSync(
  bin,
  `#!/bin/sh
echo "$*" >> "$RVOW_FAKE/argv"
case "$1 $2 $3" in
  "hook claude pre-tool-use")
    n=$(cat "$RVOW_FAKE/pre.n" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$RVOW_FAKE/pre.n"
    cat > "$RVOW_FAKE/pre-$n.in"
    if [ -f "$RVOW_FAKE/pre-$n.json" ]; then cat "$RVOW_FAKE/pre-$n.json"; elif [ -f "$RVOW_FAKE/pre.json" ]; then cat "$RVOW_FAKE/pre.json"; fi
    exit 0 ;;
  "hook claude stop")
    n=$(cat "$RVOW_FAKE/stop.n" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$RVOW_FAKE/stop.n"
    cat > "$RVOW_FAKE/stop-$n.in"
    if [ -f "$RVOW_FAKE/stop-$n.json" ]; then cat "$RVOW_FAKE/stop-$n.json"; elif [ -f "$RVOW_FAKE/stop.json" ]; then cat "$RVOW_FAKE/stop.json"; fi
    exit 0 ;;
  "hook claude status") cat > "$RVOW_FAKE/status.in"; cat "$RVOW_FAKE/status.json" 2>/dev/null ;;
  "hook claude view") cat > /dev/null; cat "$RVOW_FAKE/view.json" 2>/dev/null ;;
  "hook claude session-start"|"hook claude post-tool-use"|"hook claude lease-bind") cat > /dev/null ;;
  "hook claude "*) cat > /dev/null ;;
  *)
    cat > /dev/null
    case "$1 $2" in
      "yolo off") printf 'tier: off\\n' ;;
    esac ;;
esac
`,
);
chmodSync(bin, 0o755);
process.env.RVOW_BIN = bin;
const { default: extension } = await import("./rvow.ts");

type Handler = (event: unknown, ctx: unknown) => unknown;
type Command = { handler: (args: string, ctx: unknown) => Promise<void> };
type Select = (title: string, options: string[]) => Promise<string | undefined>;

/** The unseen yolo field the Rust hook puts next to `hookSpecificOutput` of a held-back reply. */
const UNSEEN = { tier: "audit", at: 1_700_000_000_000, until: 1_700_003_600_000 };
const DENY = JSON.stringify({
  hookSpecificOutput: {
    permissionDecision: "deny",
    permissionDecisionReason: "rvow: no edits yet",
  },
  unseen_yolo: UNSEEN,
});
const ALLOW = JSON.stringify({ hookSpecificOutput: { permissionDecision: "allow" } });

type Ui = {
  hasUI?: boolean;
  mode?: string;
  pick?: string;
  closed?: boolean;
  confirm?: boolean;
  status?: string;
};

function setup(replies: Record<string, string>, ui: Ui = {}) {
  const home = mkdtempSync(join(dir, "case-"));
  process.env.RVOW_FAKE = home;
  for (const [name, body] of Object.entries(replies)) writeFileSync(join(home, name), body);
  writeFileSync(join(home, "view.json"), JSON.stringify({ changes: [] }));
  if (ui.status) writeFileSync(join(home, "status.json"), ui.status);
  writeFileSync(join(home, "argv"), "");
  const handlers = new Map<string, Handler>();
  const commands = new Map<string, Command>();
  const calls: { kind: string; args: unknown[] }[] = [];
  const api = {
    on: (name: string, h: Handler) => handlers.set(name, h),
    registerCommand: (name: string, c: Command) => commands.set(name, c),
    registerTool: () => {},
    getActiveTools: () => [],
    setActiveTools: () => {},
    registerShortcut: () => {},
    sendMessage: () => {},
    sendUserMessage: () => {},
    appendEntry: () => {},
  };
  extension(api as unknown as ExtensionAPI);
  const hasUI = ui.hasUI ?? true;
  const ctx = {
    cwd: home,
    hasUI,
    mode: ui.mode ?? (hasUI ? "tui" : "print"),
    sessionManager: { getEntries: () => [], getSessionId: () => "pi-sess" },
    ui: {
      notify: (...args: unknown[]) => void calls.push({ kind: "notify", args }),
      confirm: (title: string, message: string) => {
        calls.push({ kind: "confirm", args: [title, message] });
        return Promise.resolve(ui.confirm ?? false);
      },
      select: ((title: string, options: string[]) => {
        calls.push({ kind: "select", args: [title, options] });
        if (ui.closed) return Promise.resolve(undefined);
        return Promise.resolve(ui.pick ?? options[0]);
      }) as Select,
      setStatus: (...args: unknown[]) => void calls.push({ kind: "status", args }),
      setWidget: () => {},
    },
  };
  const fire = (name: string, event: unknown = {}) => handlers.get(name)?.(event, ctx);
  const asks = () => calls.filter((c) => c.kind === "select");
  const notes = () => calls.filter((c) => c.kind === "notify").map((c) => String(c.args[0]));
  const argv = () => readFileSync(join(home, "argv"), "utf8").split("\n").filter(Boolean);
  const sent = (name: string) => {
    try {
      return JSON.parse(readFileSync(join(home, name), "utf8")) as {
        user_made?: { yolo: { tier: string; at: number; until: number }[] };
      };
    } catch {
      return undefined;
    }
  };
  const command = (name: string, args = "") => commands.get(name)?.handler(args, ctx);
  return { fire, asks, notes, argv, sent, command };
}

/** Wait for `ready` to hold, so a call the extension defers has run before it is checked. */
async function until(ready: () => boolean): Promise<void> {
  for (let i = 0; i < 100 && !ready(); i++) await new Promise((r) => setTimeout(r, 10));
}

const call = (toolName: string, input: object) => ({ toolName, input, toolCallId: "t1" });
const edit = () => call("edit", { path: "src/a.rs", content: "x" });

test("a denial with no unseen_yolo field asks nothing", async () => {
  const t = setup({
    "pre.json": JSON.stringify({
      hookSpecificOutput: { permissionDecision: "deny", permissionDecisionReason: "rvow: no" },
    }),
  });
  assert.deepEqual(await t.fire("tool_call", edit()), { block: true, reason: "rvow: no" });
  assert.equal(t.asks().length, 0);
  assert.equal(t.argv().filter((l) => l.startsWith("hook claude pre-tool-use")).length, 1);
});

test("Keep is asked once, records the factor the hook sent, and the call runs again", async () => {
  const t = setup({ "pre.json": DENY, "pre-2.json": ALLOW }, { pick: "Keep" });
  assert.equal(await t.fire("tool_call", edit()), undefined);
  const [ask] = t.asks();
  assert.ok(ask, "the unseen yolo dialog opened");
  const [question, options] = ask.args as [string, string[]];
  assert.match(question, /^yolo audit is on until /);
  assert.match(question, /did not see you switch it on/);
  assert.match(question, /rvow yolo/);
  assert.deepEqual(options, ["Keep", "Turn off"]);

  // The tier the hook named is in the record of the second call, with the file's own times.
  assert.deepEqual(t.sent("pre-1.in")?.user_made?.yolo, []);
  assert.deepEqual(t.sent("pre-2.in")?.user_made?.yolo, [UNSEEN]);
  assert.equal(t.argv().filter((l) => l.startsWith("hook claude pre-tool-use")).length, 2);
  // The status line is drawn again, since the tier it shows changed. The fake logs the call
  // before it writes `status.in`, so wait for the record itself.
  await until(() => t.sent("status.in")?.user_made !== undefined);
  assert.deepEqual(t.sent("status.in")?.user_made?.yolo, [UNSEEN]);
});

test("Keep says nothing when the second reply still denies", async () => {
  const second = JSON.stringify({
    hookSpecificOutput: { permissionDecision: "deny", permissionDecisionReason: "rvow: still no" },
  });
  const t = setup({ "pre.json": DENY, "pre-2.json": second }, { pick: "Keep" });
  assert.deepEqual(await t.fire("tool_call", edit()), { block: true, reason: "rvow: still no" });
});

test("Turn off runs rvow yolo off and keeps the original denial", async () => {
  const t = setup({ "pre.json": DENY }, { pick: "Turn off" });
  assert.deepEqual(await t.fire("tool_call", edit()), {
    block: true,
    reason: "rvow: no edits yet",
  });
  assert.equal(t.argv().filter((l) => l === "yolo off").length, 1);
  assert.equal(t.argv().filter((l) => l.startsWith("hook claude pre-tool-use")).length, 1);
  assert.equal(
    t.notes().some((n) => n.includes("tier: off")),
    true,
  );
});

test("a dialog that closes with no answer denies and asks no more for that file", async () => {
  const t = setup({ "pre.json": DENY }, { closed: true });
  assert.deepEqual(await t.fire("tool_call", edit()), {
    block: true,
    reason: "rvow: no edits yet",
  });
  assert.deepEqual(await t.fire("tool_call", call("write", { path: "src/a.rs", content: "y" })), {
    block: true,
    reason: "rvow: no edits yet",
  });
  assert.equal(t.asks().length, 1, "one dialog for one yolo file");
  assert.equal(t.sent("pre-1.in")?.user_made?.yolo?.length, 0);
});

test("without a UI nothing is asked and the call stays denied", async () => {
  const t = setup({ "pre.json": DENY }, { hasUI: false });
  assert.deepEqual(await t.fire("tool_call", edit()), {
    block: true,
    reason: "rvow: no edits yet",
  });
  assert.equal(t.asks().length, 0);
  assert.deepEqual(await t.fire("tool_call", edit()), {
    block: true,
    reason: "rvow: no edits yet",
  });
  assert.equal(t.sent("pre-1.in")?.user_made?.yolo?.length, 0);
});

test("the stop carries the record and asks about an unseen tier", async () => {
  const hold = JSON.stringify({
    decision: "block",
    reason: "rvow: tasks are open",
    unseen_yolo: UNSEEN,
  });
  const open = setup({ "stop.json": hold }, { pick: "Turn off" });
  const first = await open.fire("agent_before_settle", { entries: [] });
  assert.deepEqual(open.sent("stop-1.in")?.user_made?.yolo, []);
  assert.equal(open.asks().length, 1);
  assert.equal(
    (first as { entries: { content: string }[] }).entries.at(-1)?.content,
    "rvow: tasks are open",
    "a turn off holds the stop with the first reason",
  );
  assert.equal(open.argv().filter((l) => l === "yolo off").length, 1);
});

test("Keep at the stop runs it again under the tier the user kept", async () => {
  const hold = JSON.stringify({
    decision: "block",
    reason: "rvow: tasks are open",
    unseen_yolo: UNSEEN,
  });
  const t = setup({ "stop.json": hold, "stop-2.json": "{}" }, { pick: "Keep" });
  assert.equal(await t.fire("agent_before_settle", { entries: [] }), undefined);
  assert.equal(t.asks().length, 1);
  assert.deepEqual(t.sent("stop-2.in")?.user_made?.yolo, [UNSEEN]);
});

test("the status hook gets the record", async () => {
  const t = setup(
    { "status.json": JSON.stringify({ status: "rvow CHG-1 [yolo audit unseen]" }) },
    { mode: "print" },
  );
  await t.command("rvow-status");
  assert.deepEqual(t.sent("status.in")?.user_made?.yolo, []);
});

test("Keep runs the call once more even when the reply still names the tier", async () => {
  const t = setup({ "pre.json": DENY }, { pick: "Keep" });
  assert.deepEqual(await t.fire("tool_call", edit()), {
    block: true,
    reason: "rvow: no edits yet",
  });
  assert.equal(t.asks().length, 1);
  assert.equal(t.argv().filter((l) => l.startsWith("hook claude pre-tool-use")).length, 2);
  // A later call under the kept tier runs once more without a second dialog.
  assert.deepEqual(await t.fire("tool_call", edit()), {
    block: true,
    reason: "rvow: no edits yet",
  });
  assert.equal(t.asks().length, 1);
  assert.equal(t.argv().filter((l) => l.startsWith("hook claude pre-tool-use")).length, 4);
});
