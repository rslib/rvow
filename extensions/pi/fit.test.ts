import assert from "node:assert/strict";
import { test } from "node:test";
import { truncateToWidth } from "@earendil-works/pi-tui";
import { FIT_CACHE_MAX, fit, fitCacheSize, fitMisses } from "./rvow/formats.ts";

test("fit gives what truncateToWidth gives, for short, long and colored rows", () => {
  for (const [s, n] of [
    ["abc", 10],
    ["a much longer row than the width allows", 12],
    ["\u001b[31mred\u001b[0m text", 6],
    ["", 4],
  ] as const) {
    assert.equal(fit(s, n), truncateToWidth(s, n, "...", true), `${JSON.stringify(s)} at ${n}`);
  }
});

test("fit cuts a row to a width once and reuses it", () => {
  const before = fitMisses();
  fit("a row the cache has not seen before", 20);
  fit("a row the cache has not seen before", 20);
  fit("a row the cache has not seen before", 20);
  assert.equal(fitMisses() - before, 1, "three calls cut the row once");
  fit("a row the cache has not seen before", 21);
  assert.equal(fitMisses() - before, 2, "another width is another row");
});

test("fit keeps at most FIT_CACHE_MAX rows", () => {
  for (let i = 0; i < FIT_CACHE_MAX + 50; i++) fit(`row ${i}`, 30);
  assert.ok(fitCacheSize() <= FIT_CACHE_MAX, `${fitCacheSize()} rows kept`);
});
