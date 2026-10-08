import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import {
  Container,
  compositeTuiLine,
  stripTerminalSequences,
  type TuiMouseEvent,
  type TuiMouseEventResult,
  visibleWidth,
} from "@earendil-works/pi-tui";
import {
  decisionPrompt as modDecision,
  refinePrompt as modRefine,
  REFINE_STEPS as modSteps,
} from "../claude/hooks/format.ts";

const dir = mkdtempSync(join(tmpdir(), "rvow-pi-"));
const bin = join(dir, "rvow");
writeFileSync(
  bin,
  `#!/bin/sh
echo "$*" >> "$RVOW_FAKE/argv"
case "$1 $2 $3" in
  "hook claude view") [ -f "$RVOW_FAKE/fail-view" ] && exit 1; [ -f "$RVOW_FAKE/slow-view" ] && sleep 0.5; cat "$RVOW_FAKE/view.json" 2>/dev/null; true ;;
  "hook claude status") cat "$RVOW_FAKE/status.json" 2>/dev/null ;;
  "hook claude pre-tool-use") cat "$RVOW_FAKE/pre.json" 2>/dev/null ;;
  "hook claude stop") cat "$RVOW_FAKE/stop.json" 2>/dev/null ;;
  "hook claude "*) ;;
  *)
    case "$1" in
      review) cat "$RVOW_FAKE/review.txt" ;;
      show) [ -f "$RVOW_FAKE/slow" ] && sleep 0.4; if [ -f "$RVOW_FAKE/show-$2.txt" ]; then cat "$RVOW_FAKE/show-$2.txt"; else echo "$2"; echo "shown $2"; fi ;;
      clarify) cat "$RVOW_FAKE/clarify.json" ;;
      confirm) if [ $# -eq 1 ]; then cat "$RVOW_FAKE/confirm.txt"; else echo "ran $*"; fi ;;
      accept) echo "accepted"; echo "next: TSK-1 first"; ;;
      *) echo "ran $*" ;;
    esac ;;
esac
`,
);
chmodSync(bin, 0o755);
process.env.RVOW_BIN = bin;
const { default: extension } = await import("./rvow.ts");

const EMPTY = {
  branch: "trunk",
  changes: [],
  plans: [],
  backlog: [],
  archive: [
    { id: "CHG-xgnmc0", title: "Open the status pane", outcome: "done", date: "2026-10-02" },
    { id: "CHG-w2at5e", title: "Never fold output", outcome: "dropped", date: "2026-10-02" },
  ],
  questions: [],
  next: null,
  needs_you: { amendments: 0, questions: 0, blocked: 0, plans: 0 },
};
const plan = (id: string, title: string, over: object = {}) => ({
  id,
  title,
  branch: "trunk",
  here: true,
  tasks: 2,
  questions: 0,
  issues: 0,
  ready: true,
  path: `/tmp/${id}`,
  ...over,
});
const WAITING = (...ps: { id: string; title: string }[]) => ({
  ...EMPTY,
  backlog: ps.map((p) => plan(p.id, p.title)),
});
const task = (id: string, state: string, reasons: string[] = []) => ({
  id,
  title: `title of ${id}`,
  state,
  reasons,
});
const change = (over: object = {}) => ({
  id: "CHG-1",
  title: "Do it",
  stage: "active",
  tasks: [task("TSK-1", "proved"), task("TSK-2", "missing", ["no proof"])],
  drift: [],
  pending: [],
  ...over,
});
const ACTIVE = {
  ...EMPTY,
  changes: [change()],
  next: { change: "CHG-1", task: "TSK-2", title: "b", verify: "cargo test b", state: "missing" },
};
const DONE = {
  ...EMPTY,
  changes: [change({ tasks: [task("TSK-1", "proved"), task("TSK-2", "proved")] })],
};

type Handler = (event: unknown, ctx: unknown) => unknown;
type Command = { handler: (args: string, ctx: unknown) => Promise<void> };
type Rendered = {
  render: (width: number) => string[];
  invalidate: () => void;
  handleInput?: (data: string) => void;
  handleMouse?: (e: TuiMouseEvent) => TuiMouseEventResult | undefined;
};
type Factory = (
  tui: unknown,
  theme: unknown,
  kb: unknown,
  done: (result?: unknown) => void,
) => Rendered | Promise<Rendered>;
type Step = (pane: Rendered) => Promise<void> | void;
type Call = { kind: string; args: unknown[] };
type Files = Partial<
  Record<
    | "view.json"
    | "status.json"
    | "pre.json"
    | "stop.json"
    | "review.txt"
    | "slow"
    | "slow-view"
    | "fail-view"
    | "clarify.json"
    | "confirm.txt"
    | `show-${string}.txt`,
    string
  >
>;
type Ui = {
  hasUI?: boolean;
  mode?: string;
  confirm?: boolean;
  pick?: number;
  rows?: number;
  cols?: number;
  draft?: string;
  mark?: boolean;
  entries?: { type: string; customType?: string; data?: unknown }[];
};

const THEME = {
  fg: (_c: string, t: string) => t,
  bold: (t: string) => t,
  inverse: (t: string) => t,
  bg: (_c: string, t: string) => t,
};

// The fakes implement only what rvow.ts uses; the casts say so once, here.
function setup(files: Files, ui: Ui = {}) {
  const home = mkdtempSync(join(dir, "case-"));
  process.env.RVOW_FAKE = home;
  for (const [name, body] of Object.entries(files)) if (body) writeFileSync(join(home, name), body);
  writeFileSync(join(home, "argv"), "");
  const handlers = new Map<string, Handler>();
  const commands = new Map<string, Command>();
  const tools: string[] = [];
  const calls: Call[] = [];
  const entries = ui.entries ?? [];
  const script: Step[] = [];
  const components = new Map<string, Rendered>();
  const shortcuts = new Map<string, (c: unknown) => unknown>();
  const listeners: ((data: string) => { consume?: boolean } | undefined)[] = [];
  const editor = { text: ui.draft ?? "" };
  const slowView = "slow-view" in files;
  const api = {
    on: (name: string, h: Handler) => handlers.set(name, h),
    registerCommand: (name: string, c: Command) => commands.set(name, c),
    registerTool: (t: { name: string }) => tools.push(t.name),
    sendMessage: () => {},
    sendUserMessage: (...args: unknown[]) => void calls.push({ kind: "user", args }),
    registerShortcut: (key: string, o: { handler: (c: unknown) => unknown }) =>
      void shortcuts.set(key, o.handler),
    appendEntry: (customType: string, data: unknown) =>
      void entries.push({ type: "custom", customType, data }),
  };
  extension(api as unknown as ExtensionAPI);
  const rec =
    <T>(kind: string, result: T) =>
    (...args: unknown[]) => {
      calls.push({ kind, args });
      return Promise.resolve(result);
    };
  const theme = ui.mark ? { ...THEME, inverse: (t: string) => `«${t}»` } : THEME;
  const tui = {
    requestRender: () => {},
    terminal: { rows: ui.rows ?? 40, columns: ui.cols ?? 100 },
  };
  const hasUI = ui.hasUI ?? true;
  const ctx = {
    cwd: home,
    hasUI,
    mode: ui.mode ?? (hasUI ? "tui" : "print"),
    sessionManager: { getEntries: () => entries },
    ui: {
      notify: (...args: unknown[]) => void calls.push({ kind: "notify", args }),
      confirm: rec("confirm", ui.confirm ?? true),
      select: (title: string, options: string[]) => {
        calls.push({ kind: "select", args: [title, options] });
        return Promise.resolve(options[ui.pick ?? 0]);
      },
      input: rec("input", undefined),
      setWidget: (...args: unknown[]) => {
        calls.push({ kind: "widget", args });
        const [name, f] = args;
        if (typeof f === "function") components.set(String(name), f(tui, theme));
        else components.delete(String(name));
      },
      onTerminalInput: (h: (data: string) => { consume?: boolean } | undefined) => {
        listeners.push(h);
        return () => void listeners.splice(listeners.indexOf(h), 1);
      },
      getEditorText: () => editor.text,
      setEditorText: (...args: unknown[]) => void calls.push({ kind: "editor", args }),
      pasteToEditor: (...args: unknown[]) => void calls.push({ kind: "editor", args }),
      setStatus: (...args: unknown[]) => void calls.push({ kind: "status", args }),
      custom: (factory: Factory, opts: unknown) => {
        calls.push({ kind: "custom", args: [factory, opts] });
        return new Promise<unknown>((resolve) => {
          Promise.resolve(factory(tui, theme, null, resolve)).then(async (pane) => {
            const loading = () => pane.render(100).join("").includes("loading rvow...");
            for (let i = 0; i < 3000 && loading() && !slowView; i++)
              await new Promise((r) => setTimeout(r, 10));
            await script.shift()?.(pane);
            resolve(undefined);
          });
        });
      },
    },
  } as unknown as ExtensionContext;
  const notes = () => calls.filter((c) => c.kind === "notify").map((c) => String(c.args[0]));
  const users = () => calls.filter((c) => c.kind === "user").map((c) => String(c.args[0]));
  const statuses = () => calls.filter((c) => c.kind === "status").map((c) => c.args[1]);
  const argv = () => readFileSync(join(home, "argv"), "utf8").split("\n").filter(Boolean);
  const command = (name: string, args = "") => {
    const c = commands.get(name);
    assert.ok(c, `${name} is registered`);
    return c.handler(args, ctx);
  };
  const fire = (name: string, event: unknown = {}) => {
    const h = handlers.get(name);
    assert.ok(h, `${name} is handled`);
    return h(event, ctx);
  };
  const lastFactory = (kind: string): Factory => {
    const f = calls
      .filter((c) => c.kind === kind)
      .at(-1)
      ?.args.find((a) => typeof a === "function");
    assert.equal(typeof f, "function", `${kind} was called with a factory`);
    return f as Factory;
  };
  const opened = () => calls.filter((c) => c.kind === "custom").length;
  const queue = (...steps: Step[]) => void script.push(...steps);
  const status = (...steps: Step[]) => {
    script.push(...steps);
    return command("rvow-status");
  };
  const write = (name: keyof Files, text: string) => writeFileSync(join(home, name), text);
  const widget = (key: string) => {
    const call = calls.filter((c) => c.kind === "widget" && c.args[0] === key).at(-1);
    const f = call?.args[1];
    return typeof f === "function" ? (f as Factory) : undefined;
  };
  const key = (data: string) => {
    for (const l of [...listeners]) {
      const r = l(data);
      if (r?.consume) return r;
    }
    return undefined;
  };
  const shortcut = (name: string) => {
    const h = shortcuts.get(name);
    assert.ok(h, `${name} is registered`);
    return h(ctx);
  };
  const dockComponent = async () => {
    const c = components.get("rvow-dock");
    assert.ok(c, "the dock widget is set");
    return c;
  };
  const dock = async (width = ui.cols ?? 100) => {
    const c = components.get("rvow-dock");
    assert.ok(c, "the dock widget is set");
    return c.render(width);
  };
  return {
    dockComponent,
    queue,
    widget,
    key,
    shortcut,
    dock,
    editor,
    tui,
    listeners,
    commands,
    tools,
    calls,
    entries,
    notes,
    users,
    statuses,
    argv,
    command,
    fire,
    lastFactory,
    opened,
    status,
    write,
  };
}

const json = (v: object) => JSON.stringify(v);
const decided = (a: string[]) => a.filter((l) => /^(accept|archive|claim) /.test(l));
const text = (p: Rendered, width = 100) => p.render(width).join("\n");
const press = (p: Rendered, ...keys: string[]) => {
  for (const k of keys) p.handleInput?.(k);
};
const KEY = {
  esc: "\u001b",
  enter: "\r",
  down: "\u001b[B",
  up: "\u001b[A",
  left: "\u001b[D",
  right: "\u001b[C",
  tab: "\t",
  shiftTab: "\u001b[Z",
};

