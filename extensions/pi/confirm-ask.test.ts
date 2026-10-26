import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import type { ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";

const dir = mkdtempSync(join(tmpdir(), "rvow-confirm-ask-"));
after(() => rmSync(dir, { recursive: true, force: true }));
const bin = join(dir, "rvow");
const LIST =
  "AMD-a1 for CHG-a\n  weaken: TSK-1 verify changed\n  reason (from the agent): one\n" +
  "AMD-b2 for CHG-a\n  weaken: TSK-2 verify changed\n  reason (from the agent): two\n" +
  "digest def456\n";
writeFileSync(join(dir, "list.txt"), LIST);
writeFileSync(
  bin,
  `#!/bin/sh
echo "$*" >> "$RVOW_FAKE/argv.log"
case "$1" in
  confirm) if [ "$#" -eq 1 ] || [ "$2" = "--full" ]; then cat "$RVOW_FAKE/list.txt"; else echo "$2: decided"; fi ;;
  clarify) if [ -f "$RVOW_FAKE/clarify.json" ]; then cat "$RVOW_FAKE/clarify.json"; else echo '{"questions":[],"remaining":0}'; fi ;;
  decide) if [ "$2" = "--list" ]; then if [ -f "$RVOW_FAKE/decide.txt" ]; then cat "$RVOW_FAKE/decide.txt"; else echo "undecided: 0 tasks wait for a decision"; fi; else echo "$2: $3"; fi ;;
  *) ;;
esac
`,
);
chmodSync(bin, 0o755);
process.env.RVOW_BIN = bin;
process.env.RVOW_FAKE = dir;
const { default: extension } = await import("./rvow.ts");

const tools = new Map<string, ToolDefinition>();
const handlers = new Map<string, (event: unknown, ctx: unknown) => unknown>();
extension({
  on: (name: string, h: (event: unknown, ctx: unknown) => unknown) => handlers.set(name, h),
  registerCommand: () => {},
  registerTool: (value: ToolDefinition) => tools.set(value.name, value),
  getActiveTools: () => [],
  setActiveTools: () => {},
  registerShortcut: () => {},
  sendMessage: () => {},
  sendUserMessage: () => {},
  appendEntry: () => {},
} as unknown as ExtensionAPI);

await handlers.get("session_start")?.(
  {},
  {
    cwd: dir,
    hasUI: true,
    mode: "print",
    sessionManager: { getEntries: () => [], getSessionId: () => "pi-sess" },
    ui: { notify: () => {}, setStatus: () => {}, setWidget: () => {} },
  },
);

const argv = () => {
  try {
    return readFileSync(join(dir, "argv.log"), "utf8")
      .split("\n")
      .filter((l) => /^(confirm|clarify)/.test(l));
  } catch {
    return [];
  }
};
const clear = () => writeFileSync(join(dir, "argv.log"), "");
/** The `rvow decide` runs: the read-only list and each pick the user made. */
const decided = () => {
  try {
    return readFileSync(join(dir, "argv.log"), "utf8")
      .split("\n")
      .filter((l) => /^decide/.test(l));
  } catch {
    return [];
  }
};
/** Real `rvow decide --list` output: one stale relaxed task waits for the user's pick. */
const DECIDE_LIST =
  "undecided[1]{id,title}:\n  TSK-k8ergv,Write ok\n" +
  "help[1]:\n  Ask the user with the rvow_ask tool; each answer runs rvow decide <task> now|archive|keep";

function call(name: string, input: object, answer = true, asked: string[] = []) {
  const value = tools.get(name);
  assert.ok(value, name);
  const ctx = {
    cwd: dir,
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

test("confirm with an id asks only about that amendment and decides it by id", async () => {
  clear();
  const asked: string[] = [];
  const out = await call("rvow_confirm", { decision: "reject", id: "AMD-b2" }, true, asked);
  assert.equal(asked.length, 1);
  assert.match(asked[0] ?? "", /AMD-b2/);
  assert.doesNotMatch(asked[0] ?? "", /AMD-a1/);
  assert.deepEqual(argv(), ["confirm --full", "confirm AMD-b2 --reject"]);
  assert.match(JSON.stringify(out.content), /AMD-b2: decided/);
});

test("confirm with an id the user declines decides nothing", async () => {
  clear();
  const out = await call("rvow_confirm", { decision: "accept", id: "AMD-a1" }, false);
  assert.deepEqual(argv(), ["confirm --full"]);
  assert.match(JSON.stringify(out.content), /AMD-a1, so it is still pending/);
});

test("confirm with an id that is not pending says so and asks nothing", async () => {
  clear();
  const asked: string[] = [];
  const out = await call("rvow_confirm", { decision: "accept", id: "AMD-zz" }, true, asked);
  assert.deepEqual(asked, []);
  assert.match(JSON.stringify(out.content), /AMD-zz is not pending/);
});

test("confirm without an id keeps the digest path", async () => {
  clear();
  const asked: string[] = [];
  await call("rvow_confirm", { decision: "accept" }, true, asked);
  assert.match(asked[0] ?? "", /Accept all of these\?/);
  assert.deepEqual(argv(), ["confirm --full", "confirm --accept --all def456"]);
});

test("confirm refuses an id that is not an AMD id", async () => {
  await assert.rejects(call("rvow_confirm", { decision: "accept", id: "x; rm" }), /AMD id/);
});

test("ask with a change runs clarify with that id", async () => {
  clear();
  await call("rvow_ask", { change: "CHG-b" });
  assert.deepEqual(argv(), ["clarify CHG-b --json"]);
});

test("ask without a change runs clarify as before", async () => {
  clear();
  await call("rvow_ask", {});
  assert.deepEqual(argv(), ["clarify --json"]);
});

test("ask cuts long questions and options for the model, not for the dialog", async () => {
  const long = "q".repeat(500);
  const option = "o".repeat(500);
  writeFileSync(
    join(dir, "clarify.json"),
    JSON.stringify({
      questions: [{ ref: "Q1", question: long, options: [{ label: "yes", description: option }] }],
      remaining: 0,
    }),
  );
  const titles: string[] = [];
  const value = tools.get("rvow_ask");
  assert.ok(value);
  const ctx = (hasUI: boolean) =>
    ({
      cwd: dir,
      hasUI,
      ui: {
        select: async (title: string, keys: string[]) => {
          titles.push(title);
          return keys[0];
        },
      },
    }) as unknown as Parameters<typeof value.execute>[4];
  const run = (hasUI: boolean) =>
    value.execute("call", {}, new AbortController().signal, undefined, ctx(hasUI));
  const dialog = JSON.stringify((await run(true)).content);
  const chat = JSON.stringify((await run(false)).content);
  rmSync(join(dir, "clarify.json"));
  assert.equal(titles[0], long);
  assert.ok(dialog.includes(`${"q".repeat(200)}...`));
  assert.ok(!dialog.includes("q".repeat(201)));
  assert.ok(chat.includes(`${"q".repeat(200)}...`));
  assert.ok(chat.includes(`${"o".repeat(200)}...`));
  assert.ok(!chat.includes("o".repeat(201)));
});

test("ask asks the user to decide a stale task and records the pick", async () => {
  clear();
  writeFileSync(join(dir, "decide.txt"), DECIDE_LIST);
  const value = tools.get("rvow_ask");
  assert.ok(value);
  const asks: string[] = [];
  const ctx = {
    cwd: dir,
    hasUI: true,
    ui: {
      select: async (title: string, keys: string[]) => {
        asks.push(`${title} | ${keys.join(",")}`);
        return "Keep the pass";
      },
    },
  } as unknown as Parameters<typeof value.execute>[4];
  const done = await value.execute("call", {}, new AbortController().signal, undefined, ctx);
  rmSync(join(dir, "decide.txt"));
  assert.match(asks[0] ?? "", /TSK-k8ergv/);
  assert.match(asks[0] ?? "", /Write ok/);
  assert.match(asks[0] ?? "", /Re-prove now,Re-prove at archive,Keep the pass/);
  assert.deepEqual(decided(), ["decide --list", "decide TSK-k8ergv keep"]);
  assert.match(JSON.stringify(done.content), /TSK-k8ergv Write ok -> Keep the pass/);
});

test("a closed decide dialog records no decision", async () => {
  clear();
  writeFileSync(join(dir, "decide.txt"), DECIDE_LIST);
  const value = tools.get("rvow_ask");
  assert.ok(value);
  const ctx = {
    cwd: dir,
    hasUI: true,
    ui: { select: async () => undefined },
  } as unknown as Parameters<typeof value.execute>[4];
  const done = await value.execute("call", {}, new AbortController().signal, undefined, ctx);
  rmSync(join(dir, "decide.txt"));
  assert.deepEqual(decided(), ["decide --list"]);
  assert.match(JSON.stringify(done.content), /still waiting: TSK-k8ergv/);
});

test("ask with no question and no stale task says there is nothing to ask", async () => {
  clear();
  const done = await call("rvow_ask", {});
  assert.deepEqual(decided(), ["decide --list"]);
  assert.match(JSON.stringify(done.content), /no open questions/);
});

test("confirm with no input runs the plain short listing", async () => {
  clear();
  await call("rvow_confirm", {}, true);
  assert.deepEqual(argv(), ["confirm"]);
});

test("confirm with an id and no decision reads only that amendment", async () => {
  clear();
  await call("rvow_confirm", { id: "AMD-b2" }, true);
  assert.deepEqual(argv(), ["confirm AMD-b2"]);
});
