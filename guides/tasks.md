# Guide: tasks

Write `tasks.md` as a list of tasks. The order in the file is the work order.

## Fields

- Title: one line that says what the task delivers.
- `covers:` the requirement ids that the task delivers. Write ids only. Do not restate requirement text, because the spec holds it. A small change needs no `covers:`.
- `verify:` the command that proves the task. Required.
- `files:` the paths the task may touch. Required. Write `**` only when the task truly touches everything. Name the files the task changes: a proof goes stale when any file in its scope changes, and `rvow check` warns about a scope that covers more than 40 tracked files.
- `baseline: pass` only when the command is expected to pass before the work starts.
- `timeout:` such as `90s`, `20m` or `1h`, only when the config default is too short.
- `max_age:` such as `24h`, only when a cached rerun pass of this task should expire sooner than the config default.
- `depends_on:` task ids of the same change whose cache key joins this task's key, only when a change in another task's scope can break this one.
- Use placeholder ids `TSK-new1`, `TSK-new2` and so on. rvow mints real ids at accept.

## The verify command

- It must fail before the work and pass after. Run it before you write the code, and see it fail for the reason that the task fixes. The one exception is `baseline: pass`, a guard that must stay green.
- It runs real tests. A filter that matches nothing does not count, so name a test that exists. Write the test first if it does not.
- Do not use `true`, `echo` or a build alone as the check. Run the test that covers the behavior. `rvow check` and `rvow review` warn with `weak-verify` about a stub command, and about `baseline: pass` on a command that runs no counted test such as `cargo test`, `pytest` or `go test`.
- Each task lands its own tests. Do not defer tests to a final task.
- Keep each task to one reviewable step. Split a task when its `files:` scope covers unrelated areas.

## Example

<!-- example -->
# Tasks

- TSK-new1 Store the output tail in the proof event
  covers: REQ-new1
  verify: cargo test -p rvow-core output_tail
  files: crates/rvow-core/**
<!-- /example -->