async function until(p: Rendered, re: RegExp, width = 100) {
  for (let i = 0; i < 3000; i++) {
    if (re.test(text(p, width))) return;
    await new Promise((r) => setTimeout(r, 10));
  }
  assert.match(text(p, width), re);
}

async function waitFor(cond: () => boolean) {
  for (let i = 0; i < 3000 && !cond(); i++) await new Promise((r) => setTimeout(r, 10));
  assert.ok(cond(), "condition did not become true");
}

const focused = (p: Rendered, width = 100) =>
  p
    .render(width)
    .filter((l) => l.includes("▌"))
    .join("\n");

test("rvow-status overlay says what is there when nothing is active", async () => {
  const t = setup({ "view.json": json(EMPTY) });
  await t.status(async (p) => {
    const all = text(p);
    assert.match(all, /1 Backlog 0/);
    assert.match(all, /2 Active 0/);
    assert.match(all, /3 Archive 2/);
    assert.match(all, /CHG-xgnmc0/, "an empty branch opens on the archive");
    press(p, "1");
    assert.match(text(p), /No plan in the backlog\./);
    assert.match(text(p), /Start a plan: rvow new/);
    press(p, "2");
    assert.match(text(p), /Nothing active on trunk\./);
    press(p, "3");
    assert.match(text(p), /2026-10-02 CHG-xgnmc0 Open the status pane/);
    assert.match(text(p), /CHG-w2at5e Never fold output/);
  });
});

test("the pane opens as an overlay and starts on the tab with something to do", async () => {
  const waiting = setup({ "view.json": json(WAITING({ id: "CHG-a", title: "A" })) });
  await waiting.status();
  const [factory, opts] = waiting.calls.find((c) => c.kind === "custom")?.args ?? [];
  assert.equal(typeof factory, "function");
  assert.deepEqual(opts, {
    overlay: true,
    overlayOptions: { width: "100%", anchor: "center" },
  });

  const first = async (view: object, re: RegExp) => {
    const t = setup({ "view.json": json(view) });
    await t.status((p) => assert.match(text(p), re));
  };
  await first(WAITING({ id: "CHG-a", title: "A" }), /PLAN {2}A/);
  await first(ACTIVE, /Do it/);
  await first(EMPTY, /CHG-xgnmc0/);
});

test("no tab is ever drawn blank", async () => {
  const t = setup({ "view.json": json({ ...EMPTY, archive: [] }) });
  await t.status((p) => {
    for (const k of ["1", "2", "3"]) {
      press(p, k);
      const body = p
        .render(100)
        .slice(2, -1)
        .join("")
        .replace(/[\s│╭╮╰╯─]/g, "");
      assert.ok(body.length > 10, `tab ${k} has a message`);
    }
    assert.match(text(p), /Nothing archived yet\./);
  });
});

test("rvow-status lists the backlog and every archived entry in plain mode", async () => {
  const t = setup(
    { "view.json": json({ ...WAITING(), backlog: [plan("CHG-b1", "Later")] }) },
    { hasUI: false, mode: "print" },
  );
  await t.command("rvow-status");
  const [out] = t.notes();
  assert.match(out ?? "", /CHG-b1/);
  assert.match(out ?? "", /Later/);
  assert.match(out ?? "", /Nothing active/);
  assert.match(out ?? "", /CHG-xgnmc0/);
});

test("rvow-status with a failing rvow opens the pane and says rvow did not answer", async () => {
  const t = setup({ "fail-view": "1" });
  await t.status((p) => {
    assert.match(text(p), /did not answer/);
    assert.match(text(p), /No rvow change on this branch\./);
  });
  assert.equal(t.opened(), 1);
});

test("the pane opens at once while a slow rvow view loads, then fills in", async () => {
  const t = setup({ "view.json": json(WAITING({ id: "CHG-a", title: "A" })), "slow-view": "1" });
  const start = performance.now();
  await t.status(async (p) => {
    assert.ok(performance.now() - start < 300, "the pane did not wait for rvow");
    assert.match(text(p), /loading rvow\.\.\./);
    for (const line of p.render(60)) assert.equal(visibleWidth(line), 60);
    await until(p, /PLAN {2}A/);
    assert.doesNotMatch(text(p), /loading rvow/);
  });
});

test("tabs switch with 1 2 3, tab, shift+tab and the arrows, and show live counts", async () => {
  const t = setup({ "view.json": json({ ...ACTIVE, backlog: [plan("CHG-a", "A")] }) });
  await t.status((p) => {
    assert.match(text(p), /1 Backlog 1/);
    assert.match(text(p), /2 Active 1/);
    press(p, "2");
    assert.match(text(p), /Do it/);
    press(p, "3");
    assert.match(text(p), /CHG-xgnmc0/);
    press(p, KEY.tab);
    assert.match(text(p), /CHG-a/, "tab wraps to the backlog");
    press(p, KEY.shiftTab);
    assert.match(text(p), /CHG-xgnmc0/, "shift+tab wraps back");
    press(p, KEY.left);
    assert.match(text(p), /Do it/);
    press(p, KEY.right, KEY.right);
    assert.match(text(p), /CHG-a/);
  });
});

test("the footer hint changes with the focus and fits the width", async () => {
  const t = setup({ "view.json": json({ ...WAITING({ id: "CHG-a", title: "A" }), changes: [] }) });
  await t.status((p) => {
    const hint = p.render(100).at(-2) ?? "";
    assert.match(hint, /1-3 tabs.*enter open.*a accept.*esc close/);
    for (const w of [60, 40, 24]) {
      const narrow = p.render(w).at(-2) ?? "";
      assert.match(narrow, /esc close/, `esc survives at ${w}`);
      assert.ok(visibleWidth(narrow) <= w);
    }
  });
});

test("every line fits a narrow terminal", async () => {
  const view = {
    ...ACTIVE,
    backlog: [plan("CHG-a", "A very long plan title that cannot fit in a narrow terminal at all")],
    changes: [
      change({
        drift: [{ class: "weaken", what: "a long drift line that is wider than the pane is" }],
        pending: [{ id: "AMD-1", reason: "a long amendment reason that is also too wide" }],
      }),
    ],
  };
  const t = setup({ "view.json": json(view) });
  await t.status((p) => {
    for (const tab of ["1", "2", "3"]) {
      press(p, tab);
      for (const w of [100, 60, 40, 24, 20]) {
        for (const line of p.render(w)) assert.ok(visibleWidth(line) <= w, `${tab}@${w}: ${line}`);
      }
    }
  });
});

test("the cursor moves with j k and the arrows, and a hotkey acts on the focused plan", async () => {
  const t = setup({
    "view.json": json(WAITING({ id: "CHG-a", title: "A" }, { id: "CHG-b", title: "B" })),
  });
  await t.status((p) => {
    assert.match(focused(p), /PLAN {2}A/);
    press(p, "j");
    assert.match(focused(p), /PLAN {2}B/);
    press(p, "k", "k");
    assert.match(focused(p), /PLAN {2}A/);
    press(p, KEY.down);
    assert.match(focused(p), /PLAN {2}B/);
    press(p, KEY.up);
    assert.match(focused(p), /PLAN {2}A/);
    press(p, "j", "e");
  });
  assert.deepEqual(t.users(), [modRefine([plan("CHG-b", "B")])]);
});

test("a long backlog scrolls so the focused card stays on screen", async () => {
  const ps = Array.from({ length: 12 }, (_, i) => ({ id: `CHG-p${i}`, title: `Plan ${i}` }));
  const t = setup({ "view.json": json(WAITING(...ps)) }, { rows: 24 });
  await t.status((p) => {
    assert.ok(p.render(100).length <= Math.floor(24 * 0.9));
    assert.match(text(p), /Plan 0/);
    for (let i = 0; i < 9; i++) press(p, "j");
    assert.match(focused(p), /Plan 9/);
    assert.doesNotMatch(text(p), /Plan 0/);
    assert.ok(p.render(100).length <= Math.floor(24 * 0.9));
    assert.match(text(p), /\d+-\d+\/\d+/);
  });
});

test("a on a ready plan asks to confirm, then runs rvow accept and tells the agent", async () => {
  const t = setup({
    "view.json": json(WAITING({ id: "CHG-a", title: "A" })),
    "review.txt": "the checklist",
  });
  await t.status((p) => press(p, "a"));
  assert.match(String(t.calls.find((c) => c.kind === "confirm")?.args[1]), /the checklist/);
  assert.deepEqual(decided(t.argv()), ["accept CHG-a"]);
  assert.deepEqual(t.users(), ["I accepted CHG-a. next: TSK-1 first. Continue with it."]);
  assert.equal(t.opened(), 2, "the pane reopens after the action");
  assert.equal(modDecision(["accept", "CHG-a"], "accepted\nnext: TSK-1 first\n"), t.users()[0]);
});

test("a declined confirm runs no accept and says nothing to the agent", async () => {
  const t = setup(
    { "view.json": json(WAITING({ id: "CHG-a", title: "A" })), "review.txt": "x" },
    { confirm: false },
  );
  await t.status((p) => press(p, "a"));
  assert.deepEqual(decided(t.argv()), []);
  assert.deepEqual(t.users(), []);
  assert.equal(t.opened(), 2);
});

test("a on a plan that is not ready says why and does not leave the pane", async () => {
  const view = { ...EMPTY, backlog: [plan("CHG-a", "A", { ready: false, tasks: 0 })] };
  const t = setup({ "view.json": json(view) });
  await t.status((p) => {
    press(p, "a");
    assert.match(text(p), /Not ready: add at least one task with a verify command\./);
  });
  assert.equal(t.opened(), 1);
  assert.deepEqual(
    t.argv().filter((l) => l.startsWith("review")),
    [],
  );
});

test("t takes a plan from another branch, and only that plan", async () => {
  const away = plan("CHG-far", "Far", { here: false, branch: "feature-x" });
  const t = setup({ "view.json": json({ ...EMPTY, backlog: [plan("CHG-a", "A"), away] }) });
  await t.status((p) => {
    press(p, "t");
    assert.deepEqual(decided(t.argv()), [], "t does nothing on a plan that is here");
    press(p, "j");
    assert.match(text(p), /on feature-x/);
    press(p, "t");
  });
  assert.deepEqual(decided(t.argv()), ["claim CHG-far"]);
  assert.deepEqual(t.users(), [], "a claim decides nothing for the agent");
});

test("d and enter open the plan detail from rvow show, b goes back", async () => {
  const t = setup({
    "view.json": json(WAITING({ id: "CHG-a", title: "A" })),
    "show-CHG-a.txt": "CHG-a\ntitle: A\nstage: backlog\ntasks:\n  TSK-1 first",
  });
  await t.status(async (p) => {
    press(p, "d");
    await until(p, /stage: backlog/);
    assert.match(text(p), /PLAN {2}A/);
    assert.match(text(p), /b Back/);
    assert.match(text(p), /a Review & accept/);
    assert.doesNotMatch(text(p), /d Details/, "no Details button inside a detail");
    press(p, "b");
    assert.match(text(p), /1 Backlog 1/);
    assert.doesNotMatch(text(p), /stage: backlog/);
    press(p, KEY.enter);
    await until(p, /stage: backlog/);
  });
  assert.ok(t.argv().includes("show CHG-a --human"));
});

