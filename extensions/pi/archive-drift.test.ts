import assert from "node:assert/strict";
import { test } from "node:test";
import type { ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";
import type { run } from "./rvow/tools.ts";
import { driftAmendArgs, driftReason, registerTypedTools } from "./rvow/typed-tools.ts";

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

function ask(value: ToolDefinition, input: object, answer: boolean, asked: string[]) {
  const ctx = {
    cwd: "/tmp",
    hasUI: true,
    ui: {
      confirm: async (_title: string, message: string) => {
        asked.push(message);
        return answer;
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

const WEAKEN = { class: "weaken", what: "test a.test.ts changed t1" };

const view = (drift: object[], pending: object[]) =>
  JSON.stringify({
    branch: "main",
    changes: [
      {
        id: "CHG-a1",
        title: "T",
        tasks: [{ id: "TSK-1", title: "t", state: "proved" }],
        drift,
        pending,
      },
    ],
    backlog: [],
    archive: [],
    questions: [],
    next: null,
    needs_you: { amendments: 0, questions: 0, blocked: 0, plans: 0 },
  });

const LISTING =
  "AMD-n1 for CHG-a1\n  weaken: test a.test.ts changed t1\n  reason (from the agent): recorded\ndigest d1\n";

test("rvow_archive records drift itself, then asks once and archives", async () => {
  const seen: string[][] = [];
  const value = tool("rvow_archive", async (args) => {
    seen.push(args);
    if (args[0] === "hook") return { code: 0, out: view([WEAKEN], []) };
    if (args[0] === "confirm" && args[1] === "--full") return { code: 0, out: LISTING };
    return { code: 0, out: "ok\n" };
  });
  const asked: string[] = [];
  await ask(value, { change: "CHG-a1" }, true, asked);
  const amend = seen.find((a) => a[0] === "amend") ?? [];
  assert.deepEqual(amend.slice(0, 1), ["amend"]);
  assert.ok(amend.includes("--reason") && amend.includes("--change") && amend.includes("CHG-a1"));
  assert.match(amend.join(" "), /test a\.test\.ts changed t1/);
  assert.equal(asked.length, 1);
  assert.match(asked[0] ?? "", /AMD-n1/);
  const steps = seen.map((a) => a.join(" "));
  const at = (prefix: string) => steps.findIndex((l) => l.startsWith(prefix));
  assert.ok(at("amend") < at("confirm AMD-n1 --accept"));
  assert.ok(at("confirm AMD-n1 --accept") < at("archive"));
});

test("rvow_archive does not record drift that a pending plan edit already holds", async () => {
  const seen: string[][] = [];
  const value = tool("rvow_archive", async (args) => {
    seen.push(args);
    if (args[0] === "hook")
      return { code: 0, out: view([WEAKEN], [{ id: "AMD-n1", reason: "r" }]) };
    if (args[0] === "confirm" && args[1] === "--full") return { code: 0, out: LISTING };
    return { code: 0, out: "ok\n" };
  });
  await ask(value, { change: "CHG-a1" }, true, []);
  assert.equal(seen.filter((a) => a[0] === "amend").length, 0);
});

test("rvow_archive records no amendment when nothing drifted", async () => {
  const seen: string[][] = [];
  const value = tool("rvow_archive", async (args) => {
    seen.push(args);
    return args[0] === "hook" ? { code: 0, out: view([], []) } : { code: 0, out: "archived\n" };
  });
  await ask(value, { change: "CHG-a1" }, true, []);
  assert.equal(seen.filter((a) => a[0] === "amend").length, 0);
  assert.deepEqual(seen.at(-1), ["archive", "CHG-a1"]);
});

test("rvow_archive stops with the failure when the amend fails", async () => {
  const seen: string[][] = [];
  const value = tool("rvow_archive", async (args) => {
    seen.push(args);
    if (args[0] === "hook") return { code: 0, out: view([WEAKEN], []) };
    if (args[0] === "amend") return { code: 1, out: "error: no plan edit\n" };
    return { code: 0, out: "ok\n" };
  });
  const asked: string[] = [];
  const out = await ask(value, { change: "CHG-a1" }, true, asked);
  assert.equal(out.isError, true);
  assert.deepEqual(asked, []);
  assert.equal(seen.filter((a) => a[0] === "archive").length, 0);
});

test("driftAmendArgs keeps blocking items only, and none while an edit is pending", () => {
  const free = { class: "free", what: "a comment" };
  const one = (over: object) => ({ id: "CHG-a1", pending: [], ...over });
  assert.equal(driftAmendArgs(undefined, "CHG-a1"), null);
  assert.equal(driftAmendArgs(one({ drift: [free] }), "CHG-a1"), null);
  assert.equal(driftAmendArgs(one({ drift: [WEAKEN], pending: [{}] }), "CHG-a1"), null);
  assert.equal(driftAmendArgs(one({}), "CHG-a1"), null);
  const args = driftAmendArgs(one({ drift: [WEAKEN, free] }), "CHG-a1") ?? [];
  assert.deepEqual([args[0], args[1]], ["amend", "--reason"]);
  assert.deepEqual(args.slice(3), ["--change", "CHG-a1"]);
  assert.match(args[2] ?? "", /test a\.test\.ts changed t1/);
  assert.ok(driftReason([WEAKEN]).length < 600);
});
