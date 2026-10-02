---
name: rvow
description: Work on a change that the user accepted with rvow (a vow). Use when the session brief names an rvow change or task, when a task needs proof, when the plan must change, or when a task cannot pass as written.
---

# rvow

The user and you agreed on a plan, and rvow holds you to it with checks outside the model.
The brief at session start names the next task. `rvow brief` prints it again.

## Work

- Do the next task, then run `rvow prove <task>`. rvow runs the stored verify command. Never run the check yourself and claim it passed.
- A pass with 0 tests run is not a proof. Make the test exist and run.
- `rvow status` shows each task and any drift. `rvow prove --stale` re-proves stale tasks.
- Stay inside each task's `files:` scope. The gate fails on files outside it.

## Before accept

- While a plan waits for accept, code edits are denied. Run `rvow guide proposal` and follow its interview steps: `rvow clarify --json`, ask with AskUserQuestion, record with `rvow answer`.
- Then run `rvow check` and ask the user to run `/rvow-accept <id>`.

## When the plan must change

- Edit the plan files, then run `rvow amend --reason "..."` and ask the user to run `/rvow-confirm` (or `! rvow confirm`).
- To change a protected test or test setup file, run `rvow amend --unlock <path> --reason "..."` first and wait for the user.
- You never run `rvow accept` or `rvow confirm`. The hooks deny them.

## Stop and report

When a task cannot pass as written, run `rvow block <task> --reason "..."` and tell the user why.
Never weaken a verify command, edit a test to make it pass, special-case the tested input or skip a hook.

## Notes

- `rvow note decision "..."` records a choice the next session needs.
- `rvow note handoff "..."` leaves at most 5 lines for the next session.
