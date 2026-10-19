import assert from "node:assert/strict";
import { test } from "node:test";
import type { ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { ACCEPT_MS, GATE_MS, type run, SLOW_MS } from "./rvow/tools.ts";
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
    return args.length === 1 || args[1] === "--full"
      ? { code: 0, out: LIST }
      : { code: 0, out: "AMD-a1 accepted\n" };
  });
  const asked: string[] = [];
  const listed = await yoloCall(value, {}, { hasUI: false, asked });
  assert.match(JSON.stringify(listed.content), /digest abc123/);
  assert.deepEqual(asked, []);
  assert.deepEqual(seen, [["confirm"]]);

  await yoloCall(value, { decision: "accept" }, { hasUI: true, answer: true, asked });
  assert.match(asked[0] ?? "", /Accept all of these\?/);
  assert.deepEqual(seen.at(-1), ["confirm", "--accept", "--all", "abc123"]);

  seen.length = 0;
  const no = await yoloCall(value, { decision: "reject" }, { hasUI: true, answer: false });
  assert.deepEqual(seen, [["confirm", "--full"]]);
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

test("rvow_archive asks the user when a task is stale", async () => {
  const seen: string[][] = [];
  const value = tool("rvow_archive", async (args) => {
    seen.push(args);
    return args[0] === "hook"
      ? { code: 0, out: viewOf(["proved", "stale"]) }
      : { code: 0, out: "archived CHG-a1\n" };
  });
  const asked: string[] = [];
  await yoloCall(value, { change: "CHG-a1" }, { hasUI: true, answer: true, asked });
  assert.equal(asked.length, 1);
  assert.deepEqual(seen.at(-1), ["archive", "CHG-a1"]);
  for (const state of ["stale (inputs changed)"])
    assert.equal(archiveBlockers({ id: "C", tasks: [{ id: "T", state }], pending: [] }, "C"), null);
  for (const state of ["missing", "failed", "flaky", "blocked"])
    assert.match(
      archiveBlockers({ id: "C", tasks: [{ id: "T", state }], pending: [] }, "C") ?? "",
      new RegExp(`T is ${state}`),
    );
});

test("rvow_archive waits for the gate checks instead of the 60 s default", async () => {
  const waits = new Map<string, number>();
  const value = tool("rvow_archive", async (args, _cwd, _stdin, timeoutMs) => {
    waits.set(args[0] ?? "", timeoutMs);
    return args[0] === "hook"
      ? { code: 0, out: viewOf(["proved"]) }
      : { code: 0, out: "archived CHG-a1\n" };
  });
  await yoloCall(value, { change: "CHG-a1" }, { hasUI: true, answer: true });
  assert.equal(waits.get("archive"), GATE_MS);
  assert.ok(GATE_MS > SLOW_MS);
});

test("rvow_accept waits the fixed accept time, since accept runs no check", async () => {
  const waits = new Map<string, number>();
  const value = tool("rvow_accept", async (args, _cwd, _stdin, timeoutMs) => {
    waits.set(args[0] ?? "", timeoutMs);
    if (args[0] === "review") return { code: 0, out: "checklist\n" };
    return { code: 0, out: "accepted CHG-a1\n" };
  });
  await yoloCall(value, { change: "CHG-a1" }, { hasUI: true, answer: true });
  assert.equal(waits.get("accept"), ACCEPT_MS);
  assert.ok(!waits.has("hook"), "accept reads no plan budget");
});

