# rvow

Rust CLI (`crates/rvow`, libraries `crates/rvow-*`) that holds a coding agent to an accepted plan, plus a Claude Code mod and a pi extension in `extensions/`.

## Commands

Rust, from the repository root (the toolchain is pinned in `rust-toolchain.toml`):

```sh
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo nextest run --locked                         # the suite; each test runs in its own process
cargo nextest run --locked -E 'test(/^cli::/)'     # one integration test file: its module name is the file name
cargo nextest run --locked <name>                  # tests whose name has <name> in it
```

After a change to the CLI flags or help text, run `cargo run -q -p rvow -- docs --out docs/reference`; the `docs_reference` test fails while `docs/reference` is stale.

The runner is nextest (`cargo install cargo-nextest --locked`), set up in `.config/nextest.toml`: a test that sets an env var goes in its `serial` group, and `--profile fast` fails a test that runs past 15 s.
`cargo test --locked` still passes; there every test that sets an env var holds `crate::env_lock::lock()`.
The pi audit tests call `npm --prefix extensions run typecheck:pi` and `test:pi:*`; they are `#[ignore]`, so a plain run skips them and CI adds `--run-ignored all`. To run them, run `npm --prefix extensions ci` first and add `--run-ignored all`.

Extensions, from the repository root (run `npm --prefix extensions ci` once):

```sh
npm --prefix extensions run check        # all of the below, in this order
npm --prefix extensions run drift        # installed pi and claude versions match the pinned types
npm --prefix extensions run lint         # biome
npm --prefix extensions run typecheck:pi
npm --prefix extensions run test:pi      # node --test, needs Node 22.19.0 or newer
npm --prefix extensions run typecheck:claude
npm --prefix extensions run validate:claude
npm --prefix extensions run test:claude  # the whole suite, there is no filter

```

The claude steps skip with exit 0 when the `claude` CLI is missing, except in CI, which installs the pinned CLI and fails without it. The type check also needs the types that only a logged-in load writes, so in CI it runs only when the `ANTHROPIC_API_KEY` secret is set.

Docs site, from the repository root (run `npm --prefix site run setup` once; it fetches the theme at a pinned commit and runs `npm install`):

```sh
npm --prefix site run dev                      # sync the docs, then serve the site
npm --prefix site run build                    # sync the docs, then build site/dist
node site/scripts/check.mjs guides             # also: reference, workflow, theme
```

`.github/scripts/quickstart.sh` runs the README quickstart and `.github/scripts/install.sh` runs `rvow install`, each against the `rvow` on `PATH`, in a throwaway repository and home.
`.github/scripts/build-musl.sh <target> <out-dir>` builds the static Linux binary in docker, as CI and the release do; set `RUSTFLAGS="-D warnings"` to match CI.
A release is `cargo release <level> --execute`, run by the owner only: it dates `## [Unreleased]` in `CHANGELOG.md` and pushes the `v*` tag, and `.github/workflows/release.yml` publishes the binaries with that version block as the notes.
`prek run` runs the commit checks on staged files only; `prek run --hook-stage pre-push --all-files` runs the push checks (clippy, `cargo nextest run`, `test:pi`, `test:claude`).
CI (`.github/workflows/ci.yml`) runs the same commands; keep the two in step.

Agent evals (needs the `claude` CLI and a built binary):

```sh
evals/agent/run.sh haiku --case next-task --runs 1   # one live case, a few cents
evals/agent/run.sh                                   # the full suite, capped at $5
```

Trigger evals (needs the `claude` CLI and a built binary):

```sh
evals/triggers/run.sh haiku --case chg-next --runs 1   # one live case, a few cents
evals/triggers/run.sh                                  # the full suite, capped at $3
```

## Where things go

