# rvow hook

Answer a harness hook: reads the hook JSON on stdin, prints the response

## Usage

```sh
rvow hook [OPTIONS] <TARGET> [EVENT]
```

## Arguments

| Argument | Description |
| --- | --- |
| `<TARGET>` | The event, or the harness (claude, codex or json) followed by its event One of `claude`, `codex`, `json`, `pre-tool-use`, `post-tool-use`, `session-start`, `stop`, `status`, `view`, `lease-bind`, `lease-end`. |
| `<EVENT>` | One of `pre-tool-use`, `post-tool-use`, `session-start`, `stop`, `status`, `view`, `lease-bind`, `lease-end`. |
