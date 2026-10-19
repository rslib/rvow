# rvow put

Replace one part of a change, or one task or requirement block, with the text on stdin

## Usage

```sh
rvow put [OPTIONS] <TARGET> [PART]
```

## Arguments

| Argument | Description |
| --- | --- |
| `<TARGET>` | A CHG id with `part`, or a TSK or REQ id whose block the stdin replaces |
| `<PART>` | One part of a change: `proposal`, `tasks`, `design` or `spec:<name>` |