| Area | Home | Rule |
|------|------|------|
| Command logic | `crates/rvow-<crate>/src/<topic>.rs`, the crate the table below gives the topic | The CLI owns the truth; hooks are thin adapters. |
| Command bodies and output | `crates/rvow/src/cmd/<topic>.rs` | `crates/rvow/src/main.rs` holds only the clap types and dispatch. |
| Plan-writing guides | `guides/*.md` | Embedded by `crates/rvow-core/src/guide.rs`. |
| Agent skill | `skills/rvow/SKILL.md.tmpl` | Rendered by `crates/rvow/src/skill.rs` at install. Write each command as `{{cmd:<args>}}`; a command or flag the CLI lacks fails the render and `cargo test`. |
| Claude mod | `extensions/claude/hooks/` | Embedded by `crates/rvow-gate/src/install.rs`. |
| pi extension | `extensions/pi/rvow/*.ts` (`extensions/pi/rvow.ts` only re-exports it) | `extensions/pi/rvow/index.ts` is `PI_EXTENSION`; every other file is in `PI_MODULES` of `crates/rvow-gate/src/install.rs`. |
| Docs site | `site/` | Built from `docs/*.md`, the README and `docs/reference`; never edit the generated folders. |
| Agent evals | `evals/agent/` | `evals/agent/run.sh` installs the mod into a throwaway home; each case holds `case.yaml`, `prompt.md`, `setup.sh` and `graders/`; `lib.sh` holds the shared setup helpers. Each case keeps its eval store inside its scaffold at `.git/rvow-eval/home`, where the agent can write it. |
| Trigger evals | `evals/triggers/` | `evals/triggers/run.sh` renders the skill into a skill-only plugin in a throwaway home; each case holds `prompt.md` and `graders/fires.md`; results stay local. |

The library crates, bottom up. A crate links only to the crates above it in this table; `CRATE_ORDER` in `crates/rvow/tests/suite/audit_build.rs` holds the same order and fails on a link up or a crate it does not list.