test("a change card shows progress, tasks, drift, pending and next, and Details opens the change", async () => {
  const view = {
    ...ACTIVE,
    changes: [
      change({
        drift: [{ class: "weaken", what: "the verify got weaker" }],
        pending: [{ id: "AMD-1", reason: "add a task" }],
      }),
    ],
  };
  const t = setup(
    {
      "view.json": json(view),
      "show-CHG-1.txt":
        "CHG-1\ntitle: Do it\ntasks:\n  TSK-1 title of TSK-1\n  TSK-2 title of TSK-2",
    },
    { rows: 60 },
  );
  await t.status(async (p) => {
    const card = text(p);
    assert.match(card, /1\/2 proved/);
    assert.match(card, /✓ TSK-1 title of TSK-1/);
    assert.match(card, /· TSK-2 title of TSK-2 \(no proof\)/);
    assert.match(card, /needs you/);
    assert.match(card, /weaken the verify got weaker/);
    assert.match(card, /amend AMD-1 add a task/);
    assert.match(card, /▸ next TSK-2 b/);
    assert.match(card, /\$ cargo test b/);
    assert.match(card, /c Review amendments/);
    assert.match(card, /d Details/);
    assert.doesNotMatch(card, /x Archive/, "not every task is proved");
    press(p, "d");
    await until(p, /CHANGE {2}Do it/);
    press(p, KEY.esc);
  });
});

test("a task row opens the task detail with its covered requirements", async () => {
  const t = setup(
    {
      "view.json": json(ACTIVE),
      "show-TSK-2.txt": "TSK-2\ntitle: b\ncovers: REQ-9\nREQ-9 a requirement\nverify: cargo test b",
      "show-REQ-9.txt": "REQ-9\nthe system shall do it",
    },
    { rows: 60 },
  );
  await t.status(async (p) => {
    press(p, "j", "j");
    assert.match(focused(p), /TSK-2/);
    press(p, KEY.enter);
    await until(p, /the system shall do it/);
    assert.match(text(p), /TASK {2}title of TSK-2/);
    assert.doesNotMatch(text(p), /REQ-9 a requirement/);
  });
  assert.ok(t.argv().includes("show REQ-9 --human"));
});

test("the back stack unwinds change, task, then the tab", async () => {
  const t = setup({
    "view.json": json(ACTIVE),
    "show-CHG-1.txt": "CHG-1\ntitle: Do it\ntasks:\n  TSK-1 title of TSK-1\n  TSK-2 title of TSK-2",
    "show-TSK-2.txt": "TSK-2\ntitle: b\nstage: task",
  });
  await t.status(async (p) => {
    press(p, "d");
    await until(p, /Press enter on a task/);
    press(p, "j", "j", KEY.enter);
    await until(p, /TASK {2}title of TSK-2/);
    press(p, "b");
    assert.match(text(p), /CHANGE {2}Do it/);
    press(p, "b");
    assert.doesNotMatch(text(p), /CHANGE/);
    assert.match(text(p), /1\/2 proved/);
  });
});

test("an archive entry opens its detail", async () => {
  const t = setup({
    "view.json": json(EMPTY),
    "show-CHG-xgnmc0.txt": "CHG-xgnmc0\nstage: done\nledger:\n  archive 1",
  });
  await t.status(async (p) => {
    press(p, "3", KEY.enter);
    await until(p, /stage: done/);
    assert.match(text(p), /ARCHIVED {2}Open the status pane/);
  });
});

test("s selects, f refines every selected plan in one prompt and clears the selection", async () => {
  const ps = [
    { id: "CHG-a", title: "A" },
    { id: "CHG-b", title: "B" },
  ];
  const t = setup({ "view.json": json(WAITING(...ps)) });
  await t.status((p) => {
    assert.doesNotMatch(text(p), /Refine \d selected/);
    press(p, "s", "j", "s");
    assert.match(text(p), /f Refine 2 selected/);
    assert.match(text(p), /Unselect/);
    press(p, "s");
    assert.match(text(p), /f Refine 1 selected/);
    press(p, "s", "f");
  });
  const prompt = modRefine(ps.map((x) => plan(x.id, x.title)));
  assert.deepEqual(t.users(), [prompt]);
  assert.match(t.users()[0] ?? "", new RegExp(modSteps.slice(0, 40)));
  assert.equal(t.opened(), 1, "refine closes the pane so the agent reply is visible");
  assert.ok(t.notes().some((n) => /sent to the agent/.test(n)));
  assert.deepEqual(t.calls.find((c) => c.kind === "user")?.args[1], { deliverAs: "followUp" });
});

test("e refines one plan in the user's voice", async () => {
  const t = setup({ "view.json": json(WAITING({ id: "CHG-a", title: "A" })) });
  await t.status((p) => press(p, "e"));
  assert.deepEqual(t.users(), [modRefine([plan("CHG-a", "A")])]);
  assert.match(
    t.users()[0] ?? "",
    /Review and refine the rvow plan CHG-a \(A\)\. Run `rvow show CHG-a`/,
  );
});

test("x archives only a change whose tasks are all proved, then tells the agent", async () => {
  const half = setup({ "view.json": json(ACTIVE) });
  await half.status((p) => {
    press(p, "2", "x");
    assert.equal(
      p.render(100).some((l) => /x Archive/.test(l)),
      false,
    );
  });
  assert.deepEqual(decided(half.argv()), []);

  const t = setup({ "view.json": json(DONE) });
  await t.status((p) => {
    assert.match(text(p), /x Archive/);
    press(p, "x");
  });
  assert.deepEqual(decided(t.argv()), ["archive CHG-1"]);
  assert.deepEqual(t.users(), ["I archived CHG-1. The change is closed."]);
  assert.equal(t.opened(), 2);
});

test("x works inside the change detail too", async () => {
  const t = setup({
    "view.json": json(DONE),
    "show-CHG-1.txt": "CHG-1\ntitle: Do it\ntasks:\n  TSK-1 a\n  TSK-2 b",
  });
  await t.status(async (p) => {
    press(p, "d");
    await until(p, /x Archive/);
    press(p, "x");
  });
  assert.deepEqual(decided(t.argv()), ["archive CHG-1"]);
});

test("c reviews amendments through the same dialog as /rvow-confirm", async () => {
  const view = {
    ...ACTIVE,
    changes: [change({ pending: [{ id: "AMD-1", reason: "add a task" }] })],
  };
  const t = setup({ "view.json": json(view), "confirm.txt": "AMD-1 add a task\ndigest abc" });
  await t.status((p) => press(p, "c"));
  assert.ok(t.argv().includes("confirm AMD-1 --accept"));
  assert.deepEqual(t.users(), [modDecision(["confirm", "--accept", "AMD-1"], "")]);
  assert.equal(t.opened(), 2);
});

test("A answers the open questions of a plan, and q is not it", async () => {
  const asked = {
    questions: [
      { ref: "Q1", question: "Which?", options: [{ label: "one" }, { label: "two" }], header: "H" },
    ],
    remaining: 0,
  };
  const view = { ...EMPTY, backlog: [plan("CHG-a", "A", { questions: 1 })] };
  const t = setup({ "view.json": json(view), "clarify.json": json(asked) });
  await t.status((p) => {
    assert.match(text(p), /A Answer questions/);
    press(p, "A");
  });
  assert.ok(t.argv().includes("answer Q1 one --via dialog"));
  assert.equal(t.opened(), 2);

  const closing = setup({ "view.json": json(view) });
  await closing.status((p) => press(p, "q"));
  assert.equal(closing.opened(), 1);
  assert.deepEqual(
    closing.argv().filter((l) => l.startsWith("clarify")),
    [],
  );
});

test("plan questions and their recommended answers show on the card", async () => {
  const view = {
    ...EMPTY,
    backlog: [plan("CHG-a", "A", { questions: 1, ready: false })],
    questions: [{ change: "CHG-a", question: "Which store?", recommended: "sqlite" }],
  };
  const t = setup({ "view.json": json(view) });
  await t.status((p) => {
    assert.match(text(p), /\? Which store\? \(recommended: sqlite\)/);
    assert.match(text(p), /to accept: answer the open questions first/);
  });
});

test("r reloads the view and the open detail", async () => {
  const t = setup({
    "view.json": json(WAITING({ id: "CHG-a", title: "A" })),
    "show-CHG-a.txt": "CHG-a\nstage: backlog",
    "status.json": json({ status: "rvow: 1 plan waits for accept" }),
  });
  await t.status(async (p) => {
    press(p, "d");
    await until(p, /stage: backlog/);
    t.write("view.json", json(WAITING({ id: "CHG-a", title: "A" }, { id: "CHG-b", title: "B" })));
    t.write("show-CHG-a.txt", "CHG-a\nstage: renewed");
    press(p, "r");
    await until(p, /stage: renewed/);
    press(p, "b");
    assert.match(text(p), /1 Backlog 2/);
    assert.match(text(p), /CHG-b/);
  });
});

test("Escape and q close the pane in a list and in a detail", async () => {
  for (const key of [KEY.esc, "q"]) {
    const t = setup({ "view.json": json(WAITING({ id: "CHG-a", title: "A" })) });
    await t.status(async (p) => {
      press(p, "d");
      await until(p, /shown CHG-a/);
      press(p, key);
    });
    assert.equal(t.opened(), 1);
  }
});

test("the open tab and detail stack are saved on close and restored in the next session", async () => {
  const t = setup({
    "view.json": json(ACTIVE),
    "show-CHG-1.txt": "CHG-1\ntitle: Do it\ntasks:\n  TSK-1 a",
  });
  await t.status(async (p) => {
    press(p, "3", "2", "d");
    await until(p, /CHANGE {2}Do it/);
  });
  const saved = t.entries.at(-1);
  assert.equal(saved?.customType, "rvow-pane");
  assert.deepEqual(saved?.data, {
    tab: "active",
    details: [{ kind: "change", id: "CHG-1", title: "Do it" }],
  });

  const next = setup(
    { "view.json": json(ACTIVE), "show-CHG-1.txt": "CHG-1\nstage: restored" },
    { entries: t.entries },
  );
  await next.fire("session_start");
  await next.status(async (p) => {
    await until(p, /stage: restored/);
    assert.match(text(p), /CHANGE {2}Do it/);
    assert.match(text(p), /stage: restored/);
    press(p, "b");
    assert.match(text(p), /2 Active 1/);
  });
});

test("restore drops an id that is gone and keeps the saved tab", async () => {
  const entries = [
    {
      type: "custom",
      customType: "rvow-pane",
      data: {
        tab: "archive",
        details: [
          { kind: "plan", id: "CHG-404", title: "Gone" },
          { kind: "bogus", id: "CHG-1", title: "x" },
        ],
      },
    },
  ];
  const t = setup({ "view.json": json(ACTIVE) }, { entries });
  await t.status(async (p) => {
    await until(p, /CHG-xgnmc0/);
    assert.doesNotMatch(text(p), /PLAN|CHANGE|TASK/);
    assert.match(text(p), /CHG-xgnmc0/, "the saved archive tab is open");
  });
  assert.deepEqual(
    t.argv().filter((l) => l.startsWith("show")),
    [],
  );
});

test("an unchanged pane state is not appended to the session again", async () => {
  const t = setup({ "view.json": json(ACTIVE) });
  await t.status();
  await t.status();
  assert.equal(t.entries.length, 1);
});

test("a pane state that is not an object is ignored", async () => {
  const entries = [{ type: "custom", customType: "rvow-pane", data: "junk" }];
  const t = setup({ "view.json": json(ACTIVE) }, { entries });
  await t.status((p) => assert.match(text(p), /2 Active 1/));
});

test("the status line shows rvow's needs-you summary and clears when there is none", async () => {
  const t = setup({
    "view.json": json(ACTIVE),
    "status.json": json({ status: "rvow: 1 amendment, 2 questions" }),
  });
  await t.fire("session_start");
  await waitFor(() => t.statuses().length === 1);
  assert.deepEqual(t.statuses().at(-1), "rvow: 1 amendment, 2 questions");
  assert.deepEqual(
    t.calls.filter((c) => c.kind === "status").map((c) => c.args[0]),
    ["rvow"],
  );
  t.write("status.json", "{}");
  await t.fire("agent_settled");
  await waitFor(() => t.statuses().length === 2);
  assert.equal(t.statuses().at(-1), undefined);
});

