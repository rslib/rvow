import assert from "node:assert/strict";
import { test } from "node:test";
import type { ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { acceptable, blocker, planButtons } from "./rvow/formats.ts";
import type { run } from "./rvow/tools.ts";
import { archiveBlockers, findChange, registerTypedTools } from "./rvow/typed-tools.ts";
import type { Plan } from "./rvow/types.ts";

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

const change = (states: string[], id = "CHG-a1") => ({
  id,
  title: "T",
  tasks: states.map((state, i) => ({ id: `TSK-${i}`, title: "t", state })),
  pending: [],
});

const view = (changes: object[], elsewhere?: object[]) =>
  JSON.stringify({
    branch: "main",
    changes,
    ...(elsewhere ? { elsewhere } : {}),
    backlog: [],
    archive: [],
    questions: [],
    next: null,
    needs_you: { amendments: 0, questions: 0, blocked: 0, plans: 0 },
  });

const plan = (over: Partial<Plan>): Plan => ({
  id: "CHG-p1",
  title: "A plan",
  branch: "feat/x",
  here: false,
  ready: true,
  tasks: 1,
  questions: 0,
  issues: 0,
  ...over,
});

test("rvow_archive archives a change that lives on another branch", async () => {
  const seen: string[][] = [];
  const value = tool("rvow_archive", async (args) => {
    seen.push(args);
    return args[0] === "hook"
      ? { code: 0, out: view([], [change(["proved", "stale"])]) }
      : { code: 0, out: "archived CHG-a1\n" };
  });
  const asked: string[] = [];
  await ask(value, { change: "CHG-a1" }, true, asked);
  assert.match(asked[0] ?? "", /Every task of CHG-a1 is proved/);
  assert.deepEqual(seen.at(-1), ["archive", "CHG-a1"]);
});

test("rvow_archive still names a change that no worktree holds", async () => {
  const value = tool("rvow_archive", async () => ({ code: 0, out: view([]) }));
  const asked: string[] = [];
  const out = await ask(value, { change: "CHG-a1" }, true, asked);
  assert.match(JSON.stringify(out.content), /not an active change/);
  assert.match(JSON.stringify(out.content), /any worktree/);
  assert.deepEqual(asked, []);
});

test("findChange looks on this branch first, then in the other worktrees", () => {
  const here = change(["proved"]);
  const there = change(["stale"], "CHG-b2");
  const v = { changes: [here], elsewhere: [there] };
  assert.equal(findChange(v, "CHG-a1"), here);
  assert.equal(findChange(v, "CHG-b2"), there);
  assert.equal(findChange({ changes: [here] }, "CHG-b2"), undefined);
  assert.equal(archiveBlockers(findChange(v, "CHG-b2"), "CHG-b2"), null);
});

test("a plan whose branch has a worktree can be accepted from any branch", () => {
  assert.equal(acceptable(plan({ here: true })), true);
  assert.equal(acceptable(plan({ worktree: "/w" })), true);
  assert.equal(acceptable(plan({ worktree: null })), false);
  assert.equal(acceptable(plan({})), false);
  assert.equal(acceptable(plan({ worktree: "/w", ready: false, questions: 1 })), false);
  assert.equal(blocker(plan({})), "take it to this branch first");
  assert.equal(blocker(plan({ worktree: "/w", questions: 1 })), "answer the open questions first");
  const accept = (p: Plan) => planButtons(p, false, false).find((b) => b.key === "a");
  assert.equal(accept(plan({ worktree: "/w" }))?.primary, true);
  assert.equal(accept(plan({}))?.primary, false);
});
