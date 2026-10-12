import assert from "node:assert/strict";
import { test } from "node:test";
import type { ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";
import type { run } from "./rvow/tools.ts";
import {
  archiveBlockers,
  proofArgs,
  registerTypedTools,
  viewArgs,
  YOLO_DESCRIPTION,
  yoloArgs,
} from "./rvow/typed-tools.ts";

function tool(name: string, execute: typeof run) {
  const tools = new Map<string, ToolDefinition>();
  registerTypedTools(
    {
      registerTool: (value: ToolDefinition) => tools.set(value.name, value),
    } as unknown as ExtensionAPI,
    execute,
  );
  const value = tools.get(name);
  assert.ok(value);
  return value;
}

function call(value: ToolDefinition, input: object, signal = new AbortController().signal) {
  return value.execute("call", input, signal, undefined, { cwd: "/tmp" } as Parameters<
    typeof value.execute
  >[4]);
}

const dashboard = {
  branch: "trunk",
  changes: [],
  backlog: [],
  archive: [],
  questions: [],
  next: null,
  needs_you: { amendments: 0, questions: 0, blocked: 0, plans: 0 },
};

test("view and status use existing JSON flags and expose data to scripts", async () => {
  for (const target of ["view", "status"] as const) {
    const argv = target === "view" ? ["hook", "view"] : ["status", "--json"];
    assert.deepEqual(viewArgs({ target }), argv);
    const value = tool("rvow_view", async (args) => {
      assert.deepEqual(args, argv);
      return { code: 0, out: JSON.stringify(dashboard) };
    });
    assert.ok(value.outputSchema);
    const result = await call(value, { target });
    assert.deepEqual(result.details, { kind: target, code: 0, data: dashboard });
    assert.deepEqual(result.structuredContent, result.details);
  }
  const result = await call(
    tool("rvow_view", async () => ({ code: 0, out: "null\n" })),
    {},
  );
  assert.deepEqual(result.details, { kind: "view", code: 0, data: null });
});

test("show declares an artifact-text variant rather than claiming parsed data", async () => {
  const text = "lines[1]:\n  artifact";
  const result = await call(
    tool("rvow_view", async () => ({ code: 0, out: text })),
    { target: "show", id: "REQ-abc" },
  );
  assert.deepEqual(result.details, { kind: "artifact-text", code: 0, id: "REQ-abc", text });
});

test("proof preserves controlled TOON fields for success and failed verification", async () => {
  assert.deepEqual(proofArgs({ task: "TSK-abc" }), ["prove", "TSK-abc", "--toon"]);
  for (const code of [0, 1]) {
    const verdict = code ? '"failed: exit 1"' : "proved";
    const value = tool("rvow_prove", async () => ({
      code,
      out: `task: TSK-abc\nresult: ${verdict}\ntests: null\nms: 42\nevent: 7f00b0a1b2c3d4e5\nnext: done\n`,
    }));
    const result = await call(value, { task: "TSK-abc" });
    assert.deepEqual(result.details, {
      kind: "proof",
      code,
      task: "TSK-abc",
      result: code ? "failed: exit 1" : "proved",
      tests: null,
      duration_ms: 42,
      event: "7f00b0a1b2c3d4e5",
    });
    assert.deepEqual(result.structuredContent, result.details);
    assert.equal(result.isError ?? false, code !== 0);
  }
});

test("CLI failure details survive and the proof queue recovers", async () => {
  let count = 0;
  const value = tool("rvow_prove", async () =>
    ++count === 1
      ? { code: 1, out: 'error: "no task: TSK-abc"\ncode: missing\nhelp: read status' }
      : { code: 0, out: "task: TSK-abc\nresult: proved\ntests: 3\nms: 4\nevent: a1b2c3d4e5f60718" },
  );
  const failed = await call(value, { task: "TSK-abc" });
  assert.equal(failed.isError, true);
  assert.deepEqual(failed.details, {
    kind: "failure",
    code: 1,
    error: "no task: TSK-abc",
    cliCode: "missing",
    stdout: 'error: "no task: TSK-abc"\ncode: missing\nhelp: read status',
  });
  assert.deepEqual(failed.structuredContent, failed.details);
  assert.equal((await call(value, { task: "TSK-abc" })).isError, undefined);
});

test("invalid successful output fails closed and aborted queued proof never executes", async () => {
  await assert.rejects(
    call(
      tool("rvow_view", async () => ({ code: 0, out: "{}" })),
      {},
    ),
    /invalid structured/,
  );
  let unblock = () => {};
  let executions = 0;
  const pending = new Promise<void>((resolve) => {
    unblock = resolve;
  });
  const value = tool("rvow_prove", async () => {
    executions++;
    await pending;
    return {
      code: 0,
      out: "task: TSK-abc\nresult: proved\ntests: 3\nms: 4\nevent: a1b2c3d4e5f60718",
    };
  });
  const first = call(value, { task: "TSK-abc" });
  const controller = new AbortController();
  const second = call(value, { task: "TSK-def" }, controller.signal);
  controller.abort();
  const rejected = assert.rejects(second, /cancelled before execution/);
  unblock();
  await first;
  await rejected;
  assert.equal(executions, 1);
});

function yoloCall(
  value: ToolDefinition,
  input: object,
  ui: { hasUI: boolean; answer?: boolean; asked?: string[] },
) {
  const ctx = {
    cwd: "/tmp",
    hasUI: ui.hasUI,
    ui: {
      confirm: async (_title: string, message: string) => {
        ui.asked?.push(message);
        return ui.answer ?? false;
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

test("yolo arguments are exact and refuse anything else", () => {
  assert.deepEqual(yoloArgs({}), ["yolo", "--toon"]);
  assert.deepEqual(yoloArgs({ tier: "auto" }), ["yolo", "auto", "--toon"]);
  assert.deepEqual(yoloArgs({ tier: "audit", for: "30m" }), [
    "yolo",
    "audit",
    "--for",
    "30m",
    "--toon",
  ]);
  for (const input of [
    { tier: "root" },
    { for: "2h" },
    { tier: "off", for: "2h" },
    { tier: "auto", for: "soon" },
    { x: 1 },
  ])
    assert.throws(() => yoloArgs(input), /rvow:/);
  for (const word of ["audit", "auto", "bypass", "off", "gate still checks", "rvow block"])
    assert.ok(YOLO_DESCRIPTION.includes(word), word);
});

test("a yolo tier is switched on only after the user allows it", async () => {
  const seen: string[][] = [];
  const value = tool("rvow_yolo", async (args) => {
    seen.push(args);
    return { code: 0, out: "tier: auto\nminutes: 30\n" };
  });
  const asked: string[] = [];
  const yes = await yoloCall(
    value,
    { tier: "auto", for: "30m" },
    { hasUI: true, answer: true, asked },
  );
  assert.deepEqual(seen, [["yolo", "auto", "--for", "30m", "--toon"]]);
  assert.match(asked[0] ?? "", /yolo auto for 30m.*gate still checks at push/);
  assert.deepEqual(yes.details, { kind: "yolo", code: 0, tier: "auto", minutes: 30 });

  seen.length = 0;
  const no = await yoloCall(value, { tier: "bypass" }, { hasUI: true, answer: false });
  assert.deepEqual(seen, []);
  assert.match(JSON.stringify(no.content), /declined/);
  await assert.rejects(
    yoloCall(value, { tier: "auto" }, { hasUI: false }),
    /no one is here to ask/,
  );
  assert.deepEqual(seen, []);
});

test("yolo off and the status need no dialog", async () => {
  const seen: string[][] = [];
  const value = tool("rvow_yolo", async (args) => {
    seen.push(args);
    return { code: 0, out: "tier: off\n" };
  });
  const asked: string[] = [];
  await yoloCall(value, { tier: "off" }, { hasUI: false, asked });
  await yoloCall(value, {}, { hasUI: false, asked });
  assert.deepEqual(seen, [
    ["yolo", "off", "--toon"],
    ["yolo", "--toon"],
  ]);
  assert.deepEqual(asked, []);
});

const LIST = "AMD-a1 weaken T1 verify changed\ndigest abc123\n";

test("rvow_accept shows the review and accepts only when the user says yes", async () => {
  const seen: string[][] = [];
  const value = tool("rvow_accept", async (args) => {
    seen.push(args);
    return args[0] === "review"
      ? { code: 0, out: "the checklist\n" }
      : { code: 0, out: "accepted CHG-a1\nnext: TSK-1 go\n" };
  });
  const asked: string[] = [];
  const yes = await yoloCall(value, { change: "CHG-a1" }, { hasUI: true, answer: true, asked });
  assert.match(asked[0] ?? "", /the checklist[\s\S]*Accept CHG-a1\?/);
  assert.deepEqual(seen, [
    ["review", "CHG-a1"],
    ["accept", "CHG-a1", "--yes"],
  ]);
  assert.equal((yes.details as { kind: string }).kind, "decision");

  seen.length = 0;
  const no = await yoloCall(value, { change: "CHG-a1" }, { hasUI: true, answer: false });
  assert.deepEqual(seen, [["review", "CHG-a1"]]);
  assert.match(JSON.stringify(no.content), /did not accept CHG-a1/);
  assert.match(JSON.stringify(no.content), /\/rvow-accept CHG-a1/);
  await assert.rejects(yoloCall(value, { change: "CHG-a1" }, { hasUI: false }), /no one is here/);
  await assert.rejects(
    yoloCall(value, { change: "x; rm -rf /" }, { hasUI: true }),
    /complete CHG id/,
  );
});

test("a failed accept comes back to the agent with the reason", async () => {
  const value = tool("rvow_accept", async (args) =>
    args[0] === "review"
      ? { code: 0, out: "checklist\n" }
      : { code: 1, out: "error: CHG-a1 has no passing check\ncode: not-ready\n" },
  );
  const out = await yoloCall(value, { change: "CHG-a1" }, { hasUI: true, answer: true });
  assert.equal((out.details as { kind: string }).kind, "failure");
  assert.match(JSON.stringify(out.content), /no passing check[\s\S]*not-ready/);
  assert.equal((out as { isError?: boolean }).isError, true);

  const review = tool("rvow_accept", async () => ({ code: 1, out: "error: no such change\n" }));
  const asked: string[] = [];
  const failed = await yoloCall(review, { change: "CHG-zz" }, { hasUI: true, answer: true, asked });
  assert.deepEqual(asked, []);
  assert.match(JSON.stringify(failed.content), /no such change/);
});

test("rvow_confirm lists without a dialog and decides with one", async () => {
  const seen: string[][] = [];
  const value = tool("rvow_confirm", async (args) => {
    seen.push(args);
    return args.length === 1 ? { code: 0, out: LIST } : { code: 0, out: "AMD-a1 accepted\n" };
  });
  const asked: string[] = [];
  const listed = await yoloCall(value, {}, { hasUI: false, asked });
  assert.match(JSON.stringify(listed.content), /digest abc123/);
  assert.deepEqual(asked, []);

  await yoloCall(value, { decision: "accept" }, { hasUI: true, answer: true, asked });
  assert.match(asked[0] ?? "", /Accept all of these\?/);
  assert.deepEqual(seen.at(-1), ["confirm", "--accept", "--all", "abc123"]);

  seen.length = 0;
  const no = await yoloCall(value, { decision: "reject" }, { hasUI: true, answer: false });
  assert.deepEqual(seen, [["confirm"]]);
  assert.match(JSON.stringify(no.content), /still pending/);
  assert.match(JSON.stringify(no.content), /\/rvow-confirm/);
});

const viewOf = (states: string[], pending = 0) =>
  JSON.stringify({
    branch: "main",
    changes: [
      {
        id: "CHG-a1",
        title: "T",
        tasks: states.map((state, i) => ({ id: `TSK-${i}`, title: "t", state })),
        pending: Array.from({ length: pending }, () => ({ id: "AMD-1", reason: "r" })),
      },
    ],
    backlog: [],
    archive: [],
    questions: [],
    next: null,
    needs_you: { amendments: 0, questions: 0, blocked: 0, plans: 0 },
  });

test("rvow_archive asks only when every task is proved, and archives on a yes", async () => {
  const seen: string[][] = [];
  const value = tool("rvow_archive", async (args) => {
    seen.push(args);
    return args[0] === "hook"
      ? { code: 0, out: viewOf(["proved", "proved"]) }
      : { code: 0, out: "archived CHG-a1\n" };
  });
  const asked: string[] = [];
  const out = await yoloCall(value, { change: "CHG-a1" }, { hasUI: true, answer: true, asked });
  assert.match(asked[0] ?? "", /Every task of CHG-a1 is proved/);
  assert.deepEqual(seen.at(-1), ["archive", "CHG-a1"]);
  assert.equal((out.details as { kind: string }).kind, "decision");

  seen.length = 0;
  const no = await yoloCall(value, { change: "CHG-a1" }, { hasUI: true, answer: false });
  assert.deepEqual(seen, [["hook", "view"]]);
  assert.match(JSON.stringify(no.content), /stays active/);
  assert.match(JSON.stringify(no.content), /\/rvow-archive CHG-a1/);
  await assert.rejects(yoloCall(value, { change: "CHG-a1" }, { hasUI: false }), /no one is here/);
});

test("rvow_archive names what is missing and asks nothing", async () => {
  for (const [states, pending, text] of [
    [["proved", "stale"], 0, /TSK-1 is stale/],
    [["proved"], 1, /pending plan edit/],
  ] as const) {
    const value = tool("rvow_archive", async () => ({
      code: 0,
      out: viewOf([...states], pending),
    }));
    const asked: string[] = [];
    const out = await yoloCall(value, { change: "CHG-a1" }, { hasUI: true, answer: true, asked });
    assert.match(JSON.stringify(out.content), text);
    assert.deepEqual(asked, []);
  }
  assert.match(archiveBlockers(undefined, "CHG-x") ?? "", /not an active change/);
  assert.equal(
    archiveBlockers({ id: "CHG-a", tasks: [{ id: "T", state: "proved" }], pending: [] }, "CHG-a"),
    null,
  );
});
