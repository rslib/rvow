import assert from "node:assert/strict";
import { test } from "node:test";
import { decisionPrompt } from "./rvow/prompts.ts";

test("an accept prompt does not double the period of the next line", () => {
  const text = decisionPrompt(["accept", "CHG-1"], "accepted\nnext: prove TSK-1 first.\n");
  assert.equal(text, "I accepted CHG-1. next: prove TSK-1 first. Continue with it.");
});

test("a reject prompt says a different edit, not a different change", () => {
  const text = decisionPrompt(["confirm", "--reject", "AMD-1"], "");
  assert.equal(
    text,
    "I rejected the plan edit AMD-1. Undo it, or amend it again with a different edit.",
  );
});
