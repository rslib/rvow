# Guide: proposal

Write the proposal first. Keep it to 150 words: why and what, never how.
Pipe it to `rvow put <change> proposal`; the hooks deny a direct write.

## Sections

- `branch:` names the git branch of the plan; rvow sets it from the current branch.
- Why: the problem and who has it, in one to three sentences.
- What changes: the result the user sees, as a short list.
- Out of scope: what this change leaves alone. Required, at least one item.
- Assumptions: the defaults you chose for open points.

## Rules

- Describe behavior, not code; a technical decision goes in `design.md` only when that guide allows it.
- A change that alters no behavior stays small: run `rvow new` without `--spec` and write tasks only, each with a real check.
- Link by change id instead of restating the proposal elsewhere.

## Interview the user

- First search the code and run `rvow spec list` and `rvow show <id>`. Ask only what changes the plan and what the code cannot answer.
- Mark each open point as `[NEEDS CLARIFICATION: question | recommended: answer | options: b; c | why: what it decides]`. Keep at most 3 in the change.
- Ask a round with the `rvow_ask` tool when your harness has it: it shows the user each question and records the answer, at most 3 questions per round. Otherwise run `rvow clarify --json`, ask in chat and record each answer with `rvow answer <ref> "<choice>"`.
- Ask scope and behavior first. After each answer, replace the block it changes with `rvow put`.
- Stop when no open point would change the plan or the user says to use your defaults; `rvow accept` refuses while a marker remains.

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
