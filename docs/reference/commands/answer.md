# rvow answer

Record the user's answer to an open question and remove its marker

## Usage

```sh
rvow answer [OPTIONS] <REFERENCE> <CHOICE>...
```

## Arguments

| Argument | Description |
| --- | --- |
| `<REFERENCE>` | The question ref that rvow clarify printed, as CHG-xxxxxx:proposal.md:12 |
| `<CHOICE>` | The user's choice, as given |

## Options

| Option | Description |
| --- | --- |
| `--via` `<VIA>` | How the user answered: in a dialog, or in chat relayed by the agent One of `dialog`, `chat`. |
