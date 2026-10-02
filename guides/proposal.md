# Guide: proposal

Write `proposal.md` first. Keep it to 150 words. Say why and what, never how.

## Sections

- Why: the problem and who has it, in one to three sentences.
- What changes: the result the user can see, as a short list.
- Out of scope: what this change leaves alone. Required. Name at least one item.
- Assumptions: the defaults you chose for open points.

## Rules

- Describe behavior, not code. Put a technical decision in `design.md` and only when the design guide allows it.
- A change that alters no behavior stays small. Run `rvow new` without `--spec` and write tasks only. Do not invent a requirement to fill a form. Write a task with a real check instead.
- Do not restate the proposal in other files. Link to it by change id.

## Interview the user

- First search the code and run `rvow spec list` and `rvow show <id>`. Ask only what changes the plan and what the code cannot answer.
- Mark each open point as `[NEEDS CLARIFICATION: question | recommended: answer | options: b; c | why: what it decides]`. Keep at most 3 in the change.
- Ask a round with the `rvow_ask` tool when your harness has it: it shows the user each question and records the answer. Otherwise run `rvow clarify --json`, ask that round in chat and record each answer with `rvow answer <ref> "<choice>"`.
- Ask scope and behavior first. Hold a question that depends on an open answer for the next round.
- After each answer, edit the text the answer changes.
- Stop when no open point would change the plan, or when the user says to use your defaults. `rvow accept` refuses while a marker remains.

## Example

<!-- example -->
branch: feat/output-tail

# Keep the output tail of a proof run

## Why
A failed proof shows only an exit status. The user must run the command again to see why it failed.

## What changes
- `rvow prove` stores the last 20 lines of output in the proof event.
- `rvow status` prints that tail for a failed task.

## Out of scope
- Capture of the full output.
- Any change to the input fingerprint.

## Assumptions
- 20 lines is enough to show one test failure.
<!-- /example -->
