import assert from "node:assert/strict";
import { test } from "node:test";
import {
  acceptTimeoutMs,
  baselineNotice,
  declinedAcceptReason,
  planBudget,
  repeatAcceptResult,
  shortReview,
} from "./rvow/tools.ts";

test("baselineNotice names the number of baseline checks", () => {
  assert.equal(baselineNotice(3), "rvow accept: 3 baseline checks; this can take a while");
  assert.equal(baselineNotice(1), "rvow accept: 1 baseline checks; this can take a while");
});

test("acceptTimeoutMs is at least the sum of the task timeouts", () => {
  assert.equal(acceptTimeoutMs(null), 600_000);
  // Four tasks of 10 minutes each: pi waits up to 40 minutes.
  assert.equal(
    acceptTimeoutMs({ tasks: 4, timeouts: [600, 600, 600, 600], defaultSecs: 600 }),
    2_400_000,
  );
  // A task without a timeout counts the configured default.
  assert.equal(acceptTimeoutMs({ tasks: 3, timeouts: [], defaultSecs: 300 }), 900_000);
  assert.equal(acceptTimeoutMs({ tasks: 2, timeouts: [120], defaultSecs: 300 }), 600_000);
});

test("planBudget reads the accept budget from the hook view", () => {
  const view = {
    backlog: [{ id: "CHG-a", tasks: 2, task_timeouts_secs: [600, 0], default_timeout_secs: 300 }],
  };
  assert.deepEqual(planBudget(view, "CHG-a"), { tasks: 2, timeouts: [600, 0], defaultSecs: 300 });
  assert.equal(planBudget(view, "CHG-b"), null);
  assert.equal(planBudget("not a view", "CHG-a"), null);
});

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