test("rvow_archive names what is missing and asks nothing", async () => {
  for (const [states, pending, text] of [[["proved", "failed"], 0, /TSK-1 is failed/]] as const) {
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

const amd = (id: string, change: string, class_: string) =>
  `${id} for ${change}\n  ${class_}: TSK-1 something\n  reason (from the agent): why ${id}\n`;
const listing = (blocks: string[], digest = "d1") => `${blocks.join("")}digest ${digest}\n`;

function archiveFake(lists: string[], seen: string[][]) {
  let reads = 0;
  return tool("rvow_archive", async (args) => {
    seen.push(args);
    if (args[0] === "hook") return { code: 0, out: viewOf(["proved"], 3) };
    if (args[0] === "confirm" && args[1] === "--full")
      return { code: 0, out: lists[Math.min(reads++, lists.length - 1)] ?? "" };
    if (args[0] === "confirm") return { code: 0, out: "decided\n" };
    return { code: 0, out: "archived CHG-a1\n" };
  });
}

const three = listing([
  amd("AMD-a", "CHG-a1", "add"),
  amd("AMD-other", "CHG-zz", "weaken"),
  amd("AMD-b", "CHG-a1", "add"),
  amd("AMD-w", "CHG-a1", "weaken"),
]);

test("rvow_archive lists the change's amendments weaken first, accepts them by id, then archives", async () => {
  const seen: string[][] = [];
  const value = archiveFake([three], seen);
  const asked: string[] = [];
  const out = await yoloCall(value, { change: "CHG-a1" }, { hasUI: true, answer: true, asked });
  assert.equal(asked.length, 1);
  const text = asked[0] ?? "";
  assert.ok(text.indexOf("AMD-w") >= 0 && text.indexOf("AMD-w") < text.indexOf("AMD-a"));
  assert.ok(text.indexOf("AMD-a") < text.indexOf("AMD-b"));
  assert.doesNotMatch(text, /AMD-other/);
  assert.deepEqual(seen.slice(-2), [
    ["confirm", "AMD-w", "AMD-a", "AMD-b", "--accept"],
    ["archive", "CHG-a1"],
  ]);
  assert.equal((out.details as { kind: string }).kind, "decision");
});

test("rvow_archive on a no leaves the amendments pending and names /rvow-archive", async () => {
  const seen: string[][] = [];
  const value = archiveFake([three], seen);
  const out = await yoloCall(value, { change: "CHG-a1" }, { hasUI: true, answer: false });
  assert.ok(seen.every((a) => a[0] === "hook" || (a[0] === "confirm" && a[1] === "--full")));
  assert.match(JSON.stringify(out.content), /\/rvow-archive CHG-a1/);
  assert.match(JSON.stringify(out.content), /pending/);
});

test("rvow_archive asks again when an amendment is recorded before the answer", async () => {
  const seen: string[][] = [];
  const four = listing(
    [
      amd("AMD-a", "CHG-a1", "add"),
      amd("AMD-b", "CHG-a1", "add"),
      amd("AMD-w", "CHG-a1", "weaken"),
      amd("AMD-d", "CHG-a1", "add"),
    ],
    "d2",
  );
  const three3 = listing([
    amd("AMD-a", "CHG-a1", "add"),
    amd("AMD-b", "CHG-a1", "add"),
    amd("AMD-w", "CHG-a1", "weaken"),
  ]);
  const value = archiveFake([three3, four, four, four], seen);
  const asked: string[] = [];
  await yoloCall(value, { change: "CHG-a1" }, { hasUI: true, answer: true, asked });
  assert.equal(asked.length, 2);
  assert.doesNotMatch(asked[0] ?? "", /AMD-d/);
  assert.match(asked[1] ?? "", /AMD-d/);
  const accepts = seen.filter((a) => a[0] === "confirm" && a[1] !== "--full");
  assert.equal(accepts.length, 1);
  assert.ok(accepts[0]?.includes("AMD-d"));
});

test("rvow_archive gives up when the list keeps changing and accepts nothing", async () => {
  const seen: string[][] = [];
  let n = 0;
  const value = tool("rvow_archive", async (args) => {
    seen.push(args);
    if (args[0] === "hook") return { code: 0, out: viewOf(["proved"], 1) };
    if (args[0] === "confirm" && args[1] === "--full")
      return { code: 0, out: listing([amd(`AMD-n${n++}`, "CHG-a1", "add")]) };
    return { code: 0, out: "ok\n" };
  });
  const out = await yoloCall(value, { change: "CHG-a1" }, { hasUI: true, answer: true });
  assert.ok(!seen.some((a) => a[0] === "archive" || (a[0] === "confirm" && a[1] !== "--full")));
  assert.match(JSON.stringify(out.content), /\/rvow-archive CHG-a1/);
});

test("rvow_archive does not archive when the accept fails", async () => {
  const seen: string[][] = [];
  const value = tool("rvow_archive", async (args) => {
    seen.push(args);
    if (args[0] === "hook") return { code: 0, out: viewOf(["proved"], 1) };
    if (args[1] === "--full") return { code: 0, out: listing([amd("AMD-a", "CHG-a1", "add")]) };
    return { code: 1, out: "error: nope\n" };
  });
  const out = await yoloCall(value, { change: "CHG-a1" }, { hasUI: true, answer: true });
  assert.ok(!seen.some((a) => a[0] === "archive"));
  assert.match(JSON.stringify(out.content), /nope/);
});

test("rvow_archive with no pending amendment asks only whether to archive", async () => {
  const seen: string[][] = [];
  const value = tool("rvow_archive", async (args) => {
    seen.push(args);
    return args[0] === "hook"
      ? { code: 0, out: viewOf(["proved"], 0) }
      : { code: 0, out: "archived CHG-a1\n" };
  });
  const asked: string[] = [];
  await yoloCall(value, { change: "CHG-a1" }, { hasUI: true, answer: true, asked });
  assert.equal(asked.length, 1);
  assert.doesNotMatch(asked[0] ?? "", /AMD-/);
  assert.deepEqual(
    seen.map((a) => a[0]),
    ["hook", "archive"],
  );
});

const reviewLines = (n: number) =>
  `${Array.from({ length: n }, (_, i) => `checklist line ${i + 1}`).join("\n")}\n`;

test("rvow_accept cuts a long review in the dialog and names rvow review", async () => {
  const value = tool("rvow_accept", async (args) =>
    args[0] === "review"
      ? { code: 0, out: reviewLines(40) }
      : { code: 0, out: "accepted CHG-m1\n" },
  );
  const asked: string[] = [];
  await yoloCall(value, { change: "CHG-m1" }, { hasUI: true, answer: true, asked });
  assert.match(asked[0] ?? "", /checklist line 10/);
  assert.match(asked[0] ?? "", /30 more lines/);
  assert.match(asked[0] ?? "", /rvow review CHG-m1/);
  assert.doesNotMatch(asked[0] ?? "", /checklist line 11/);
});

test("a failed accept is not asked again while the plan is unchanged", async () => {
  let accepts = 0;
  const value = tool("rvow_accept", async (args) => {
    if (args[0] === "review") return { code: 0, out: reviewLines(40) };
    if (args[0] === "accept") {
      accepts++;
      return { code: 1, out: "error: CHG-m2 has no passing check\ncode: not-ready\n" };
    }
    return { code: 0, out: "null\n" };
  });
  const asked: string[] = [];
  const first = await yoloCall(value, { change: "CHG-m2" }, { hasUI: true, answer: true, asked });
  assert.match(JSON.stringify(first.content), /not-ready/);
  const second = await yoloCall(value, { change: "CHG-m2" }, { hasUI: true, answer: true, asked });
  // One dialog for both calls, and accept ran once: the repeat returns the old reason.
  assert.equal(asked.length, 1);
  assert.equal(accepts, 1);
  assert.match(JSON.stringify(second.content), /not-ready/);
  assert.match(JSON.stringify(second.content), /\/rvow-accept CHG-m2/);
});

test("a declined accept is not asked again while the plan is unchanged", async () => {
  let accepts = 0;
  const value = tool("rvow_accept", async (args) => {
    if (args[0] === "review") return { code: 0, out: reviewLines(40) };
    accepts++;
    return { code: 0, out: "accepted CHG-m3\n" };
  });
  const asked: string[] = [];
  const first = await yoloCall(value, { change: "CHG-m3" }, { hasUI: true, answer: false, asked });
  assert.match(JSON.stringify(first.content), /did not accept CHG-m3/);
  const second = await yoloCall(value, { change: "CHG-m3" }, { hasUI: true, answer: false, asked });
  assert.equal(asked.length, 1);
  assert.equal(accepts, 0);
  assert.match(JSON.stringify(second.content), /did not accept CHG-m3/);
});

test("a changed plan after a failure asks again", async () => {
  let reviews = 0;
  const value = tool("rvow_accept", async (args) => {
    if (args[0] === "review") {
      reviews++;
      return { code: 0, out: reviews === 1 ? reviewLines(40) : `${reviewLines(40)}one more\n` };
    }
    return { code: 1, out: "error: CHG-m4 has no passing check\ncode: not-ready\n" };
  });
  const asked: string[] = [];
  await yoloCall(value, { change: "CHG-m4" }, { hasUI: true, answer: true, asked });
  await yoloCall(value, { change: "CHG-m4" }, { hasUI: true, answer: true, asked });
  assert.equal(asked.length, 2);
});
