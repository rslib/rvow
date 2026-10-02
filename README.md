# rvow

Hold a coding agent to an accepted plan.

Status: early development, with no release yet.

You and the agent agree on a plan, and `rvow accept` records it as the vow.
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

## Next steps

- `rvow install` adds the skill, the Claude Code mod, the pi extension and the pre-push gate.
- `rvow guide proposal` prints the short guide for writing a plan artifact. The other artifacts are `spec`, `tasks` and `design`.
- `rvow --help` and `rvow <command> --help` are the command reference.
- `AGENTS.md` has the build and test commands for contributors.

## License

MIT, see `LICENSE`.
