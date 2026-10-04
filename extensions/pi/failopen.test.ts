import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const dir = mkdtempSync(join(tmpdir(), "rvow-failopen-"));
const bin = join(dir, "rvow");
writeFileSync(
  bin,
  `#!/bin/sh
cat > /dev/null
case "$(cat "$RVOW_FAKE/mode" 2>/dev/null)" in
  crash) exit 3 ;;
  badjson) echo "not json" ;;
  hang) exec sleep 30 ;;
  stop) case "$3" in stop) echo '{"reason":"keep going"}' ;; esac ;;
esac
`,
);
chmodSync(bin, 0o755);
process.env.RVOW_BIN = bin;
process.env.RVOW_FOCUS_SHORTCUT = "not a chord";
process.env.RVOW_FAKE = dir;
const { default: extension } = await import("./rvow.ts");

type Handler = (event: unknown, ctx: unknown) => unknown;

function setup(mode: string) {
  writeFileSync(join(dir, "mode"), mode);
  const handlers = new Map<string, Handler>();
  const shortcuts: string[] = [];
  const notes: { text: string; level: unknown }[] = [];
  extension({
    on: (name: string, h: Handler) => handlers.set(name, h),
    registerCommand: () => {},
    registerTool: () => {},
    registerShortcut: (key: string) => void shortcuts.push(key),
    sendMessage: () => {},
    sendUserMessage: () => {},
    appendEntry: () => {},
  } as unknown as ExtensionAPI);
  const ctx = {
    cwd: dir,
    hasUI: true,
    mode: "print",
    sessionManager: { getEntries: () => [] },
    ui: {
      notify: (text: string, level: unknown) => void notes.push({ text, level }),
      setStatus: () => {},
      setWidget: () => {},
    },
  };
  const fire = (name: string, event: unknown) => handlers.get(name)?.(event, ctx);
  return { fire, shortcuts, notes };
}

const edit = { toolName: "edit", input: { path: "a.ts" } };
const off = (notes: { text: string; level: unknown }[]) =>
  notes.filter((n) => n.text.startsWith("rvow enforcement is off"));

test("an invalid RVOW_FOCUS_SHORTCUT falls back to alt+r with a notice and the extension loads", async () => {
  const t = setup("ok");
  assert.deepEqual(t.shortcuts, ["alt+r"]);
  await t.fire("session_start", {});
  assert.equal(
    t.notes.filter((n) => n.text.includes("RVOW_FOCUS_SHORTCUT 'not a chord'")).length,
    1,
  );
  assert.equal(await t.fire("tool_call", edit), undefined);
});

for (const mode of ["crash", "badjson"]) {
  test(`tool_call allows and says once per session that rvow is off (${mode})`, async () => {
    const t = setup(mode);
    await t.fire("session_start", {});
    assert.equal(await t.fire("tool_call", edit), undefined);
    assert.equal(await t.fire("tool_call", edit), undefined);
    assert.equal(off(t.notes).length, 1);
    assert.equal(off(t.notes)[0]?.level, "warning");
    await t.fire("session_start", {});
    await t.fire("tool_call", edit);
    assert.equal(off(t.notes).length, 2);
  });
}

test("a healthy rvow that prints nothing allows without a notice", async () => {
  const t = setup("ok");
  await t.fire("session_start", {});
  assert.equal(await t.fire("tool_call", edit), undefined);
  assert.equal(off(t.notes).length, 0);
});

test("session_start resets the settle continue counter", async () => {
  const t = setup("stop");
  const settle = () => t.fire("agent_before_settle", { entries: [] });
  for (let i = 0; i < 8; i++) assert.ok(await settle());
  assert.equal(await settle(), undefined);
  await t.fire("session_start", {});
  assert.ok(await settle());
});

test("a hung rvow cannot hold the settle or the context handler for long", async () => {
  const t = setup("hang");
  await t.fire("session_start", {});
  const start = Date.now();
  assert.equal(await t.fire("agent_before_settle", { entries: [] }), undefined);
  assert.ok(Date.now() - start < 9_000, "settle waited past the short budget");
  assert.equal(off(t.notes).length, 1);
  await t.fire("session_start", {});
  await t.fire("session_compact", {});
  const again = Date.now();
  assert.equal(await t.fire("context", { messages: [] }), undefined);
  assert.ok(Date.now() - again < 9_000, "context waited past the short budget");
  assert.equal(off(t.notes).length, 2);
});
