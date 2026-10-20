import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";

const home = mkdtempSync(join(tmpdir(), "rvow-dock-"));
after(() => rmSync(home, { recursive: true, force: true }));
const binary = join(home, "binary.mjs");
const stateFile = join(home, "view.json");
process.env.RVOW_BIN = binary;
process.env.RVOW_FOCUS_SHORTCUT = "alt+w";
process.env.RVOW_DOCK_STATE = stateFile;
writeFileSync(
  binary,
  `#!/usr/bin/env node\nimport fs from 'node:fs'; const args=process.argv.slice(2); if(args[2]==='view') console.log(fs.readFileSync(process.env.RVOW_DOCK_STATE,'utf8')); if(args[2]==='status') console.log(JSON.stringify({status:'ready'})); if(args[2]==='pre-tool-use') console.log(JSON.stringify({hookSpecificOutput:{permissionDecision:'deny',permissionDecisionReason:'user only'}}));\n`,
);
chmodSync(binary, 0o755);
const { default: extension } = await import("./rvow.ts");
const { refreshLoop } = await import("./rvow/refresh.ts");
const view = (title: string) =>
  JSON.stringify({
    branch: "trunk",
    backlog: [],
    archive: [],
    questions: [],
    next: null,
    changes: [{ id: "CHG-a", title, tasks: [] }],
    needs_you: { amendments: 0, questions: 0, blocked: 0, plans: 0 },
  });

type Handler = (event: unknown, ctx: unknown) => unknown;
type Command = { handler: (args: string, ctx: unknown) => unknown };
type Shortcut = { handler: (ctx: unknown) => unknown };
type Widget = { render: (width: number) => string[] };
type Factory = (tui: unknown, theme: unknown) => Widget;
function get<T>(map: Map<string, T>, key: string): T {
  const value = map.get(key);
  assert.ok(value);
  return value;
}

test("dock sees external state, configurable focus, Escape interruption and teardown", async () => {
  writeFileSync(stateFile, view("before external edit"));
  const events = new Map<string, Handler>();
  const commands = new Map<string, Command>();
  const shortcuts = new Map<string, Shortcut>();
  const widgets = new Map<string, Widget | undefined>();
  let listener: ((data: string) => unknown) | undefined;
  let disposed = false;
  let idle = false;
  let refreshes = 0;
  extension({
    on(name: string, handler: Handler) {
      events.set(name, handler);
    },
    registerCommand(name: string, cmd: Command) {
      commands.set(name, cmd);
    },
    registerShortcut(key: string, shortcut: Shortcut) {
      shortcuts.set(key, shortcut);
    },
    registerTool() {},
    appendEntry() {},
    sendMessage() {},
  } as unknown as ExtensionAPI);
  const theme = {
    fg: (_: unknown, text: string) => text,
    bg: (_: unknown, text: string) => text,
    bold: (text: string) => text,
    inverse: (text: string) => text,
  };
  const tui = { terminal: { rows: 40, columns: 100 }, requestRender() {} };
  const ctx = {
    cwd: home,
    hasUI: true,
    mode: "tui",
    isIdle: () => idle,
    sessionManager: { getEntries: () => [] },
    ui: {
      notify() {},
      setStatus() {
        refreshes++;
      },
      setWidget(name: string, factory: Factory | undefined) {
        widgets.set(name, factory ? factory(tui, theme) : undefined);
      },
      onTerminalInput(fn: typeof listener) {
        listener = fn;
        return () => {
          disposed = true;
          listener = undefined;
        };
      },
    },
  } as unknown as ExtensionContext;
  get(events, "session_start")({}, ctx);
  await get(commands, "rvow-dock").handler("on", ctx);
  const waitFor = async (predicate: () => boolean, timeout: number) => {
    const end = Date.now() + timeout;
    while (!predicate() && Date.now() < end)
      await new Promise((resolve) => setTimeout(resolve, 25));
    assert.ok(predicate(), "timed out waiting for external view refresh");
  };
  const render = () => widgets.get("rvow-dock")?.render(100).join("\n") ?? "";
  // A refresh that misses waits for the next 5 s loop turn plus two process
  // starts, which a loaded machine can push past a short budget. The wait
  // polls, so a long budget costs nothing when the view arrives early.
  await waitFor(() => render().includes("before external edit"), 30_000);
  assert.ok(shortcuts.has("alt+w"));
  get(shortcuts, "alt+w").handler(ctx);
  assert.match(render(), / focus /);
  assert.ok(listener);
  assert.equal(listener("\x1b"), undefined, "running agent receives Escape");
  assert.doesNotMatch(render(), / focus /);
  idle = true;
  get(shortcuts, "alt+w").handler(ctx);
  assert.deepEqual(listener("\x1b"), { consume: true });
  writeFileSync(stateFile, view("changed in another shell"));
  await waitFor(() => render().includes("changed in another shell"), 30_000);
  const denied = await get(events, "tool_call")(
    {
      toolName: "bash",
      input: { command: "rvow accept CHG-a" },
      parentToolCallId: "codemode-call",
      toolCallId: "codemode-call/1",
    },
    ctx,
  );
  assert.deepEqual(denied, { block: true, reason: "user only" });
  get(events, "session_shutdown")({}, ctx);
  assert.ok(disposed);
  assert.equal(widgets.get("rvow-dock"), undefined);
  const count = refreshes;
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert.equal(refreshes, count);
});

test("refresh timer bounds delay, avoids overlapping work and stops idempotently", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let calls = 0;
  let release: () => void = () => {};
  const stop = refreshLoop(() => {
    calls++;
    return new Promise<void>((resolve) => {
      release = resolve;
    });
  }, 1);
  t.mock.timers.tick(999);
  await Promise.resolve();
  assert.equal(calls, 0);
  t.mock.timers.tick(1);
  await Promise.resolve();
  assert.equal(calls, 1);
  t.mock.timers.tick(60_000);
  assert.equal(calls, 1);
  release();
  await new Promise<void>((resolve) => queueMicrotask(resolve));
  stop();
  stop();
  t.mock.timers.tick(60_000);
  await Promise.resolve();
  assert.equal(calls, 1);
});
