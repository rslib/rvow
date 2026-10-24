# rvow decide

Record the user's decision about a stale relaxed task (user only)

## Usage

```sh
rvow decide [OPTIONS] [TASK] [PICK]
```

## Arguments

| Argument | Description |
| --- | --- |
| `<TASK>` | The task id or a unique prefix of it |
| `<PICK>` | Prove it again now, prove it again at archive, or keep the proof that stands One of `now`, `archive`, `keep`. |

## Options

| Option | Description |
| --- | --- |
| `--list` | List the stale relaxed tasks that wait for the user's decision |
