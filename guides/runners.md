# Guide: runners

Use a runner with `build` and `artifacts` when stale proofs cost too much. Many tasks go stale on each edit and each verify rebuilds a lot. rvow then keeps a stale proof without a verify run when the built files are unchanged.

## When it helps

- A proof goes stale when any file in `files:` changes, also when the test binary stays the same.
- It does not help a task with no build step, a task with `mutants:` or a private check, or the gate in a clean worktree.

## Find the artifacts

- The artifacts are the built files that `run` executes, such as the test binary of one target. Add a file that the test reads and the build does not make, such as a data file.
- Do not guess a path. Build the target once, list the build dir and check that each path exists and is a file.
- A path is relative to the work tree and holds no `..`.

## Write the runner

- It goes in the project config. `run` builds and tests one target, `build` only builds it and `artifacts` lists its files.
- `build` and `artifacts` use the same `{param}` as `run`. Set both or neither.
- Keep `run` and `build` on the same build dir, so `build` leaves what `run` executes.

## One task per target

- Each task names one target: `verify: @cpp target=pool_test test=PoolShutdown`.
- Split a task that runs many test binaries. Or write a runner that names those targets and artifacts.

## Move a plan without weakening it

- Run the same tests as before. Each ctest name still runs, with `--no-tests=error`.
- Do not drop a test, lower a limit or remove a check. Put the full suite and the benchmarks in one last gate task or in `checks`.
- Change each verify with `rvow put`, then record it with `rvow amend --reason`. The user confirms it.

## Check that it works

- Run `rvow prove --stale` once. It records the hashes.
- Add a comment line to a file in the scope and run it again. The output says `revalidated`. Then remove the line.

## Example

<!-- example -->
[runners.cpp]
run = "cmake --build build --target {target} && ctest --test-dir build -R '^{test}$' --no-tests=error"
build = "cmake --build build --target {target}"
artifacts = ["build/tests/{target}"]
<!-- /example -->
