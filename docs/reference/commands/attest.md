# rvow attest

Export the current proofs of a change as in-toto attestations

## Usage

```sh
rvow attest [OPTIONS] --out <FILE> <CHANGE>
```

## Arguments

| Argument | Description |
| --- | --- |
| `<CHANGE>` | The change id or a unique prefix of it |

## Options

| Option | Description |
| --- | --- |
| `--out` `<FILE>` | Write the JSON lines to this file |
| `--sign` | Sign the file with the configured signing key |
