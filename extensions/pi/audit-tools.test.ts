import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import type { ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";

const home = mkdtempSync(join(tmpdir(), "rvow-tools-"));
const bin = join(home, "binary.mjs");
writeFileSync(
  bin,
  `#!/usr/bin/env node\nimport fs from 'node:fs'; fs.writeFileSync(process.env.RVOW_ARGS, JSON.stringify(process.argv.slice(2))); console.log('literal');\n`,
);
chmodSync(bin, 0o755);
process.env.RVOW_BIN = bin;
process.env.RVOW_ARGS = join(home, "args.json");
const { viewArgs, proofArgs, serialQueue, registerTypedTools } = await import(
  "./rvow/typed-tools.ts"
);
const { run } = await import("./rvow/tools.ts");

function registered(execute?: typeof run) {
  const tools = new Map<string, ToolDefinition>();
  registerTypedTools(
    {
      registerTool: (tool: ToolDefinition) => tools.set(tool.name, tool),
    } as unknown as ExtensionAPI,
    execute,
  );
  return tools;
}

test("typed view and proof reject owner actions and open inputs", () => {
  assert.deepEqual(viewArgs({ target: "show", id: "REQ-abc123" }), [
    "show",
    "REQ-abc123",
    "--toon",
  ]);
  assert.deepEqual(proofArgs({ task: "TSK-abc123" }), ["prove", "TSK-abc123", "--toon"]);
  for (const target of ["accept", "confirm", "archive", "prove", "view; touch /tmp/no"]) {
    assert.throws(() => viewArgs({ target }));
  }
  for (const task of ["TSK-a; touch /tmp/no", "--stale", "$(id)", "CHG-a", "TSK-a\n"]) {
    assert.throws(() => proofArgs({ task }));
  }
  assert.throws(() => proofArgs({ task: "TSK-a", accept: true }));
  assert.throws(() => viewArgs({ target: "status", id: "CHG-a" }));
  assert.deepEqual(
    [...registered().keys()],
    ["rvow_view", "rvow_prove", "rvow_accept", "rvow_archive", "rvow_confirm", "rvow_yolo"],
  );
});

test("process execution passes literal argv with no shell", async () => {
  const args = ["show", "$(touch /tmp/rvow-no); space and ' quote", "--toon"];
  assert.equal((await run(args, home, "", 30_000))?.code, 0);
  assert.deepEqual(JSON.parse(readFileSync(join(home, "args.json"), "utf8")), args);
});

test("proof tools serialize the entire call and recover after failure", async () => {
  let active = 0;
  let max = 0;
  const order: string[] = [];
  const tools = registered(async (args) => {
    active++;
    max = Math.max(max, active);
    order.push(args[1] ?? "");
    await new Promise((resolve) => setTimeout(resolve, 20));
    active--;
    const failed = args[1] === "TSK-bad";
    return {
      code: failed ? 1 : 0,
      out: failed
        ? 'error: "no proof for TSK-bad"\ncode: unproved'
        : `task: ${args[1]}\nresult: proved\ntests: 1\nms: 4\nevent: ${"a".repeat(16)}`,
    };
  });
  const tool = tools.get("rvow_prove");
  assert.ok(tool);
  const execute = tool.execute;
  const ctx = { cwd: home } as Parameters<typeof execute>[4];
  const results = await Promise.allSettled(
    ["TSK-bad", "TSK-good", "TSK-next"].map((task) =>
      execute("call", { task }, new AbortController().signal, undefined, ctx),
    ),
  );
  assert.equal(max, 1);
  assert.deepEqual(order, ["TSK-bad", "TSK-good", "TSK-next"]);
  const statuses = results.map((r) =>
    r.status === "fulfilled" ? (r.value.isError ? "rejected" : "fulfilled") : r.status,
  );
  assert.deepEqual(statuses, ["rejected", "fulfilled", "fulfilled"]);
  const queue = serialQueue();
  await assert.rejects(
    queue(async () => {
      throw new Error("failed call");
    }),
  );
  assert.equal(await queue(async () => 42), 42);
});

test("installed folder loads every runtime module and named prompt exports", async () => {
  if (!process.env.RVOW_TEST_INSTALLED) return;
  const { default: extension } = await import(process.env.RVOW_TEST_INSTALLED);
  const tools: string[] = [];
  const events = new Map<string, (event: unknown, ctx: unknown) => unknown>();
  extension({
    on(name: string, handler: (event: unknown, ctx: unknown) => unknown) {
      events.set(name, handler);
    },
    registerCommand() {},
    registerShortcut() {},
    sendMessage() {},
    registerTool(tool: { name: string }) {
      tools.push(tool.name);
    },
  });
  const start = events.get("session_start");
  assert.ok(start);
  start({}, { cwd: home, hasUI: false, sessionManager: { getEntries: () => [] } });
  assert.deepEqual(tools.sort(), [
    "rvow_accept",
    "rvow_archive",
    "rvow_ask",
    "rvow_confirm",
    "rvow_judge",
    "rvow_prove",
    "rvow_view",
    "rvow_yolo",
  ]);
  const prompts = await import(join(process.env.RVOW_TEST_INSTALLED, "..", "prompts.ts"));
  assert.match(prompts.refinePrompt([{ id: "CHG-a", title: "A" }]), /CHG-a/);
});
