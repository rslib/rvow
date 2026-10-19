# rvow amend

Record the current plan edits as an amendment for the user to confirm

## Usage

```sh
rvow amend [OPTIONS] --reason <REASON>
```

## Options

| Option | Description |
| --- | --- |
| `--reason` `<REASON>` | Why the plan changed |
| `--change` `<CHANGE>` | The change id; default: every active change on this branch |
| `--unlock` `<PATH>` | Ask to edit a protected test or setup file instead of recording drift |
