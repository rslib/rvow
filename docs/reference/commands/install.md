# rvow install

Install the skill, the Claude Code hooks, the pi extension and the pre-push gate

## Usage

```sh
rvow install [OPTIONS]
```

## Options

| Option | Description |
| --- | --- |
| `--claude` `<CLAUDE>` | How rvow joins Claude Code: a mod (plugin), or hooks in a settings file One of `mod`, `settings`. |
| `--scope` `<SCOPE>` | For --claude settings: your user settings, or this repository's .claude/settings.json One of `user`, `project`. |
| `--all` | Also refresh the pre-push gate of every repository where rvow install wrote one |
