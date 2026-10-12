# rvow

Rust CLI (`crates/rvow`, library `crates/rvow-core`) that holds a coding agent to an accepted plan, plus a Claude Code mod and a pi extension in `extensions/`.

## Commands

Rust, from the repository root (the toolchain is pinned in `rust-toolchain.toml`):

```sh
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo test --locked <name>          # one test, by name substring
```

`cargo test --all-targets` also runs the pi audit tests, which call `npm --prefix extensions run typecheck:pi` and `test:pi:*`, so run `npm --prefix extensions ci` first.

Extensions, from the repository root (run `npm --prefix extensions ci` once):

```sh
npm --prefix extensions run check        # all of the below, in this order
npm --prefix extensions run drift        # installed pi and claude versions match the pinned types
npm --prefix extensions run lint         # biome
npm --prefix extensions run typecheck:pi
npm --prefix extensions run test:pi      # node --test, needs Node 22.18 or newer
npm --prefix extensions run typecheck:claude
npm --prefix extensions run validate:claude
npm --prefix extensions run test:claude  # the whole suite, there is no filter

```

The claude steps skip with exit 0 when the `claude` CLI is missing, so a skip proves nothing.
`.github/scripts/quickstart.sh` runs the README quickstart and `.github/scripts/install.sh` runs `rvow install`, each against the `rvow` on `PATH`, in a throwaway repository and home.
`prek run` runs the same checks as the git hook, on staged files only.
CI (`.github/workflows/ci.yml`) runs the same commands; keep the two in step.

Agent evals (needs the `claude` CLI and a built binary):

```sh
evals/agent/run.sh haiku --case next-task --runs 1   # one live case, a few cents
evals/agent/run.sh                                   # the full suite, capped at $5
```

## Where things go

| Area | Home | Rule |
|------|------|------|
| Command logic | `crates/rvow-core/src/<topic>.rs` | The CLI owns the truth; hooks are thin adapters. |
| Command bodies and output | `crates/rvow/src/cmd/<topic>.rs` | `crates/rvow/src/main.rs` holds only the clap types and dispatch. |
| Plan-writing guides | `guides/*.md` | Embedded by `crates/rvow-core/src/guide.rs`. |
| Agent skill | `skills/rvow/SKILL.md.tmpl` | Rendered by `crates/rvow/src/skill.rs` at install. Write each command as `{{cmd:<args>}}`; a command or flag the CLI lacks fails the render and `cargo test`. |
| Claude mod | `extensions/claude/hooks/` | Embedded by `crates/rvow-core/src/install.rs`. |
| pi extension | `extensions/pi/rvow/*.ts` (`extensions/pi/rvow.ts` only re-exports it) | `extensions/pi/rvow/index.ts` is `PI_EXTENSION`; every other file is in `PI_MODULES` of `crates/rvow-core/src/install.rs`. |
| Agent evals | `evals/agent/` | `evals/agent/run.sh` installs the mod into a throwaway home; each case holds `case.yaml`, `prompt.md`, `setup.sh` and `graders/`; `lib.sh` holds the shared setup helpers. |

Update this section in the change that moves an area.

## Contracts across layers

- The version lives once, in `[workspace.package]` of `Cargo.toml`. Both crates use `version.workspace = true`.
- Every file the installer writes is embedded with `include_str!`. A new module under `extensions/claude/hooks/` needs one entry in `MOD_MODULES` in `crates/rvow-core/src/install.rs`; the install test fails when one is missing.
- Every file under `extensions/pi/rvow/` other than `extensions/pi/rvow/index.ts` needs one entry in `PI_MODULES` in `crates/rvow-core/src/install.rs`, like `MOD_MODULES`; a file that is missing fails at pi load on an installed copy.
- The `'__RVOW_BIN__'` placeholder stays in `extensions/claude/hooks/register.tsx`, where the installer replaces it.
- The `version` in `extensions/claude/.claude-plugin/plugin.json` is overwritten at install.

## Rust rules

- No `.unwrap()` or `.expect()` outside tests. Return `rvow_core::error::Error::new(code, message, fix)`: a stable short `code`, a `message` that names the path or id, and a `fix` that is one next step.
- Errors and results print on stdout as TOON for agents (`crates/rvow/src/output.rs`). Usage errors exit 2, failures exit 1.
- A fixed set of values is an enum, not a string or an int.
- Rust source is ASCII only; the hook and CI reject any other byte.
- Add a dependency with `cargo add`.

## Claude mod rules

- The plugin engine follows `$` and `atom()` state only inside the file that declares them. Keep every function that takes `$`, and every atom, in `extensions/claude/hooks/register.tsx`; only pure code moves to other files.
- The sandbox has no `setTimeout`. Use `later($, ...)` or `$.clock`.
- `prompt.submit` from inside `command.run` is refused. Defer it with `later`.
- `extensions/claude/.claude-plugin/types` is generated and git-ignored; never edit it. `npm --prefix extensions run typecheck:claude` regenerates it. `extensions/claude/types/index.d.ts` is different: it is tracked, hand-written and holds the rvow view types.
- Strict undefined checks apply: `claude plugin test` can pass while `tsc` fails.

## Tests and proof

- A bug fix adds a test that fails without the fix.
- CLI behavior tests live in `crates/rvow/tests/cli.rs`. They run in temporary git repositories, so the git hook must not leak `GIT_*` variables into them (see `.pre-commit-config.yaml`).
- The native pi codemode proof is skipped unless `RVOW_NATIVE_SDK` is set. From `extensions/`, with a built binary: `RVOW_NATIVE_SDK=$PWD/node_modules/@earendil-works/pi-coding-agent RVOW_NATIVE_BIN=<repo>/target/debug/rvow node --test pi/native-codemode.test.ts`.
- A change under `extensions/claude` is proven by `npm --prefix extensions run typecheck:claude` and `npm --prefix extensions run test:claude` together.

## Traps

- `rvow` on `PATH` is the installed copy. A CLI change reaches the hooks only after `cargo install --path crates/rvow --locked`.
- The pre-push gate reruns tasks in a clean worktree with no `extensions/node_modules`. Each contributor sets `rerun_links = ["extensions/node_modules"]` and `cache_with_links = true` in the project `config.toml` (see `docs/settings.md` for where it lives), or the push fails and the cache stays off. The Claude version pin only warns outside CI.
- `PLAN.md` is git-ignored on purpose. Do not add it.
- The global git ignore lists `AGENTS.md`, so `.gitignore` carries `!AGENTS.md`. Stage new files by name.

## Boundaries

- Never edit the ledger, the enforcement files or the tests to make a check pass. Fix the code, or use `rvow block` with a reason.
- Never weaken a verify command to make it pass.
- Ask before changing the CLI flags or the output shape that the hooks and the skill parse.