| Crate | Holds | Links to |
|------|------|------|
| `rvow-proto` | the wire types of the engine socket: hello, hook call and reply | nothing in rvow |
| `rvow-base` | errors, ids, diagnostics, hashing, time, the glob cache, paths and every store file name (`layout`) | nothing in rvow |
| `rvow-git` | git queries, worktrees; the `testrepo` test fixtures (`TempDir`, `NamedTempDir`, `env_lock`) | base |
| `rvow-syntax` | tree-sitter units, grammars, queries, the Rust guard; the only crate with tree-sitter and ureq | base |
| `rvow-shell` | the shell parser and its rules (`Rule`, `Denial`, `Mode`) | base, proto |
| `rvow-store` | the store types, file names, lock, project index and gate record, `project_home`; no git | base |
| `rvow-repo` | how a repository finds its store (`store::find`, `init`, `rebind`; its `store` module re-exports rvow-store's) and the provenance snapshots | base, git, store |
| `rvow-plan` | specs, tasks, proposals, templates, config, lint, changes, ledger, vow, seal, fingerprint, runners | base, git, syntax, store; repo for tests only |
| `rvow-gate` | proof, gate, drift, guard, lease, amend, brief, archive, install, stats, yolo, review: the code the hooks need | all above |
| `rvow-core` | land, import, export, bundle, attest, guide, interview, notes, put, mint, show, view; re-exports the crates above so `rvow_core::<module>` paths stay | all above |
| `rvow-hook` | the hook rules for each harness event | gate and below, never core |
| `rvow-daemon` | the engine: the hook dispatch that both `rvow hook` and the daemon run, and the environment hash | all above |

- A helper that two crates need goes in the lowest crate that both reach, most often `rvow-base`; never add a link up.
- A cross-crate test fixture goes in `rvow_git::git::testrepo`, or in `testutil` of the lowest crate that has what it needs; both are always built, marked `#[doc(hidden)]`.
- Keep `rvow-hook` off `rvow-core`: code the hooks call goes in `rvow-gate` or below, so an edit in the views or the import does not rebuild the hooks.

Update this section in the change that moves an area.

## Contracts across layers

- The version lives once, in `[workspace.package]` of `Cargo.toml`. Both crates use `version.workspace = true`.
- Every file the installer writes is embedded with `include_str!`. A new module under `extensions/claude/hooks/` needs one entry in `MOD_MODULES` in `crates/rvow-gate/src/install.rs`; the install test fails when one is missing.
- Every file under `extensions/pi/rvow/` other than `extensions/pi/rvow/index.ts` needs one entry in `PI_MODULES` in `crates/rvow-gate/src/install.rs`, like `MOD_MODULES`; a file that is missing fails at pi load on an installed copy.
- The `'__RVOW_BIN__'` placeholder stays in `extensions/claude/hooks/register.tsx`, where the installer replaces it.
- The `version` in `extensions/claude/.claude-plugin/plugin.json` is overwritten at install.

## Rust rules

- No `.unwrap()` or `.expect()` outside tests. Return `rvow_core::error::Error::new(code, message, fix)`: a stable short `code`, a `message` that names the path or id, and a `fix` that is one next step.
- Errors and results print on stdout as TOON for agents (`crates/rvow/src/output.rs`). Usage errors exit 2, failures exit 1.
- A fixed set of values is an enum, not a string or an int.
- Rust source is ASCII only; the hook and CI reject any other byte.
- Add a dependency with `cargo add`.
- A hook path (pre-tool-use, post-tool-use, stop, session-start, the view) runs on every tool call: read the store, config and leases once per call and pass them down, load specs with `change::living` (no lint), borrow `&Value` and `&str`, and make no `format!`, `clone`, `collect` or `to_string` per item in a loop over specs, tasks, records, tokens or bytes. A process-wide cache has an entry limit and a key that includes what makes it stale.
- A git `Command` that sets or removes an env var copies the whole environment; remove a var only when it is set (`without_hook_env` in `crates/rvow-git/src/git.rs`).

## Claude mod rules

- The plugin engine follows `$` and `atom()` state only inside the file that declares them. Keep every function that takes `$`, and every atom, in `extensions/claude/hooks/register.tsx`; only pure code moves to other files.
- The sandbox has no `setTimeout`. Use `later($, ...)` or `$.clock`.
- `prompt.submit` from inside `command.run` is refused. Defer it with `later`.
- `extensions/claude/.claude-plugin/types` is generated and git-ignored; never edit it. `npm --prefix extensions run typecheck:claude` regenerates it. `extensions/claude/types/index.d.ts` is different: it is tracked, hand-written and holds the rvow view types.
- Strict undefined checks apply: `claude plugin test` can pass while `tsc` fails.

## Tests and proof

- A bug fix adds a test that fails without the fix.
- `crates/rvow/tests/suite/alloc.rs` holds an allocation budget per hook, counted in process on a fixed fixture store. A hook change that raises a count past its budget fails; lower a budget when a change cuts the count, and never raise one to make a test pass.
- A test that starts the rvow binary, or reads the whole repository (the `audit_*` files), goes in `crates/rvow/tests/suite/`. Any other integration test goes in `crates/<crate>/tests/suite/` of the lowest crate that has everything it uses, so `cargo nextest run --locked -p <crate>` runs it without building the crates above.
- Each crate's integration tests build as one binary, `crates/<crate>/tests/main.rs`, with one `#[path]` module per file in `crates/<crate>/tests/suite/`. A new test file needs a `mod` entry there. A test that sets an env var holds `crate::env_lock::lock()` and its module joins the `serial` group in `.config/nextest.toml`; `audit_nextest` fails when either is missing.
  A test that sets PATH or HOME builds as its own binary, as `crates/rvow/tests/alloc.rs` does, because under `cargo test` it would hide cargo and node from every test in `main.rs`.
- CLI behavior tests live in `crates/rvow/tests/suite/cli.rs`. They run in temporary git repositories, so the git hook must not leak `GIT_*` variables into them (see `.pre-commit-config.yaml`).
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
