import assert from "node:assert/strict";
import { test } from "node:test";

test("a run that times out says so instead of returning nothing", async () => {
  // BIN is read at import, so set it before the module loads.
  process.env.RVOW_BIN = "sleep";
  const { run } = await import("./rvow/tools.ts");
  const r = await run(["5"], process.cwd(), "", 200);
  assert.equal(r?.code, 1);
  assert.match(r?.out ?? "", /rvow 5: stopped after 0 s with no result; run it in a terminal/);
});