test("the status line refreshes at session start, after agent turns and after rvow commands", async () => {
  const t = setup({ "view.json": json(EMPTY), "status.json": json({ status: "s" }) });
  await t.fire("session_start");
  const after = (n: number) => waitFor(() => t.statuses().length === n);
  await after(1);
  await t.fire("agent_settled");
  await after(2);
  await t.fire("tool_result", { toolName: "bash", input: { command: "rvow ls" } });
  await after(3);
  await t.fire("tool_result", { toolName: "bash", input: { command: "ls" } });
  await new Promise((r) => setTimeout(r, 150));
  assert.equal(t.statuses().length, 3);
});

test("the band above the editor names what needs the user, as the mod does", async () => {
  const none = setup({ "view.json": json(EMPTY) });
  await none.fire("session_start");
  await waitFor(() => none.statuses().length === 1);
  assert.equal(
    none.calls.filter((c) => c.kind === "widget" && c.args[0] === "rvow").at(-1)?.args[1],
    undefined,
  );

  const view = {
    ...WAITING({ id: "CHG-a", title: "A" }),
    needs_you: { amendments: 1, questions: 2, blocked: 0, plans: 1 },
  };
  const t = setup({ "view.json": json(view) });
  await t.fire("session_start");
  await waitFor(() => t.statuses().length === 1);
  const band = t.widget("rvow");
  assert.ok(band);
  const lines = (await Promise.resolve(band(t.tui, THEME, null, () => {}))).render(120);
  assert.deepEqual(lines, [
    "rvow · needs you: 1 plan to accept: /rvow-accept CHG-a · 1 amendment: /rvow-confirm · 2 questions /rvow-status",
  ]);
});

test("the shared prompt strings equal the Claude mod's", () => {
  const p = (id: string, title: string) => plan(id, title);
  return import("./rvow.ts").then((m) => {
    assert.equal(m.REFINE_STEPS, modSteps);
    for (const plans of [[p("CHG-a", "A")], [p("CHG-a", "A"), p("CHG-b", "B")]]) {
      assert.equal(m.refinePrompt(plans), modRefine(plans));
    }
    const out = "ok\nnext: TSK-1 go\nAMD-x1";
    for (const argv of [
      ["archive", "CHG-1"],
      ["accept", "CHG-1"],
      ["confirm", "AMD-x1", "--accept"],
      ["confirm", "--reject", "AMD-x1"],
      ["claim", "CHG-1"],
    ]) {
      assert.equal(m.decisionPrompt(argv, out), modDecision(argv, out), argv.join(" "));
    }
  });
});

test("tool_call blocks when the hook denies and passes otherwise", async () => {
  const deny = json({
    hookSpecificOutput: { permissionDecision: "deny", permissionDecisionReason: "no edits yet" },
  });
  const t = setup({ "pre.json": deny });
  const event = { toolName: "bash", input: { command: "ls" } };
  assert.deepEqual(await t.fire("tool_call", event), { block: true, reason: "no edits yet" });
  assert.ok(t.argv().includes("hook claude pre-tool-use"));

  const ok = setup({ "pre.json": json({ hookSpecificOutput: { permissionDecision: "allow" } }) });
  assert.equal(await ok.fire("tool_call", event), undefined);
  assert.equal(await setup({}).fire("tool_call", event), undefined);
});

test("tool_call asks the user when the hook asks, and a no keeps the call blocked", async () => {
  const ask = json({
    hookSpecificOutput: { permissionDecision: "ask", permissionDecisionReason: "rvow: allow it?" },
  });
  const event = { toolName: "bash", input: { command: "ls" } };

  const yes = setup({ "pre.json": ask }, { confirm: true });
  assert.equal(await yes.fire("tool_call", event), undefined);
  assert.equal(yes.calls.find((c) => c.kind === "confirm")?.args[1], "rvow: allow it?");

  const no = setup({ "pre.json": ask }, { confirm: false });
  assert.deepEqual(await no.fire("tool_call", event), { block: true, reason: "rvow: allow it?" });

  const nobody = setup({ "pre.json": ask }, { hasUI: false });
  assert.deepEqual(await nobody.fire("tool_call", event), {
    block: true,
    reason: "rvow: allow it?",
  });
});

test("the stop gate continues 8 times, then lets the agent stop", async () => {
  const t = setup({ "stop.json": json({ reason: "tasks are open" }) });
  const results: unknown[] = [];
  for (let i = 0; i < 9; i++) results.push(await t.fire("agent_before_settle", { entries: [] }));
  assert.equal(results.filter((r) => r !== undefined).length, 8);
  assert.equal(results[8], undefined);
});

test("rvow-confirm refuses without a UI", async () => {
  const t = setup({}, { hasUI: false });
  await t.command("rvow-confirm");
  assert.match(t.notes().join(), /in a terminal/);
  assert.deepEqual(t.argv(), []);
});

test("rvow-confirm tells the agent what the user decided", async () => {
  const t = setup({ "confirm.txt": "AMD-1 one\nAMD-2 two" }, { confirm: true });
  await t.command("rvow-confirm");
  assert.deepEqual(t.users(), [modDecision(["confirm", "--accept", "AMD-1", "AMD-2"], "")]);
});

test("the pi extension registers its commands and tool", () => {
  const t = setup({});
  for (const c of ["status", "confirm", "review", "accept", "archive"])
    assert.ok(t.commands.has(`rvow-${c}`), c);
  assert.deepEqual(t.tools, ["rvow_ask"]);
});

for (const name of ["rvow-review", "rvow-accept", "rvow-archive"]) {
  test(`${name} refuses without a UI and runs nothing`, async () => {
    const t = setup({}, { hasUI: false });
    await t.command(name, "CHG-a");
    assert.match(t.notes().join(), /in a terminal/);
    assert.deepEqual(t.argv(), []);
  });
}

for (const name of ["rvow-review", "rvow-accept"]) {
  test(`${name} without an id says when no plan waits`, async () => {
    const t = setup({ "view.json": json(EMPTY) });
    await t.command(name);
    assert.match(t.notes().join(), /no plan waits/);
    assert.deepEqual(decided(t.argv()), []);
  });

  test(`${name} without an id uses the only waiting plan`, async () => {
    const t = setup({
      "view.json": json(WAITING({ id: "CHG-a", title: "A" })),
      "review.txt": "CHG-a\nchecklist",
    });
    await t.command(name);
    assert.ok(t.argv().includes("review CHG-a"));
    assert.equal(
      t.calls.some((c) => c.kind === "select"),
      false,
    );
  });

  test(`${name} without an id asks which of several plans`, async () => {
    const t = setup(
      {
        "view.json": json(WAITING({ id: "CHG-a", title: "A" }, { id: "CHG-b", title: "B" })),
        "review.txt": "checklist",
      },
      { pick: 1 },
    );
    await t.command(name);
    assert.deepEqual(t.calls.find((c) => c.kind === "select")?.args[1], ["CHG-a A", "CHG-b B"]);
    assert.ok(t.argv().includes("review CHG-b"));
  });
}

test("rvow-review shows the review output in the overlay", async () => {
  const t = setup({ "review.txt": "CHG-a\nthe checklist" });
  await t.command("rvow-review", "CHG-a");
  const pane = await t.lastFactory("custom")(null, THEME, null, () => {});
  assert.match(text(pane), /the checklist/);
  assert.deepEqual(decided(t.argv()), []);
  assert.deepEqual(t.users(), []);
});

test("rvow-accept declined runs no accept", async () => {
  const t = setup({ "review.txt": "the checklist" }, { confirm: false });
  await t.command("rvow-accept", "CHG-a");
  assert.match(String(t.calls.find((c) => c.kind === "confirm")?.args[1]), /the checklist/);
  assert.deepEqual(decided(t.argv()), []);
  assert.deepEqual(t.users(), []);
});

test("rvow-accept confirmed runs rvow accept with the id and tells the agent", async () => {
  const t = setup({ "review.txt": "the checklist" });
  await t.command("rvow-accept", "CHG-a");
  assert.deepEqual(decided(t.argv()), ["accept CHG-a"]);
  assert.ok(t.notes().includes("accepted\nnext: TSK-1 first"));
  assert.deepEqual(t.users(), ["I accepted CHG-a. next: TSK-1 first. Continue with it."]);
});

test("rvow-archive needs an id, then archives it", async () => {
  const t = setup({});
  await t.command("rvow-archive");
  assert.match(t.notes().join(), /usage: \/rvow-archive/);
  assert.deepEqual(t.argv(), []);
  await t.command("rvow-archive", "CHG-a");
  assert.deepEqual(decided(t.argv()), ["archive CHG-a"]);
  assert.ok(t.notes().includes("ran archive CHG-a"));
  assert.deepEqual(t.users(), ["I archived CHG-a. The change is closed."]);
});

test("the panel is one frame with the same size on every tab and detail", async () => {
  const view = {
    ...ACTIVE,
    backlog: [plan("CHG-a", "A")],
    changes: [change({ pending: [{ id: "AMD-1", reason: "add" }] })],
  };
  const t = setup({ "view.json": json(view) }, { rows: 40 });
  await t.status(async (p) => {
    for (const w of [130, 100, 60]) {
      const heights = new Set<number>();
      for (const k of ["1", "2", "3"]) {
        press(p, k);
        const lines = p.render(w);
        heights.add(lines.length);
        const panel = Math.min(w, 100);
        const left = Math.floor((w - panel) / 2);
        const framed = lines.map((l) => stripTerminalSequences(l).slice(left, left + panel));
        assert.match(framed[0] ?? "", /^╭─+╮$/, `top border at ${w}`);
        assert.match(framed.at(-1) ?? "", /^╰─+╯$/, `bottom border at ${w}`);
        assert.match(framed[1] ?? "", /^│ .* │$/, "title row");
        assert.match(framed[1] ?? "", /1 Backlog/);
        assert.match(framed.at(-2) ?? "", /esc close/, "hint sits inside the bottom border");
        for (const line of lines) assert.equal(visibleWidth(line), w, `row width at ${w}`);
      }
      press(p, "2", "d");
      await until(p, /shown CHG-1/, w);
      heights.add(p.render(w).length);
      press(p, "b");
      assert.equal(heights.size, 1, `height is stable at ${w}`);
      assert.ok(p.render(w).length <= Math.floor(40 * 0.9));
    }
  });
});

