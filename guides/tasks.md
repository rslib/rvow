# Guide: tasks

Write `tasks.md` as a list of tasks in work order. Pipe the whole file to `rvow put <change> tasks`, or one task block to `rvow put <task id>`.

## Fields

- Title: one line that says what the task delivers.
- `covers:` the requirement ids the task delivers, ids only; a small change needs none.
- `verify:` the command that proves the task. Required.
- `files:` the paths the task may touch. Required. Avoid `**`: a proof goes stale when any file in scope changes, and `rvow check` warns past 40 files.
- `baseline: pass` only when the command is expected to pass before the work starts.
- `mutants:` an extra check after `verify:` passes, such as `cargo mutants --in-diff "$RVOW_DIFF"`. Set it when the tests could pass without proving anything. `RVOW_DIFF` holds the task's diff against the base commit.
- `timeout:` such as `90s`, `20m` or `1h`, when the config default is too short.
- `max_age:` such as `24h`, when a cached rerun pass should expire sooner.
- `depends_on:` task ids whose cache key joins this task's key, when a change in their scope can break this one.
- Use placeholder ids `TSK-new1`, `TSK-new2` and so on. rvow mints real ids at accept.

## The verify command

- It must fail before the work and pass after. Run it before you write the code and see it fail. The one exception is `baseline: pass`, a guard that stays green.
- It runs real tests. A filter that matches nothing does not count, so name a test that exists.
- Do not use `true`, `echo` or a build alone. `rvow check` and `rvow review` warn with `weak-verify` about a stub command, and about `baseline: pass` with no counted test runner.
- A verify can name a project runner: `verify: @<runner> key=value` runs the runner's `run` command from the project config, with the keys filled. See `docs/settings.md`.
- Keep the full suite in the project `checks` and each task verify targeted, so accept stays fast. `rvow check` warns with `full-suite`.
- Each task lands its own tests.
- Keep each task to one reviewable step; split one whose scope covers unrelated areas.

## Example

<!-- example -->
# Tasks

- TSK-new1 Store the output tail in the proof event
  covers: REQ-new1
  verify: cargo test -p rvow-core output_tail
  files: crates/rvow-core/**
<!-- /example -->
