import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { compositeTuiLine, stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
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
  "hook claude view") [ -f "$RVOW_FAKE/slow-view" ] && sleep 0.5; cat "$RVOW_FAKE/view.json" 2>/dev/null ;;
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
type Rendered = { render: (width: number) => string[]; handleInput?: (data: string) => void };
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
  const slowView = "slow-view" in files;
  const api = {
    on: (name: string, h: Handler) => handlers.set(name, h),
    registerCommand: (name: string, c: Command) => commands.set(name, c),
    registerTool: (t: { name: string }) => tools.push(t.name),
    sendMessage: () => {},
    sendUserMessage: (...args: unknown[]) => void calls.push({ kind: "user", args }),
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
  const tui = { requestRender: () => {}, terminal: { rows: ui.rows ?? 40, columns: 100 } };
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
      setWidget: (...args: unknown[]) => void calls.push({ kind: "widget", args }),
      setStatus: (...args: unknown[]) => void calls.push({ kind: "status", args }),
      custom: (factory: Factory, opts: unknown) => {
        calls.push({ kind: "custom", args: [factory, opts] });
        return new Promise<unknown>((resolve) => {
          Promise.resolve(factory(tui, THEME, null, resolve)).then(async (pane) => {
            const loading = () => pane.render(100).join("").includes("loading rvow...");
            for (let i = 0; i < 300 && loading() && !slowView; i++)
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
  const status = (...steps: Step[]) => {
    script.push(...steps);
    return command("rvow-status");
  };
  const write = (name: keyof Files, text: string) => writeFileSync(join(home, name), text);
  return {
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
  for (let i = 0; i < 300; i++) {
    if (re.test(text(p, width))) return;
    await new Promise((r) => setTimeout(r, 10));
  }
  assert.match(text(p, width), re);
}

async function waitFor(cond: () => boolean) {
  for (let i = 0; i < 300 && !cond(); i++) await new Promise((r) => setTimeout(r, 10));
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

test("rvow-status with no view opens the pane and says rvow did not answer", async () => {
  const t = setup({});
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
  assert.equal(none.calls.filter((c) => c.kind === "widget").at(-1)?.args[1], undefined);

  const view = {
    ...WAITING({ id: "CHG-a", title: "A" }),
    needs_you: { amendments: 1, questions: 2, blocked: 0, plans: 1 },
  };
  const t = setup({ "view.json": json(view) });
  await t.fire("session_start");
  await waitFor(() => t.calls.some((c) => c.kind === "widget"));
  const lines = (
    await Promise.resolve(t.lastFactory("widget")(null, THEME, null, () => {}))
  ).render(120);
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
    assert.ok((performance.now() - start) / 50 < 20, "under 20 ms per key and render");
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
