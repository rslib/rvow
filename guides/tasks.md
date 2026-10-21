# Guide: tasks

List the tasks in work order. Pipe the whole file to `rvow put <change> tasks`, or one task block to `rvow put <task id>`.

## Fields

- Title: one line that says what the task delivers; the detail goes in the spec or the test (`long-task`).
- `covers:` the requirement ids the task delivers, ids only; a small change needs none.
- `verify:` the command that proves the task. Required.
- `files:` the paths the task changes. Required. Name files or one folder, not a source root with `**`. A proof goes stale when any scope file changes, so a file in 3 or more tasks makes them all stale at once (`hot-file`).
- `baseline: pass` only when the command is expected to pass before the work starts.
- `mutants:` an extra check after `verify:` passes, such as `cargo mutants --in-diff "$RVOW_DIFF"`.
- `timeout:` such as `90s`, `20m` or `1h`, only when the config default is too short.
- `max_age:` such as `24h`, only when a cached pass should expire sooner.
- `depends_on:` task ids, only when a change in their scope can break this one. A chain of 3 makes one edit stale for all (`depends-chain`).
- Use placeholder ids `TSK-new1`, `TSK-new2` and so on. rvow mints real ids at accept.

## The verify command

- It must fail before the work and pass after. Write the test, then run `rvow prove <task>` to record the failure. The one exception is `baseline: pass`.
- It runs real tests. A filter that matches nothing does not count, so name a test that exists.
- Do not use `true`, `echo` or a build alone (`weak-verify`).
- `verify: @<runner> key=value` runs a project runner; see `rvow guide runners`.
- Prove one target: build it and run its tests only, such as `cmake --build build --target test_a` then `ctest -R '^a$'`. A whole-preset build, or a `ctest` with no `-R`, slows every proof and gate run (`broad-verify`).
- Put the full build, the full suite, benchmarks and timing checks in one last gate task or in the project `checks`. A timing check in a task verify makes the gate flaky.
- Each task lands its own tests.

## Example

<!-- example -->
# Tasks

- TSK-new1 Store the output tail in the proof event
  covers: REQ-new1
  verify: cargo test -p rvow-core output_tail
  files: crates/rvow-core/src/proof.rs
<!-- /example -->
