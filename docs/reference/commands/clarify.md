# rvow clarify

List the open questions of a plan, each with its recommended answer

## Usage

```sh
rvow clarify [OPTIONS] [CHANGE]
```

## Arguments

| Argument | Description |
| --- | --- |
| `<CHANGE>` | The change id; default: every open change on this branch |

## Options

| Option | Description |
| --- | --- |
| `--json` | Print the next round (at most interview_cap questions, default 3) as JSON: question, options, recommended first |
