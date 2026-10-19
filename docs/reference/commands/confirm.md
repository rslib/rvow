# rvow confirm

List pending amendments, or accept or reject them (user only)

## Usage

```sh
rvow confirm [OPTIONS] [IDS]...
```

## Arguments

| Argument | Description |
| --- | --- |
| `<IDS>` | Amendment ids or unique prefixes |

## Options

| Option | Description |
| --- | --- |
| `--accept` | Accept the named amendments |
| `--reject` | Reject the named amendments |
| `--all` `<DIGEST>` | Decide every pending amendment; pass the digest that the list printed |
| `--sign` | Sign the decision with the global signing_key |
| `--signer` `<IDENTITY>` | Explicit principal in the allowed-signers file |
| `--full` | List every changed line whole. A plain list shows only the item names |
