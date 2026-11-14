# rvow uninstall

Remove what rvow install wrote: the skill, the Claude Code hooks, the pi extension and the pre-push gate

## Usage

```sh
rvow uninstall [OPTIONS]
```

## Options

| Option | Description |
| --- | --- |
| `--all` | Also remove the pre-push gate of every repository where rvow install wrote one |
| `--purge` | Also delete the rvow home with every plan and proof in it. This cannot be undone |
