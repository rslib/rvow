import assert from "node:assert/strict";
import { test } from "node:test";
import { acceptTimeoutMs, baselineNotice, planBudget } from "./rvow/tools.ts";

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
