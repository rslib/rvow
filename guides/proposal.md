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
- Mark an open point as `[NEEDS CLARIFICATION: question | recommended: answer]`. Keep at most 3 in the change.
- Ask the user one open point at a time and offer the recommended answer. Write each answer into the proposal at once.
- Move a point that the user does not answer into Assumptions, with the recommended answer. `rvow accept` refuses while a marker remains.
- Do not restate the proposal in other files. Link to it by change id.

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
