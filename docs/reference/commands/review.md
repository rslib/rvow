# rvow review

Print a plan and the accept checklist; runs no verify command

## Usage

```sh
rvow review [OPTIONS] <CHANGE>
```

## Arguments

| Argument | Description |
| --- | --- |
| `<CHANGE>` | The change id or a unique prefix of it |

## Options

| Option | Description |
| --- | --- |
| `--packet` | Build fresh-context advisory evidence without running checks |
| `--record` `<RECORD>` | Record advisory JSON findings against the current packet identity |
| `--judge` | Record as a judge note with harness and model |
| `--harness` `<HARNESS>` | The harness that ran the judge |
| `--model` `<MODEL>` | The model the judge ran on |
