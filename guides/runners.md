# Guide: runners

Set `build` and `artifacts` on a runner when stale proofs cost too much. A proof goes stale when any file in `files:` changes. rvow then keeps it without a verify run when the built files are unchanged.

## When it helps

- It does not help a task with no build step, a task with `mutants:` or a private check, or the gate in a clean worktree.
- It does not fit a build whose files are outside the work tree or have hashed names, such as cargo test binaries. Use narrow `files:` and a targeted verify.

## Find the artifacts

- The artifacts are the built files that `run` executes, such as the test binary of one target. Add a file that the test reads.
- Do not guess a path. Build once and check that each path exists.
- A path is relative to the work tree. A glob such as `build/lib/*.so` hashes every match, and a new match runs the verify.

## Write the runner

- It goes in the project config. `run` builds and tests one target, `build` only builds it and `artifacts` lists its files. Set `build` and `artifacts` together.
- `{param}` fills `run`, `build` and `artifacts` alike. `env` applies to both commands.
- `extends` takes the keys of another runner. Lists add up and a set key wins.
- `always` lists cheap checks that run before each revalidation, such as `test ! -e <header>`. Artifacts do not cover source-text checks.

## One task per target

- Each task names one target: `verify: @cpp target=pool_test test=PoolShutdown`.
- Split a task that runs many binaries.

## Move a plan without weakening it

- Each ctest name still runs, with `--no-tests=error`. Drop no test and lower no limit.
- Put the full suite and the benchmarks in one last gate task or in `checks`.
- Change each verify with `rvow put`, then `rvow amend --reason`. The user confirms it.

## Check that it works

- Run `rvow prove --stale` once to record the hashes. Add a comment line to a file in the scope and run it again. The output says `revalidated`. Remove the line.

## Example

<!-- example -->
[runners.cpp]
run = "cmake --build build --target {target} && ctest --test-dir build -R '^{test}$' --no-tests=error"
build = "cmake --build build --target {target}"
artifacts = ["build/tests/{target}"]
<!-- /example -->
