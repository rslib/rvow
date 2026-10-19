# Add project context to a guide

Put shared project facts in `context` and artifact-specific instructions in `rules` in the project's `config.toml`.

```toml
context = "The library owns validation. The CLI only formats results."

[rules]
proposal = "Name the user-visible change."
spec = "Describe behavior without naming private helpers."
tasks = "Use a focused behavior test in each verify command."
design = "Record only decisions that the code cannot explain."
```

Run `rvow guide proposal` to read the built-in guide followed by project context and proposal rules.
The other guide commands use their own rules.
The context and each artifact's rules are limited to 2000 UTF-8 bytes.

> [!NOTE]
> An oversized value is an error, not a silent truncation.

You can set the same keys in the global config.
These rules decide which value wins:

- The project context replaces the global context.
- A project's rule overrides the global rule for that artifact only.
- An empty string clears an inherited value.
- Repo-mode projects ignore global context and rules so personal settings cannot change shared instructions.

Without a project, the command prints the built-in guide.
