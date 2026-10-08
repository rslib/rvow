# rvow settings

rvow reads settings from a global file and a project file.
The global file lives inside the rvow home (the directory named in `RVOW_HOME`).
The project file is `config.toml` in the rvow tree root of the project.
That root is the project folder of the home store in home mode, and the `rvow/` folder of the repository in repository mode.
The project value wins when both set a key, and a project list replaces the global list instead of extending it.

In repository mode the shared settings come from the project file only, so one contributor's settings cannot change a check that the project owns.
The global-only keys `machine`, `signing_key` and `agent` still come from the global file in both modes, and the project file refuses them.

Most keys are part of the vow: changing them after the plan is accepted is
drift until the user confirms it. `machine`, `signing_key` and `agent` are
exceptions. They belong in the global file only, and their values never enter
the proof key.

## An example

The full example below is a project file in home mode. It parses as valid rvow
settings: three interview rounds, unsigned confirmations by default, and a ten
minute test timeout.

```toml
base = "main"
timeout = "10m"
interview_cap = 3
checks = ["cargo test --locked"]
```

| Key | Meaning | Global only |
|---|---|---|
| `base` | The branch that becomes the `branch:` of a new plan | no |
| `timeout` | The default time one verify command may run | no |
| `inputs` | Extra globs whose content is part of a proof's input identity | no |
| `test_globs` | Globs that mark a file as a test, so editing it counts as weakening a proof | no |
| `protect_infra` | Globs of test infrastructure that the guard protects, such as snapshots, runner configs and `package.json` | no |
| `checks` | Commands the gate runs after task verification; an entry is a string or `{ run, files }` | no |
| `setup` | Commands the gate rerun runs before verification; an entry is a string or `{ run, files }` | no |
| `rerun_env` | Environment pairs applied when the gate reruns a task | no |
| `rerun_links` | Git-ignored directories of the repository, by repo-relative name, that the rerun worktree links in (default none) | no |
| `rerun_skip` | Archived tasks, by id or a prefix of it, that a push does not rerun on this machine; the gate prints a warning that names each one (default none) | no |
| `tools` | Executables whose content is part of the gate cache key, by name on `PATH` or by path | no |
| `rerun_cache` | How long a cached rerun pass stays valid, such as `24h`, or `off`; default `168h` | no |
| `cache_with_links` | Keep the rerun cache on when `rerun_links` is set; default false | no |
| `env` / `env_vars` | Extra commands and variables captured in the proof environment | no |
| `interview_cap` | The most questions one clarify round asks, 1 to 3, default 3 | no |
| `require_signature` | When true, a confirm only decides when its ssh signature is valid | no |
| `allowed_signers` | The allowed-signers file for signed confirmations | no |
| `context` | Project context text the `rvow guide` command appends | no |
| `rules` | Per-artifact authoring rules loaded by `rvow guide` | no |
| `machine` | A name for this computer | yes |
| `signing_key` | The ssh key for `rvow confirm --sign` | yes |
| `agent` | A name for the agent that runs proofs | yes |

The project `context` and each artifact `rules.<artifact>` value are capped at
2000 UTF-8 bytes. Values above the cap are refused at load.

## Linking build output into the rerun

`rvow gate --rerun` runs every verify command in a clean worktree of HEAD, so a verify command that needs generated output such as `node_modules` fails there.
List such a directory in `rerun_links` to link it from the real repository into the worktree.
The default is an empty list, which keeps the worktree clean.
A name must be a relative path inside the repository that git does not track.
A tracked name, an absolute name or a name with `..` is refused, and the error names the key.
A listed directory that does not exist is skipped.
A verify command can write through a link into the real directory, so list only directories that a verify command may change.
`rerun_links` is not part of the vow, so changing it is not drift.
The same holds for `rerun_skip`. Use it for an archived task that reruns the whole test suite, since pre-commit and CI already run the suite, and it would otherwise run again on every push that touches code.

## Scoping setup and checks to files

A `setup` or `checks` entry can be a table with `run` and `files`, such as `{ run = "make -C build", files = ["src/**"] }`.
A table entry runs only when a task that runs shares a file with its globs, or when a file that the branch changed matches one of them.
A plain string entry always runs.
With a C++ entry scoped to `src/**` and a Python entry scoped to `python/**`, a branch that changes only `python/a.py` runs the Python entry and skips the C++ one.
When the base branch is unknown, a table entry runs, so nothing is skipped by mistake.

The gate makes its rerun worktree and runs `setup` only when something needs them.
When the cache answers every task and no archived task reruns, it makes no worktree and runs no `setup`.
The `checks` that remain then run in the repository itself.

## How long a cached pass lasts

An entry in the gate cache stops answering for a task after `max_age`, which a task sets with a `max_age:` line such as `max_age: 24h`.
A task without one uses `rerun_cache`, and a project without that uses 168 hours.
Set `rerun_cache = "off"` to run every task each time.
`max_age`, `rerun_cache` and `cache_with_links` tune the cache, so changing them is not drift.

## Linked build output and the cache

A task that runs against linked build output, listed in `rerun_links`, can pass or fail on files that git does not track.
For that reason the gate turns the cache off whenever `rerun_links` is set, and prints a warning that says so.
Set `cache_with_links = true` when you know the linked output does not change a result.

## Tools in the cache key

The gate cache skips a task whose inputs did not change.
A compiler or container runtime that changes under the same project files can change a result, so list it in `tools`.
Each entry is a name on `PATH` or a path; the cache key holds the sha256 of the file after links are resolved.
A tool that is missing makes the task uncacheable, so the gate runs it.

A clean checkout never contains the user's `machine`, `signing_key` or `agent`.
Send those through the environment or a separate global file instead.