# rvow

Hold a coding agent to an accepted plan.

Status: early development, with no release yet.

You and the agent agree on a plan, and `rvow accept` records it as the vow.
The vow is the accepted plan with its hashes.
The plan is the files; the change is the folder that holds them.
Checks that run outside the model then hold the agent to it.
Each task stores its own verify command, and `rvow prove` runs exactly that command.
Edits to the plan after accept show up as drift that the gate refuses until you confirm them.

## Install

Install the binary from a clone of this repository.
It needs Rust 1.98 or later and builds for macOS and Linux.

```sh
cargo install --path crates/rvow --locked
```

## Quickstart

In a git repository that has at least one commit, create the rvow tree and a first change.

```sh
rvow init
rvow new "Print a greeting"
rvow ls
```

`rvow ls` prints one line per change with its id, stage, branch and title.
The id differs on every run, and the branch is the name of your current branch.

```
CHG-mssayd backlog    main Print a greeting
```

By default the tree lives in your home store, not in the repository.
Run `rvow init --in-repo` to keep it in `rvow/` inside the repository.

## Config

Settings live in `config.toml`: in the project's `rvow/` folder for project settings and in the rvow home for your own.
A key in the project config wins over the same key in the home config.
The table names every key.
[docs/settings.md](docs/settings.md) has the detail and the global-only keys.

| Key | Meaning |
|-----|---------|
| `base` | The branch a change is measured against (default `main`). |
| `timeout` | How long a verify or check command may run (default `10m`). |
| `inputs` | Extra globs whose content is part of a proof's input identity. |
| `test_globs` | Globs that mark a file as a test the vow protects. |
| `protect_infra` | Globs of test setup files the vow protects. |
| `checks` | Commands the gate runs after task verification, each a string or `{ run, files }`. |
| `setup` | Commands the gate rerun runs before verification, each a string or `{ run, files }`. |
| `rerun_env` | Environment pairs applied when the gate reruns a task. |
| `rerun_links` | Ignored folders that the rerun worktree links in. |
| `rerun_cache`, `cache_with_links`, `tools` | How long a cached rerun pass lasts, whether links keep the cache on, and the binaries in the cache key. |
| `env`, `env_vars` | Commands and variables captured in the proof environment. |
| `interview_cap` | The most questions a clarify round asks (default 3). |
| `require_signature` | When true, only signed confirmations pass the gate. |
| `allowed_signers` | The allowed-signers file for signed confirmations. |
| `context`, `rules` | Project context and per-artifact rules that `rvow guide` appends. |
| `machine`, `signing_key`, `agent` | Global only: your computer, signing key and agent names. |

## Upgrading

After you upgrade rvow, run `rvow install` again.
It refreshes the skill, the Claude Code mod and the pi extension.
It keeps a file that you edited and says so.

## Next steps

- `rvow install` adds the skill, the Claude Code mod, the pi extension and the pre-push gate.
- `rvow guide proposal` prints the short guide for writing a plan artifact. The other artifacts are `spec`, `tasks` and `design`.
- `rvow --help` and `rvow <command> --help` are the command reference.
- [docs/settings.md](docs/settings.md) lists every setting and the global-only keys.
- [docs/authoring-context.md](docs/authoring-context.md) explains the project context and per-artifact rules that `rvow guide` appends.
- [docs/how-it-works.md](docs/how-it-works.md) explains the vow, drift, the proof and the gate.
- [docs/ci.md](docs/ci.md) covers portable verification in GitHub Actions and other CI with `rvow bundle --out`, `rvow gate --bundle` and `rvow ingest`.
- [AGENTS.md](AGENTS.md) has the build and test commands for contributors.

## License

MIT, see [LICENSE](LICENSE).
