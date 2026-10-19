import assert from "node:assert/strict";
import { test } from "node:test";
import { declinedAcceptReason, repeatAcceptResult, shortReview } from "./rvow/tools.ts";

const lines = (n: number) =>
  `${Array.from({ length: n }, (_, i) => `checklist line ${i + 1}`).join("\n")}\n`;

test("shortReview shows a short review in full, with no cut line", () => {
  const review = lines(6);
  assert.equal(shortReview(review, "CHG-a1"), review);
});

test("shortReview cuts a long review to 10 lines and names rvow review", () => {
  const out = shortReview(lines(40), "CHG-a1");
  const shown = out.split("\n");
  assert.deepEqual(shown.slice(0, 10), lines(40).split("\n").slice(0, 10));
  assert.equal(shown.length, 11);
  assert.match(shown[10] ?? "", /30 more lines/);
  assert.match(shown[10] ?? "", /rvow review CHG-a1/);
});

test("repeatAcceptResult names the slash command the user can run", () => {
  assert.match(repeatAcceptResult("the user declined", "CHG-a1"), /\/rvow-accept CHG-a1/);
  assert.match(declinedAcceptReason("CHG-a1"), /did not accept CHG-a1/);
});