test("composited over dense base text, no base character shows through the panel rows", async () => {
  const t = setup({ "view.json": json({ ...ACTIVE, backlog: [plan("CHG-a", "A")] }) });
  await t.status((p) => {
    for (const w of [100, 60]) {
      for (const k of ["1", "2", "3"]) {
        press(p, k);
        for (const line of p.render(w)) {
          const base = `\u001b[2m${"#".repeat(w + 4)}`;
          const out = stripTerminalSequences(compositeTuiLine(base, line, 0, w, w));
          assert.equal(visibleWidth(out), w);
          assert.doesNotMatch(out, /#/, `base text shows through: ${out}`);
        }
      }
    }
  });
});

test("opening a detail never blocks: the key returns at once and a loading line shows", async () => {
  const t = setup({ "view.json": json(WAITING({ id: "CHG-a", title: "A" })), slow: "1" });
  await t.status(async (p) => {
    const start = performance.now();
    press(p, "d");
    const lines = p.render(100);
    assert.ok(performance.now() - start < 100, "key press and render return before rvow show ends");
    assert.match(lines.join("\n"), /loading rvow show CHG-a/);
    await until(p, /shown CHG-a/);
  });
});

test("render and key handling stay fast on a long backlog", async () => {
  const ps = Array.from({ length: 200 }, (_, i) => ({ id: `CHG-p${i}`, title: `Plan ${i}` }));
  const t = setup({ "view.json": json(WAITING(...ps)) });
  await t.status((p) => {
    const start = performance.now();
    for (let i = 0; i < 50; i++) {
      press(p, "j");
      p.render(100);
    }
    assert.ok(
      (performance.now() - start) / 50 < 80,
      "under 80 ms per key and render (200 plans, loaded machine)",
    );
  });
});

test("session start, agent turns and rvow tool results never wait for a slow rvow", async () => {
  const t = setup({ "view.json": json(EMPTY), "slow-view": "1" });
  const start = performance.now();
  await t.fire("session_start");
  await t.fire("agent_settled");
  await t.fire("tool_result", { toolName: "bash", input: { command: "rvow ls" } });
  assert.ok(performance.now() - start < 100, "handlers returned before rvow answered");
});

const DOCK_ON = [{ type: "custom", customType: "rvow-dock", data: { on: true } }];
const NEEDY = {
  ...ACTIVE,
  backlog: [plan("CHG-a", "Alpha")],
  needs_you: { amendments: 1, questions: 0, blocked: 0, plans: 1 },
};

async function docked(view: object, ui: Ui = {}, files: Files = {}) {
  const t = setup(
    { "view.json": json(view), "status.json": json({ status: "s" }), ...files },
    { entries: [...DOCK_ON], ...ui },
  );
  await t.fire("session_start");
  await waitFor(() => t.statuses().length === 1);
  return t;
}

const dockText = async (t: Awaited<ReturnType<typeof docked>>) => (await t.dock()).join("\n");

test("the dock is off by default and /rvow-dock turns it on and off", async () => {
  const t = setup({ "view.json": json(NEEDY) });
  await t.fire("session_start");
  assert.equal(t.widget("rvow-dock"), undefined);
  await t.command("rvow-dock", "on");
  assert.ok(t.widget("rvow-dock"));
  assert.deepEqual(t.calls.filter((c) => c.kind === "widget").at(-1)?.args[2], {
    placement: "aboveEditor",
  });
  await t.command("rvow-dock", "off");
  assert.equal(t.widget("rvow-dock"), undefined);
  await t.command("rvow-dock");
  assert.ok(t.widget("rvow-dock"), "no argument toggles");
  await t.command("rvow-dock", "bogus");
  assert.ok(t.notes().some((n) => /usage: \/rvow-dock/.test(n)));
});

test("/rvow-dock needs a UI", async () => {
  const t = setup({}, { hasUI: false });
  await t.command("rvow-dock", "on");
  assert.match(t.notes().join(), /terminal UI/);
  assert.equal(t.widget("rvow-dock"), undefined);
});

test("the dock state is remembered in the session and restored at session start", async () => {
  const t = setup({ "view.json": json(NEEDY) });
  await t.command("rvow-dock", "on");
  assert.deepEqual(t.entries.at(-1), {
    type: "custom",
    customType: "rvow-dock",
    data: { on: true },
  });
  const again = setup({ "view.json": json(NEEDY) }, { entries: t.entries });
  await again.fire("session_start");
  assert.ok(again.widget("rvow-dock"), "restored on");
  await t.command("rvow-dock", "off");
  const off = setup({ "view.json": json(NEEDY) }, { entries: t.entries });
  await off.fire("session_start");
  assert.equal(off.widget("rvow-dock"), undefined, "restored off");
});

test("the dock shows branch, needs-you counts, the change with progress, plans and next", async () => {
  const t = await docked(NEEDY);
  const lines = await t.dock();
  const all = lines.join("\n");
  assert.match(
    lines[0] ?? "",
    /^╭ rvow ⎇ trunk · needs you: 1 plan · 1 amendment ─+.*alt\+r focus─╮$/,
  );
  assert.match(all, /▸ Do it █+░+ 1\/2/);
  assert.match(all, /✓ TSK-1 title of TSK-1/);
  assert.match(all, /· TSK-2 title of TSK-2/);
  assert.doesNotMatch(all, /PLAN Alpha/, "plans live on the Backlog tab");
  assert.match(all, /▸ next TSK-2 b/);
  assert.match(lines.at(-1) ?? "", /^╰─+╯$/);
  for (const l of lines) assert.equal(visibleWidth(l), 100);
});

test("dock height: content-sized, capped at 10 rows and 30% of the terminal", async () => {
  const tasks = Array.from({ length: 30 }, (_, i) => task(`TSK-${i}`, "missing"));
  const big = { ...EMPTY, changes: [change({ tasks })] };
  assert.equal((await (await docked(big, { rows: 60 })).dock()).length, 10);
  assert.equal((await (await docked(big, { rows: 24 })).dock()).length, 7, "30% of 24 rows");
  assert.equal((await (await docked(big, { rows: 20 })).dock()).length, 6, "30% of 20 rows");
  const small = { ...EMPTY, changes: [change({ tasks: [task("TSK-1", "proved")] })] };
  assert.equal(
    (await (await docked(small, { rows: 60 })).dock()).length,
    5,
    "title, counts, change, task, bottom",
  );
});

test("the dock hides only when the terminal is too small", async () => {
  assert.deepEqual(await (await docked(NEEDY, { cols: 49 })).dock(49), []);
  assert.deepEqual(await (await docked(NEEDY, { rows: 19 })).dock(), []);
  assert.ok((await (await docked(NEEDY, { cols: 50, rows: 20 })).dock(50)).length > 0);
  const t = await docked({
    ...EMPTY,
    needs_you: { amendments: 0, questions: 2, blocked: 0, plans: 0 },
  });
  assert.match((await t.dock())[0] ?? "", /needs you: 2 questions/);
});

test("every dock state shows the tab counts like the pane's tab bar", async () => {
  const busy = await dockText(await docked({ ...NEEDY, archive: [] }));
  assert.match(busy, / 1 Backlog 1 {3}2 Active 1 {3}3 Archive 0 /);
  const idle = await dockText(await docked(EMPTY));
  assert.match(idle, / 1 Backlog 0 {3}2 Active 0 {3}3 Archive 2 /);
});

test("an idle dock opens on the Archive tab and lists as many entries as fit", async () => {
  const archive = Array.from({ length: 5 }, (_, i) => ({
    id: `CHG-a${i}`,
    title: `Shipped ${i}`,
    outcome: i === 1 ? "dropped" : "done",
    date: "2026-10-02",
  }));
  const t = await docked({ ...EMPTY, archive });
  const lines = await t.dock();
  const all = lines.join("\n");
  assert.match(lines[0] ?? "", /^╭ rvow ⎇ trunk .*alt\+r focus─╮$/);
  assert.match(all, /1 Backlog 0 {3}2 Active 0 {3}3 Archive 5/);
  assert.match(all, /✓ 2026-10-02 CHG-a0 done Shipped 0/);
  assert.match(all, /✗ 2026-10-02 CHG-a1 dropped Shipped 1/);
  assert.match(all, /CHG-a4 done Shipped 4/);
  assert.equal(lines.length, 2 + 1 + 5);
  for (const l of lines) assert.equal(visibleWidth(l), 100);
});

test("an idle dock with no archive and a fresh project still draws", async () => {
  const t = await docked({ ...EMPTY, archive: [] });
  const lines = await t.dock();
  assert.equal(lines.length, 4);
  assert.match(lines.join("\n"), /Nothing archived yet\./);
  assert.match(lines.join("\n"), /1 Backlog 0 {3}2 Active 0 {3}3 Archive 0/);
});

test("alt+r works on an idle dock: move over archive rows, open the Archive tab, refresh, Esc", async () => {
  const t = await docked(
    EMPTY,
    {},
    { "show-CHG-w2at5e.txt": "CHG-w2at5e\nstage: done\nledger:\n  archive 1" },
  );
  await t.shortcut("alt+r");
  assert.match(await dockText(t), / focus /);
  assert.match(await dockText(t), /j\/k move.*enter open.*esc editor/);
  assert.match((await t.dock()).find((l) => l.includes("›")) ?? "", /CHG-xgnmc0/);
  t.key("j");
  assert.match((await t.dock()).find((l) => l.includes("›")) ?? "", /CHG-w2at5e/);
  let seen = "";
  t.queue(async (p) => {
    await until(p, /stage: done/);
    seen = text(p);
    press(p, KEY.esc);
  });
  t.key("\r");
  await waitFor(() => /stage: done/.test(seen));
  assert.match(seen, /ARCHIVED {2}Never fold output/);
  await waitFor(() => t.key("k")?.consume === true);
  assert.deepEqual(t.key("r"), { consume: true });
  assert.deepEqual(t.key("a"), { consume: true }, "an action key has nothing to act on");
  assert.match(await dockText(t), /Nothing to act on here\./);
  assert.deepEqual(t.key("\u001b"), { consume: true });
  assert.equal(t.key("a"), undefined, "after Esc keys reach the editor");
});

test("the dock and /rvow-dock say why it cannot be shown", async () => {
  const off = setup({ "view.json": json(NEEDY) });
  await off.fire("session_start");
  await off.shortcut("alt+r");
  assert.ok(off.notes().includes("the rvow dock is off; run /rvow-dock on"));

  const small = await docked(NEEDY, { cols: 45, rows: 30 });
  await small.shortcut("alt+r");
  const msg =
    "terminal too small for the rvow dock (needs 50 columns by 20 rows, this one is 45x30)";
  assert.ok(small.notes().includes(msg));
  await small.command("rvow-dock", "on");
  assert.ok(small.notes().filter((n) => n === msg).length === 2, "/rvow-dock on says it too");
  assert.equal(small.key("j"), undefined);

  const ok = setup({ "view.json": json(NEEDY), "status.json": json({ status: "s" }) });
  await ok.fire("session_start");
  await waitFor(() => ok.statuses().length === 1);
  await ok.command("rvow-dock", "on");
  assert.ok(
    ok.notes().includes("rvow dock on: shows plans, changes and the next task; alt+r focuses it"),
  );
});

test("the dock shows a loading line, and alt+r says rvow did not answer yet, until the view arrives", async () => {
  const t = setup({ "view.json": json(NEEDY), "slow-view": "1" }, { entries: [...DOCK_ON] });
  await t.fire("session_start");
  const loading = await t.dock();
  assert.ok(loading.length > 0, "not zero rows while loading");
  assert.match(loading.join("\n"), /loading rvow\.\.\./);
  await t.shortcut("alt+r");
  assert.ok(t.notes().includes("rvow did not answer yet"));
  await waitFor(() => t.statuses().length === 1);
  assert.doesNotMatch((await t.dock()).join("\n"), /loading/);
  await t.shortcut("alt+r");
  assert.match((await t.dock()).join("\n"), / focus /);
});

test("a long dock scrolls to keep the focused row visible", async () => {
  const tasks = Array.from({ length: 30 }, (_, i) => task(`TSK-${i}`, "missing"));
  const t = await docked({ ...EMPTY, changes: [change({ tasks })] }, { rows: 60 });
  t.shortcut("alt+r");
  for (let i = 0; i < 20; i++) t.key("j");
  const all = await dockText(t);
  assert.match(all, /› .*TSK-19/);
  assert.doesNotMatch(all, /TSK-0 /);
  assert.equal((await t.dock()).length, 10);
});

test("alt+r focuses the dock and leaves it again; the title row shows the focus tag", async () => {
  const t = await docked(NEEDY);
  assert.match(await dockText(t), /alt\+r focus/);
  await t.shortcut("alt+r");
  assert.match(await dockText(t), / focus /);
  assert.doesNotMatch(await dockText(t), /alt\+r focus/);
  assert.match(await dockText(t), /j\/k move.*esc editor/, "key hint inside the dock");
  await t.shortcut("alt+r");
  assert.match(await dockText(t), /alt\+r focus/);
});

test("unfocused, the dock consumes nothing; focused, it consumes its keys and passes the rest", async () => {
  const t = await docked(NEEDY);
  await t.dock();
  for (const k of ["j", "k", "a", "\r", "\u001b", "x"]) assert.equal(t.key(k), undefined, k);
  t.shortcut("alt+r");
  for (const k of ["j", "k", "\u001b[B", "\u001b[A", "r", "\u001b"]) {
    t.shortcut("alt+r");
    t.shortcut("alt+r");
    assert.deepEqual(t.key(k), { consume: true }, JSON.stringify(k));
    if (k === "\u001b") t.shortcut("alt+r");
  }
  t.shortcut("alt+r");
  for (const k of ["z", "q", "\u001b[D", "1", " ", "\u0003"])
    assert.equal(t.key(k), undefined, `${JSON.stringify(k)} reaches the editor`);
});

test("j k and the arrows move the dock cursor; Esc returns focus to the editor", async () => {
  const t = await docked(NEEDY);
  await t.shortcut("alt+r");
  const on = async () => (await t.dock()).find((l) => l.includes("›")) ?? "";
  assert.match(await on(), /▸ Do it/);
  t.key("j");
  assert.match(await on(), /TSK-1/);
  t.key("\u001b[B");
  assert.match(await on(), /TSK-2/);
  t.key("k");
  assert.match(await on(), /TSK-1/);
  t.key("\u001b[A");
  t.key("k");
  assert.match(await on(), /▸ Do it/, "stops at the first row");
  for (let i = 0; i < 9; i++) t.key("j");
  assert.match(await on(), /TSK-2/, "stops at the last item");
  assert.deepEqual(t.key("\u001b"), { consume: true });
  assert.equal(t.key("j"), undefined, "after Esc keys go to the editor");
  assert.doesNotMatch(await dockText(t), /›/);
});

test("the dock action hotkeys act on the focused plan", async () => {
  const focusPlan = async (t: Awaited<ReturnType<typeof docked>>) => {
    await t.shortcut("alt+r");
    t.key("1");
  };

  const accept = await docked(NEEDY, {}, { "review.txt": "checklist" });
  await focusPlan(accept);
  accept.key("a");
  await waitFor(() => decided(accept.argv()).length === 1);
  assert.deepEqual(decided(accept.argv()), ["accept CHG-a"]);
  assert.match(String(accept.calls.find((c) => c.kind === "confirm")?.args[1]), /checklist/);
  await waitFor(() => accept.users().length === 1);

  const refine = await docked(NEEDY);
  await focusPlan(refine);
  refine.key("e");
  await waitFor(() => refine.users().length === 1);
  assert.deepEqual(refine.users(), [modRefine([plan("CHG-a", "Alpha")])]);

  const pick = await docked(NEEDY);
  await focusPlan(pick);
  pick.key("s");
  assert.match(await dockText(pick), /◉ PLAN Alpha/);
  assert.match(await dockText(pick), /f refine 1/);
  pick.key("f");
  await waitFor(() => pick.users().length === 1);
  pick.key("s");

  const away = plan("CHG-far", "Far", { here: false, branch: "other" });
  const take = await docked({ ...EMPTY, backlog: [plan("CHG-a", "A"), away] });
  await take.shortcut("alt+r");
  take.key("j");
  take.key("t");
  await waitFor(() => decided(take.argv()).length === 1);
  assert.deepEqual(decided(take.argv()), ["claim CHG-far"]);
});

test("t takes a plan that is not here, and A answers open questions", async () => {
  const away = plan("CHG-far", "Far", { here: false, branch: "other" });
  const take = await docked({ ...EMPTY, backlog: [away] });
  await take.shortcut("alt+r");
  assert.match(await dockText(take), /PLAN Far CHG-far on other/);
  take.key("t");
  await waitFor(() => take.argv().some((l) => l.startsWith("claim")));
  assert.ok(take.argv().includes("claim CHG-far"));

  const asked = {
    questions: [{ ref: "Q1", question: "Which?", options: [{ label: "one" }] }],
    remaining: 0,
  };
  const withQ = { ...EMPTY, backlog: [plan("CHG-a", "A", { questions: 1 })] };
  const ask = await docked(withQ, {}, { "clarify.json": json(asked) });
  await ask.shortcut("alt+r");
  ask.key("A");
  await waitFor(() => ask.argv().includes("answer Q1 one --via dialog"));
});

test("x archives a finished change and c reviews amendments from the dock", async () => {
  const done = await docked(DONE);
  await done.shortcut("alt+r");
  assert.match(await dockText(done), /x archive/);
  done.key("x");
  await waitFor(() => decided(done.argv()).length === 1);
  assert.deepEqual(decided(done.argv()), ["archive CHG-1"]);

  const half = await docked(ACTIVE);
  await half.shortcut("alt+r");
  assert.doesNotMatch(await dockText(half), /x archive/);
  assert.deepEqual(half.key("x"), { consume: true });
  assert.match(await dockText(half), /Nothing to act on here\./);

  const view = { ...ACTIVE, changes: [change({ pending: [{ id: "AMD-1", reason: "add" }] })] };
  const amend = await docked(view, {}, { "confirm.txt": "AMD-1 add" });
  await amend.shortcut("alt+r");
  assert.match(await dockText(amend), /c amend/);
  amend.key("c");
  await waitFor(() => amend.argv().includes("confirm AMD-1 --accept"));
});

test("a not-ready plan says why inside the dock", async () => {
  const view = { ...EMPTY, backlog: [plan("CHG-a", "A", { ready: false, tasks: 0 })] };
  const t = await docked(view);
  await t.shortcut("alt+r");
  assert.deepEqual(t.key("a"), { consume: true });
  assert.match(await dockText(t), /✗ Not ready: add at least one task/);
  assert.deepEqual(
    t.argv().filter((l) => l.startsWith("review")),
    [],
  );
});

test("enter opens the modal pane at the focused item, and focus returns to the dock", async () => {
  const t = await docked(NEEDY, {}, { "show-CHG-1.txt": "CHG-1\ntitle: Do it\nstage: active" });
  await t.shortcut("alt+r");
  let seen = "";
  let during: unknown = "unset";
  t.queue(async (p) => {
    await until(p, /stage: active/);
    seen = text(p);
    during = t.key("j");
    press(p, KEY.esc);
  });
  t.key("\r");
  await waitFor(() => t.opened() === 1);
  await waitFor(() => /stage: active/.test(seen));
  assert.match(seen, /CHANGE {2}Do it/);
  assert.equal(during, undefined, "the dock does not eat keys meant for the modal");
  await waitFor(() => t.key("k")?.consume === true);
  assert.match(await dockText(t), / focus /, "focus came back");
});

test("enter on a task opens the task detail, on a plan the plan detail", async () => {
  const t = await docked(NEEDY, {}, { "show-TSK-1.txt": "TSK-1\ntitle: a\nstage: t1" });
  await t.shortcut("alt+r");
  t.key("j");
  let seen = "";
  t.queue(async (p) => {
    await until(p, /stage: t1/);
    seen = text(p);
  });
  t.key("d");
  await waitFor(() => /stage: t1/.test(seen));
  assert.match(seen, /TASK {2}title of TSK-1/);
});

test("the dock and the modal share the selection", async () => {
  const t = await docked(NEEDY);
  await t.shortcut("alt+r");
  t.key("1");
  t.key("s");
  let seen = "";
  t.queue((p) => {
    press(p, "1");
    seen = text(p);
    press(p, "s");
  });
  t.key("\r");
  await waitFor(() => t.opened() === 1);
  await waitFor(() => seen !== "");
  assert.match(seen, /f Refine 1 selected/);
  await waitFor(() => t.key("k")?.consume === true);
  assert.doesNotMatch(await dockText(t), /◉/, "unselected in the modal shows in the dock");
});

test("focus changes never touch the editor", async () => {
  const t = await docked(NEEDY, { draft: "half a sentence" });
  await t.shortcut("alt+r");
  t.key("j");
  t.key("z");
  t.key("\u001b");
  await t.shortcut("alt+r");
  t.key("\u001b");
  assert.equal(t.editor.text, "half a sentence");
  assert.deepEqual(
    t.calls.filter((c) => c.kind === "editor"),
    [],
  );
});

test("the band is hidden while the dock is shown and returns when it is off or hidden", async () => {
  const view = {
    ...WAITING({ id: "CHG-a", title: "A" }),
    needs_you: { amendments: 0, questions: 1, blocked: 0, plans: 1 },
  };
  const band = async (t: Awaited<ReturnType<typeof docked>>, width: number) => {
    const f = t.widget("rvow");
    assert.ok(f);
    return (await Promise.resolve(f(t.tui, THEME, null, () => {}))).render(width);
  };
  const on = await docked(view);
  assert.deepEqual(await band(on, 100), []);
  const narrow = await docked(view, { cols: 40 });
  assert.equal((await band(narrow, 40)).length, 1, "dock hidden, band returns");
  const short = await docked(view, { rows: 19 });
  assert.equal((await band(short, 100)).length, 1);
  await on.command("rvow-dock", "off");
  assert.equal((await band(on, 100)).length, 1, "dock off, band returns");
});

test("the dock refreshes with the view in the background and never blocks", async () => {
  const t = await docked(NEEDY);
  const before = await dockText(t);
  t.write("view.json", json({ ...NEEDY, changes: [change({ title: "Renamed" })] }));
  await t.fire("agent_settled");
  await waitFor(() => t.statuses().length === 2);
  assert.notEqual(await dockText(t), before);
  assert.match(await dockText(t), /Renamed/);

  const slow = await docked(NEEDY, {}, { "slow-view": "1" });
  await slow.shortcut("alt+r");
  const start = performance.now();
  slow.key("r");
  for (let i = 0; i < 20; i++) slow.key("j");
  await slow.dock();
  assert.ok(performance.now() - start < 150, "keys and render do not wait for rvow");
});

test("rvow printing nothing means a project with no change and no archive", async () => {
  const t = setup({});
  await t.status((p) => {
    assert.match(text(p), /Nothing archived yet\./);
    assert.doesNotMatch(text(p), /did not answer/);
    press(p, "1");
    assert.match(text(p), /No plan in the backlog\./);
  });
  const d = setup({ "status.json": json({ status: "s" }) }, { entries: [...DOCK_ON] });
  await d.fire("session_start");
  await waitFor(() => d.statuses().length === 1);
  const lines = (await d.dock()).join("\n");
  assert.match(lines, /Nothing archived yet\./);
  assert.match(lines, /1 Backlog 0 {3}2 Active 0 {3}3 Archive 0/);
  await d.shortcut("alt+r");
  assert.match((await d.dock()).join("\n"), / focus /);
});

test("a failing rvow keeps the dock on its loading line and says so on alt+r", async () => {
  const t = setup({ "fail-view": "1" }, { entries: [...DOCK_ON] });
  await t.fire("session_start");
  await new Promise((r) => setTimeout(r, 100));
  assert.match((await t.dock()).join("\n"), /loading rvow\.\.\./);
  await t.shortcut("alt+r");
  assert.ok(t.notes().includes("rvow did not answer yet"));
});

const ARCHIVE10 = Array.from({ length: 10 }, (_, i) => ({
  id: `CHG-a${i}`,
  title: `Shipped ${i}`,
  outcome: "done",
  date: "2026-10-02",
}));
const FULL = {
  ...ACTIVE,
  backlog: [plan("CHG-a", "Alpha"), plan("CHG-far", "Far", { here: false, branch: "other" })],
  archive: ARCHIVE10,
};

test("the dock tab bar matches the pane and highlights the current tab", async () => {
  const plain = await docked(FULL);
  assert.match(
    (await plain.dock())[1] ?? "",
    /^│ {2}1 Backlog 2 {3}2 Active 1 {3}3 Archive 10 +│$/,
  );
  const t = await docked(FULL, { mark: true });
  const marked = async () => /«([^»]*)»/.exec((await t.dock())[1] ?? "")?.[1];
  assert.equal(await marked(), " 2 Active 1 ");
  await t.shortcut("alt+r");
  t.key("1");
  assert.equal(await marked(), " 1 Backlog 2 ");
  t.key("3");
  assert.equal(await marked(), " 3 Archive 10 ");
});

test("the default dock tab is Active, else Backlog, else Archive, and shows only that tab", async () => {
  const active = await dockText(await docked(FULL));
  assert.match(active, /▸ Do it/);
  assert.doesNotMatch(active, /PLAN|CHG-a0/);
  const backlog = await dockText(await docked({ ...FULL, changes: [] }));
  assert.match(backlog, /PLAN Alpha/);
  assert.match(backlog, /PLAN Far CHG-far on other/);
  assert.doesNotMatch(backlog, /Do it|CHG-a0/);
  const archive = await dockText(await docked({ ...FULL, changes: [], backlog: [] }));
  assert.match(archive, /CHG-a0 done Shipped 0/);
  assert.doesNotMatch(archive, /PLAN|Do it/);
});

test("a tab the user picks sticks across refreshes", async () => {
  const t = await docked(FULL);
  await t.shortcut("alt+r");
  t.key("3");
  t.write("view.json", json({ ...FULL, changes: [change({ title: "Renamed" })] }));
  await t.fire("agent_settled");
  await waitFor(() => t.statuses().length === 2);
  assert.match(await dockText(t), /CHG-a0 done Shipped 0/);
  assert.doesNotMatch(await dockText(t), /Renamed/);
});

test("the dock and the modal pane share the current tab", async () => {
  const t = await docked(FULL);
  await t.shortcut("alt+r");
  t.key("3");
  let seen = "";
  t.queue(async (p) => {
    seen = text(p);
    press(p, "1");
    press(p, KEY.esc);
  });
  await t.command("rvow-status");
  assert.match(seen, /CHG-a0/, "the modal opened on the dock's Archive tab");
  assert.match(await dockText(t), /PLAN Alpha/, "a tab chosen in the modal shows in the dock");
});

test("tab keys are consumed only while the dock has focus", async () => {
  const t = await docked(FULL);
  await t.dock();
  for (const k of [
    "1",
    "2",
    "3",
    "\t",
    "\u001b[Z",
    "\u001b[D",
    "\u001b[C",
    "\u001b[5~",
    "\u001b[6~",
  ])
    assert.equal(t.key(k), undefined, JSON.stringify(k));
  await t.shortcut("alt+r");
  const themed = await docked(FULL, { mark: true });
  await themed.shortcut("alt+r");
  const tabOf = async () => /«([^»]*)»/.exec((await themed.dock())[1] ?? "")?.[1]?.trim();
  for (const [key, want] of [
    ["1", "1 Backlog 2"],
    ["2", "2 Active 1"],
    ["3", "3 Archive 10"],
    ["\t", "1 Backlog 2"],
    ["\u001b[Z", "3 Archive 10"],
    ["\u001b[D", "2 Active 1"],
    ["\u001b[C", "3 Archive 10"],
  ] as const) {
    assert.deepEqual(themed.key(key), { consume: true }, JSON.stringify(key));
    assert.equal(await tabOf(), want, JSON.stringify(key));
  }
});

test("j and k move over the rows of the current tab; pages jump", async () => {
  const t = await docked(FULL);
  await t.shortcut("alt+r");
  const on = async () => ((await t.dock()).find((l) => l.includes("›")) ?? "").replace(/\s+/g, " ");
  t.key("1");
  assert.match(await on(), /PLAN Alpha/);
  t.key("j");
  assert.match(await on(), /PLAN Far/);
  t.key("j");
  assert.match(await on(), /PLAN Far/, "stops at the last plan");
  t.key("3");
  assert.match(await on(), /Shipped 0/, "a new tab starts at its first row");
  t.key("j");
  assert.match(await on(), /Shipped 1/);
  t.key("\u001b[6~");
  assert.match(await on(), /Shipped 9/, "page down jumps and stops at the end");
  t.key("\u001b[5~");
  assert.match(await on(), /Shipped 1/);
  t.key("2");
  assert.match(await on(), /▸ Do it/);
});

test("a long Archive tab scrolls inside the height cap with an n-m/total marker", async () => {
  const t = await docked({ ...EMPTY, archive: ARCHIVE10 });
  const lines = await t.dock();
  assert.equal(lines.length, 10);
  assert.match(lines.at(-1) ?? "", /1-7\/10/);
  await t.shortcut("alt+r");
  for (let i = 0; i < 9; i++) t.key("j");
  const after = await t.dock();
  assert.equal(after.length, 10);
  assert.match(after.join("\n"), /› .*Shipped 9/);
  assert.match(after.at(-1) ?? "", /4-10\/10/);
  assert.doesNotMatch(after.join("\n"), /Shipped 0/);
});

test("an empty tab shows its message and never draws blank", async () => {
  const t = await docked({ ...EMPTY, archive: [] });
  await t.shortcut("alt+r");
  for (const [key, msg] of [
    ["1", /No plan in the backlog\./],
    ["2", /Nothing active on trunk\./],
    ["3", /Nothing archived yet\./],
  ] as const) {
    t.key(key);
    assert.match(await dockText(t), msg);
    assert.equal((await t.dock()).length, 4);
  }
});

test("the chosen dock tab is saved and restored in the next session", async () => {
  const t = await docked(FULL);
  await t.shortcut("alt+r");
  t.key("3");
  assert.equal(
    (t.entries.filter((e) => e.customType === "rvow-pane").at(-1)?.data as { tab?: string })?.tab,
    "archive",
  );
  const next = await docked(FULL, { entries: [...t.entries] });
  assert.match(await dockText(next), /CHG-a0 done Shipped 0/);
});

test("height rules hold on every tab", async () => {
  const many = (n: number) => Array.from({ length: n }, (_, i) => plan(`CHG-p${i}`, `Plan ${i}`));
  const tasks = Array.from({ length: 30 }, (_, i) => task(`TSK-${i}`, "missing"));
  const view = { ...EMPTY, backlog: many(30), archive: ARCHIVE10, changes: [change({ tasks })] };
  for (const [rows, cap] of [
    [60, 10],
    [24, 7],
    [20, 6],
  ] as const) {
    const t = await docked(view, { rows });
    await t.shortcut("alt+r");
    for (const k of ["1", "2", "3"]) {
      t.key(k);
      assert.equal((await t.dock()).length, cap, `tab ${k} at ${rows} rows`);
    }
  }
});

test("the focus hint lists the tab keys and shortens at narrow widths", async () => {
  const t = await docked(FULL);
  await t.shortcut("alt+r");
  assert.match((await t.dock()).at(-1) ?? "", /1-3 tabs {2}j\/k move.*esc editor/);
  const narrow = await docked(FULL, { cols: 50 });
  await narrow.shortcut("alt+r");
  const hint = (await narrow.dock(50)).at(-1) ?? "";
  assert.match(hint, /1-3 tabs.*esc editor/);
  assert.equal(visibleWidth(hint), 50);
});

type MouseOver = Partial<TuiMouseEvent>;

function at(lines: string[], text: string, nth = 0) {
  const hits = lines.flatMap((line, row) => {
    const plain = stripTerminalSequences(line);
    const i = plain.indexOf(text);
    return i < 0 ? [] : [{ row, col: visibleWidth(plain.slice(0, i)) }];
  });
  return hits[nth];
}

function event(x: number, y: number, width: number, height: number, over: MouseOver = {}) {
  const e: TuiMouseEvent = {
    type: "click",
    button: "left",
    x,
    y,
    screenX: x,
    screenY: y,
    width,
    height,
    shift: false,
    alt: false,
    ctrl: false,
    ...over,
  };
  return e;
}

/** Clicks `dx` columns into the first visible occurrence of `text`. */
function clickText(c: Rendered, text: string, over: MouseOver = {}, dx = 1, nth = 0, width = 100) {
  const lines = c.render(width);
  const pos = at(lines, text, nth);
  assert.ok(pos, `${text} is on screen`);
  return c.handleMouse?.(event(pos.col + dx, pos.row, width, lines.length, over));
}

const noEditor = (t: { calls: Call[] }) =>
  assert.deepEqual(
    t.calls.filter((c) => c.kind === "editor"),
    [],
  );

test("dock: clicking a tab label switches the tab and focuses the dock", async () => {
  const t = await docked(FULL, { mark: true, draft: "my draft" });
  const dock = await t.dockComponent();
  const marked = () => /«([^»]*)»/.exec(dock.render(100)[1] ?? "")?.[1]?.trim();
  assert.equal(marked(), "2 Active 1");
  assert.deepEqual(clickText(dock, "3 Archive"), { handled: true });
  assert.equal(marked(), "3 Archive 10");
  assert.match(dock.render(100).join("\n"), / focus /);
  clickText(dock, "1 Backlog");
  assert.equal(marked(), "1 Backlog 2");
  assert.match(dock.render(100).join("\n"), /PLAN Alpha/);
  clickText(dock, "2 Active");
  assert.equal(marked(), "2 Active 1");
  noEditor(t);
  assert.equal(t.editor.text, "my draft");
});

test("dock: tab click regions end at the label (wide titles do not shift them)", async () => {
  const t = await docked({ ...FULL, branch: "日本語ブランチ" });
  const dock = await t.dockComponent();
  const lines = dock.render(100);
  const pos = at(lines, "3 Archive");
  assert.ok(pos);
  const label = " 3 Archive 10 ".length;
  const send = (dx: number) =>
    dock.handleMouse?.(event(pos.col - 1 + dx, pos.row, 100, lines.length));
  assert.equal(send(-1), undefined, "the gap before the label");
  assert.deepEqual(send(0), { handled: true });
  assert.deepEqual(send(label - 1), { handled: true });
  assert.equal(send(label), undefined, "the gap after the label");
});

test("dock: a click on a row moves the cursor and focuses; a second click opens the modal", async () => {
  const t = await docked(
    FULL,
    { draft: "my draft" },
    { "show-TSK-2.txt": "TSK-2\ntitle: b\nstage: t2" },
  );
  const dock = await t.dockComponent();
  const cursor = () => (dock.render(100).find((l) => l.includes("›")) ?? "").replace(/\s+/g, " ");
  assert.equal(cursor(), "");
  clickText(dock, "title of TSK-2", {}, 3);
  assert.match(cursor(), /TSK-2/);
  assert.match(dock.render(100).join("\n"), / focus /);
  assert.equal(t.opened(), 0, "the first click only selects");
  let seen = "";
  t.queue(async (p) => {
    await until(p, /stage: t2/);
    seen = text(p);
    press(p, KEY.esc);
  });
  clickText(dock, "title of TSK-2", {}, 3);
  await waitFor(() => /stage: t2/.test(seen));
  assert.match(seen, /TASK {2}title of TSK-2/);
  noEditor(t);
  assert.equal(t.editor.text, "my draft");
});

test("dock: a double click (clickCount 2) opens the modal at once", async () => {
  const t = await docked(FULL, {}, { "show-CHG-1.txt": "CHG-1\ntitle: Do it\nstage: dbl" });
  const dock = await t.dockComponent();
  let seen = "";
  t.queue(async (p) => {
    await until(p, /stage: dbl/);
    seen = text(p);
    press(p, KEY.esc);
  });
  clickText(dock, "▸ Do it", { clickCount: 2 });
  await waitFor(() => /stage: dbl/.test(seen));
  assert.match(seen, /CHANGE {2}Do it/);
});

test("dock: the title tag focuses and unfocuses", async () => {
  const t = await docked(FULL);
  const dock = await t.dockComponent();
  assert.match(dock.render(100)[0] ?? "", /alt\+r focus/);
  clickText(dock, "alt+r focus");
  assert.match(dock.render(100)[0] ?? "", / focus ─╮/);
  clickText(dock, " focus ", {}, 2);
  assert.match(dock.render(100)[0] ?? "", /alt\+r focus/);
});

test("dock: the wheel scrolls the current tab, focused or not", async () => {
  const t = await docked({ ...EMPTY, archive: ARCHIVE10 });
  const dock = await t.dockComponent();
  const lines = () => dock.render(100).join("\n");
  assert.match(lines(), /Shipped 0/);
  for (let i = 0; i < 9; i++)
    clickText(dock, "Shipped", { type: "wheel", button: "none", wheelDelta: 1 });
  assert.match(lines(), /Shipped 9/);
  assert.doesNotMatch(lines(), /Shipped 0/);
  assert.match(dock.render(100).at(-1) ?? "", /4-10\/10/);
  for (let i = 0; i < 9; i++)
    clickText(dock, "Shipped", { type: "wheel", button: "none", wheelDelta: -1 });
  assert.match(lines(), /Shipped 0/);
});

test("dock: blank space, borders, other buttons and press/release do nothing", async () => {
  const t = await docked(FULL, { draft: "my draft" });
  const dock = await t.dockComponent();
  const before = dock.render(100).join("\n");
  const lines = dock.render(100);
  const send = (x: number, y: number, over: MouseOver = {}) =>
    dock.handleMouse?.(event(x, y, 100, lines.length, over));
  assert.equal(send(90, 1), undefined, "blank part of the tab bar");
  assert.equal(send(5, 0), undefined, "the title text");
  assert.equal(send(50, lines.length - 1), undefined, "the bottom border");
  assert.equal(send(5, 2, { button: "right" }), undefined);
  const pos = at(lines, "2 Active");
  assert.ok(pos);
  const press = send(pos.col + 3, pos.row, { type: "press" });
  assert.deepEqual(press, { handled: true, render: false });
  send(pos.col + 3, pos.row, { type: "release" });
  assert.equal(dock.render(100).join("\n"), before);
  noEditor(t);
  assert.equal(t.editor.text, "my draft");
});

test("dock: clicks reach the dock through pi's real container hit-testing", async () => {
  const t = await docked(FULL);
  const dock = await t.dockComponent();
  const box = new Container();
  box.addChild({ render: () => ["transcript one", "transcript two"], invalidate() {} });
  box.addChild(dock);
  const lines = box.render(100);
  const pos = at(lines, "3 Archive");
  assert.ok(pos);
  const r = box.handleMouse(event(pos.col + 3, pos.row, 100, lines.length));
  assert.equal(r?.handled, true);
  assert.match(dock.render(100).join("\n"), /CHG-a0 done Shipped 0/);
  assert.equal(
    box.handleMouse(event(3, 0, 100, lines.length)),
    undefined,
    "the transcript is not ours",
  );
});

test("dock: clicks are ignored while a modal or dialog owns the screen", async () => {
  const t = await docked(FULL, {}, { "show-CHG-1.txt": "CHG-1\ntitle: Do it\nstage: m" });
  const dock = await t.dockComponent();
  await t.shortcut("alt+r");
  let during: unknown = "unset";
  t.queue(async (p) => {
    await until(p, /stage: m/);
    during = clickText(dock, "3 Archive");
    press(p, KEY.esc);
  });
  t.key("\r");
  await waitFor(() => during !== "unset");
  assert.equal(during, undefined);
});

async function pane(view: object, files: Files = {}, ui: Ui = {}) {
  const t = setup({ "view.json": json(view), ...files }, ui);
  return t;
}

test("pane: tab labels are clickable", async () => {
  const t = await pane(FULL);
  await t.status((p) => {
    clickText(p, "3 Archive");
    assert.match(text(p), /CHG-a0/);
    clickText(p, "1 Backlog");
    assert.match(text(p), /PLAN {2}Alpha/);
    clickText(p, "2 Active");
    assert.match(text(p), /Do it/);
  });
  noEditor(t);
});

test("pane: plan card buttons do what their hotkeys do, on the card that was clicked", async () => {
  const two = { ...EMPTY, backlog: [plan("CHG-a", "A"), plan("CHG-b", "B")] };
  const accept = await pane(two, { "review.txt": "checklist" });
  await accept.status((p) => {
    assert.ok(clickText(p, "a Review & accept", {}, 2, 1), "the second card's button");
  });
  assert.deepEqual(decided(accept.argv()), ["accept CHG-b"]);
  assert.match(String(accept.calls.find((c) => c.kind === "confirm")?.args[1]), /checklist/);

  const refine = await pane(two);
  await refine.status((p) => void clickText(p, "e Refine with agent", {}, 2, 0));
  assert.deepEqual(refine.users(), [modRefine([plan("CHG-a", "A")])]);

  const pick = await pane(two);
  await pick.status((p) => {
    clickText(p, "s Select", {}, 2, 1);
    assert.match(text(p), /f Refine 1 selected/);
    assert.match(text(p), /s Unselect/);
    clickText(p, "f Refine 1 selected", {}, 2);
  });
  assert.deepEqual(pick.users(), [modRefine([plan("CHG-b", "B")])]);

  const detail = await pane(two, { "show-CHG-b.txt": "CHG-b\ntitle: B\nstage: clicked" });
  await detail.status(async (p) => {
    clickText(p, "d Details", {}, 2, 1);
    await until(p, /stage: clicked/);
    assert.match(text(p), /PLAN {2}B/);
    clickText(p, "b Back", {}, 2);
    assert.doesNotMatch(text(p), /stage: clicked/);
  });
});

test("pane: Take, Answer questions, Refresh and not-ready buttons", async () => {
  const away = plan("CHG-far", "Far", { here: false, branch: "other" });
  const take = await pane({ ...EMPTY, backlog: [away] });
  await take.status((p) => void clickText(p, "t Take to this branch", {}, 2));
  assert.deepEqual(decided(take.argv()), ["claim CHG-far"]);

  const asked = {
    questions: [{ ref: "Q1", question: "Which?", options: [{ label: "one" }] }],
    remaining: 0,
  };
  const withQ = { ...EMPTY, backlog: [plan("CHG-a", "A", { questions: 1 })] };
  const ask = await pane(withQ, { "clarify.json": json(asked) });
  await ask.status((p) => void clickText(p, "A Answer questions", {}, 2));
  assert.ok(ask.argv().includes("answer Q1 one --via dialog"));

  const notReady = { ...EMPTY, backlog: [plan("CHG-a", "A", { ready: false, tasks: 0 })] };
  const nr = await pane(notReady);
  await nr.status((p) => {
    clickText(p, "a Review & accept", {}, 2);
    assert.match(text(p), /Not ready: add at least one task/);
  });

  const fresh = await pane(WAITING({ id: "CHG-a", title: "A" }));
  await fresh.status(async (p) => {
    fresh.write(
      "view.json",
      json(WAITING({ id: "CHG-a", title: "A" }, { id: "CHG-b", title: "B" })),
    );
    clickText(p, "r Refresh", {}, 2);
    await until(p, /CHG-b/);
  });
});

test("pane: Archive, Review amendments and Details buttons on a change card", async () => {
  const done = await pane(DONE);
  await done.status((p) => void clickText(p, "x Archive", {}, 2));
  assert.deepEqual(decided(done.argv()), ["archive CHG-1"]);

  const view = { ...ACTIVE, changes: [change({ pending: [{ id: "AMD-1", reason: "add" }] })] };
  const amend = await pane(view, { "confirm.txt": "AMD-1 add" }, { rows: 60 });
  await amend.status((p) => void clickText(p, "c Review amendments", {}, 2));
  assert.ok(amend.argv().includes("confirm AMD-1 --accept"));

  const det = await pane(
    ACTIVE,
    { "show-CHG-1.txt": "CHG-1\ntitle: Do it\nstage: chg" },
    { rows: 60 },
  );
  await det.status(async (p) => {
    clickText(p, "d Details", {}, 2);
    await until(p, /stage: chg/);
    assert.match(text(p), /CHANGE {2}Do it/);
  });
});

test("pane: the Archive button inside a finished change's detail works", async () => {
  const t = await pane(DONE, { "show-CHG-1.txt": "CHG-1\ntitle: Do it\nstage: fin" });
  await t.status(async (p) => {
    press(p, "d");
    await until(p, /stage: fin/);
    clickText(p, "x Archive", {}, 2);
  });
  assert.deepEqual(decided(t.argv()), ["archive CHG-1"]);
});

test("pane: task rows and archive rows open like enter; a card body only focuses it", async () => {
  const t = await pane(
    ACTIVE,
    {
      "show-TSK-2.txt": "TSK-2\ntitle: b\nstage: task2",
      "show-CHG-xgnmc0.txt": "CHG-xgnmc0\nstage: arch",
    },
    { rows: 60 },
  );
  await t.status(async (p) => {
    clickText(p, "TSK-2 title of TSK-2", {}, 3);
    await until(p, /stage: task2/);
    assert.match(text(p), /TASK {2}title of TSK-2/);
    clickText(p, "b Back", {}, 2);
    clickText(p, "3 Archive");
    clickText(p, "CHG-xgnmc0", {}, 3);
    await until(p, /stage: arch/);
    clickText(p, "b Back", {}, 2);
    clickText(p, "2 Active");
    assert.match(focused(p), /Do it/);
    clickText(p, "Do it", {}, 2);
    assert.doesNotMatch(text(p), /TASK|CHANGE {2}/, "one click on a card body only focuses it");
  });
});

test("pane: a double click on a card opens it", async () => {
  const t = await pane(WAITING({ id: "CHG-a", title: "A" }), {
    "show-CHG-a.txt": "CHG-a\nstage: dbl",
  });
  await t.status(async (p) => {
    clickText(p, "waiting for your accept", { clickCount: 2 });
    await until(p, /stage: dbl/);
  });
});

test("pane: the wheel scrolls the body and the esc close hint closes", async () => {
  const lines = Array.from({ length: 80 }, (_, i) => `line ${i}`).join("\n");
  const t = await pane(WAITING({ id: "CHG-a", title: "A" }), {
    "show-CHG-a.txt": `CHG-a\n${lines}`,
  });
  await t.status(async (p) => {
    press(p, "d");
    await until(p, /line 0/);
    for (let i = 0; i < 5; i++)
      clickText(p, "line", { type: "wheel", button: "none", wheelDelta: 1 });
    assert.doesNotMatch(text(p), /line 0\b/);
    assert.match(text(p), /line 15/);
    clickText(p, "line", { type: "wheel", button: "none", wheelDelta: -1 });
    clickText(p, "esc close", {}, 2);
  });
  assert.equal(t.opened(), 1);
});

test("pane: regions are exact, survive wide characters and a narrow terminal", async () => {
  const wide = WAITING({ id: "CHG-a", title: "日本語のタイトル" });
  const t = await pane(wide, { "review.txt": "x", "show-CHG-a.txt": "CHG-a\nstage: w" });
  await t.status(async (p) => {
    for (const width of [100, 60, 40]) {
      const lines = p.render(width);
      const pos = at(lines, "d Details");
      assert.ok(pos, `Details at ${width}`);
      const send = (dx: number) =>
        p.handleMouse?.(event(pos.col + dx, pos.row, width, lines.length));
      assert.equal(send(-2)?.handled, true, "still inside the card (focuses it)");
      const before = text(p, width);
      assert.equal(send(0)?.handled, true);
      await until(p, /stage: w/, width);
      press(p, "b");
      assert.equal(text(p, width), before);
    }
  });
  noEditor(t);
});

test("pane: right clicks and clicks on blank space do nothing", async () => {
  const t = await pane(WAITING({ id: "CHG-a", title: "A" }), {}, { draft: "keep me" });
  await t.status((p) => {
    const lines = p.render(100);
    const before = text(p);
    assert.equal(p.handleMouse?.(event(50, 1, 100, lines.length)), undefined, "blank tab bar");
    assert.equal(p.handleMouse?.(event(3, 0, 100, lines.length)), undefined, "the border");
    const pos = at(lines, "a Review & accept");
    assert.ok(pos);
    assert.equal(
      p.handleMouse?.(event(pos.col + 2, pos.row, 100, lines.length, { button: "right" })),
      undefined,
    );
    assert.equal(text(p), before);
  });
  assert.deepEqual(decided(t.argv()), []);
  noEditor(t);
  assert.equal(t.editor.text, "keep me");
});

test("no mouse handler is needed in regular mode: keys still do everything", async () => {
  const t = await pane(WAITING({ id: "CHG-a", title: "A" }), { "review.txt": "x" });
  await t.status((p) => press(p, "a"));
  assert.deepEqual(decided(t.argv()), ["accept CHG-a"]);
});
