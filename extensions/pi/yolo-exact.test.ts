import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import type { ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";

/** A yolo switch the user made, with the times the CLI printed, which is what the hook counts. */
type Entry = { tier: string; at: number; until: number };

const dir = mkdtempSync(join(tmpdir(), "rvow-yolo-exact-"));
// A deferred status refresh can still write in the folder, so the delete tries again.
after(() => rmSync(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 }));
const bin = join(dir, "rvow");
// `yolo` prints `yolo.out` and exits with `yolo.code`, so a case controls what the CLI says. Each
// pre-tool-use call sends its input to `pre-<n>.in`, so a case reads the record the hook got.
writeFileSync(
  bin,
  `#!/bin/sh
echo "$*" >> "$RVOW_FAKE/argv"
case "$1 $2" in
  "hook claude")
    case "$3" in
      pre-tool-use)
        n=$(cat "$RVOW_FAKE/pre.n" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$RVOW_FAKE/pre.n"
        cat > "$RVOW_FAKE/pre-$n.in"
        printf '{"hookSpecificOutput":{"permissionDecision":"allow"}}\\n' ;;
      *) cat > /dev/null ;;
    esac ;;
  *)
    cat > /dev/null
    case "$1" in
      yolo)
        if [ -f "$RVOW_FAKE/yolo.out" ]; then cat "$RVOW_FAKE/yolo.out"; fi
        if [ -f "$RVOW_FAKE/yolo.code" ]; then exit "$(cat "$RVOW_FAKE/yolo.code")"; fi ;;
    esac ;;
esac
exit 0
`,
);
chmodSync(bin, 0o755);
process.env.RVOW_BIN = bin;
process.env.RVOW_FAKE = dir;
const { default: extension } = await import("./rvow.ts");

const handlers = new Map<string, (event: unknown, ctx: unknown) => unknown>();
const commands = new Map<string, { handler: (args: string, ctx: unknown) => Promise<void> }>();
const tools = new Map<string, ToolDefinition>();
extension({
  on: (name: string, h: (event: unknown, ctx: unknown) => unknown) => handlers.set(name, h),
  registerCommand: (name: string, c: { handler: (args: string, ctx: unknown) => Promise<void> }) =>
    commands.set(name, c),
  registerTool: (value: ToolDefinition) => tools.set(value.name, value),
  getActiveTools: () => [],
  setActiveTools: () => {},
  registerShortcut: () => {},
  sendMessage: () => {},
  sendUserMessage: () => {},
  appendEntry: () => {},
} as unknown as ExtensionAPI);

function ctx() {
  return {
    cwd: dir,
    hasUI: true,
    mode: "print",
    sessionManager: { getEntries: () => [], getSessionId: () => "pi-sess" },
    ui: {
      notify: () => {},
      setStatus: () => {},
      setWidget: () => {},
      confirm: async () => true,
    },
  };
}

// The typed tools, the yolo tool among them, are declared when a session starts.
await handlers.get("session_start")?.({}, ctx());

/** What `rvow yolo` prints for a switch, as TOON with the times it sealed. */
const SWITCH =
  "yolo auto is on for 30 more minutes; the gate still checks at push\n" +
  "tier: auto\nminutes: 30\nat: 1700000000000\nuntil: 1700001800000\n";
const ENTRY: Entry = { tier: "auto", at: 1_700_000_000_000, until: 1_700_001_800_000 };
/** The same switch with no times, as an older binary or a failed run prints it. */
const NO_TIMES = "tier: auto\nminutes: 30\n";

/** Make the fake CLI answer `yolo` with `out`, and exit with `code`. */
function cli(out: string, code = 0) {
  writeFileSync(join(dir, "yolo.out"), out);
  if (code === 0) rmSync(join(dir, "yolo.code"), { force: true });
  else writeFileSync(join(dir, "yolo.code"), String(code));
}

let hookCalls = 0;
/** The yolo entries in the record the extension sends with a fresh hook call. */
async function sent(): Promise<Entry[]> {
  await handlers.get("tool_call")?.(
    { toolName: "edit", input: { path: "src/a.ts", content: "x" }, toolCallId: "t1" },
    ctx(),
  );
  hookCalls++;
  const input = JSON.parse(readFileSync(join(dir, `pre-${hookCalls}.in`), "utf8")) as {
    user_made?: { yolo: Entry[] };
  };
  return input.user_made?.yolo ?? [];
}

/** The entries the record gained while `run` did its work. */
async function gained(run: () => Promise<unknown>): Promise<Entry[]> {
  const before = (await sent()).length;
  await run();
  return (await sent()).slice(before);
}

test("the yolo slash command records the times the CLI printed", async () => {
  cli(SWITCH);
  const gainedEntries = await gained(async () => {
    await commands.get("rvow-yolo")?.handler("auto 30m", ctx());
  });
  assert.deepEqual(gainedEntries, [ENTRY]);
});

test("the yolo slash command records nothing when the output has no times", async () => {
  cli(NO_TIMES);
  const gainedEntries = await gained(async () => {
    await commands.get("rvow-yolo")?.handler("auto 30m", ctx());
  });
  assert.deepEqual(gainedEntries, []);
});

test("the yolo slash command records nothing when the run fails", async () => {
  cli(`error: "the switch file is locked"\ncode: yolo-lock\n`, 1);
  const gainedEntries = await gained(async () => {
    await commands.get("rvow-yolo")?.handler("auto 30m", ctx());
  });
  assert.deepEqual(gainedEntries, []);
});

test("the yolo tool records the times the CLI printed", async () => {
  cli(SWITCH);
  const tool = tools.get("rvow_yolo");
  assert.ok(tool);
  const gainedEntries = await gained(() =>
    tool.execute(
      "call",
      { tier: "auto", for: "30m" },
      new AbortController().signal,
      undefined,
      ctx() as unknown as Parameters<typeof tool.execute>[4],
    ),
  );
  assert.deepEqual(gainedEntries, [ENTRY]);
});

test("the yolo tool records nothing when the output has no times", async () => {
  cli(NO_TIMES);
  const tool = tools.get("rvow_yolo");
  assert.ok(tool);
  const gainedEntries = await gained(() =>
    tool.execute(
      "call",
      { tier: "auto", for: "30m" },
      new AbortController().signal,
      undefined,
      ctx() as unknown as Parameters<typeof tool.execute>[4],
    ),
  );
  assert.deepEqual(gainedEntries, []);
});
