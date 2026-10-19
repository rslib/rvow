# rvow gate

Check the active changes of this branch

## Usage

```sh
rvow gate [OPTIONS]
```

## Options

| Option | Description |
| --- | --- |
| `--strict` | Fail on stale and flaky proofs too |
| `--rerun` | Run every verify command again in a clean worktree of HEAD |
| `--no-cache` | With --rerun, run every task even when an earlier clean pass has the same key |
| `--out` `<FILE>` | Write each rerun result to this file, for `rvow ingest` to read |
| `--bundle` `<FILE>` | Execute an accepted verification bundle without accessing the home store |
