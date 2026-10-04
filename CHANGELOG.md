# Changelog

All notable changes to this project are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) with rules from [Common Changelog](https://common-changelog.org/), and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed

- Make the Stop hook give each reason once per session, and let subagent stops and stops during a rebase, merge or cherry-pick pass.
- Let the hook allow read-only shell commands on the enforcement files and the git hooks folder, and text that only names a protected action.
- Shorten the plan-changed message to three items and a count.

### Added

- Add a signed cache to `rvow gate --rerun` that skips a task when an earlier clean pass had the same key.
  Use `--no-cache` to run every task, and see `docs/settings.md` for `rerun_cache`, `cache_with_links` and `tools`.
- Add `max_age:` and `depends_on:` task fields for the gate cache.
- Add scoped `setup` and `checks` entries, `{ run, files }`, that run only when a task or a changed file matches.
  The rerun worktree and `setup` now wait until a task needs them.

- Add `rvow init`, `rvow new` and `rvow validate` to create the rvow tree, plan a change and check it.
- Add `rvow accept` to run the baseline and record the plan as the vow.
- Add `rvow prove` to run a task's stored verify command and record the proof, and `rvow status` and `rvow gate` to show where the branch stands.
- Add drift detection for plan edits made after accept.
  Record each edit with `rvow amend` and let the user decide with `rvow confirm`.
- Add `rvow block` to report a task that cannot pass as written, and `rvow note` to leave decisions and handoffs.
- Add `rvow check`, `rvow clarify`, `rvow answer`, `rvow review` and `rvow claim` to prepare a plan before accept.
- Add `rvow archive`, `rvow drop` and `rvow supersede` to close a change, and `rvow trace` to list the changes that touched a requirement.
- Add `rvow ls`, `rvow show`, `rvow path`, `rvow spec` and `rvow guide` to read plans, specs and the writing guides.
- Add `rvow import openspec` to convert an OpenSpec tree into specs and backlog changes.
- Add `rvow export openspec` to write the specs, open changes and done changes as an OpenSpec tree, one way and never over an existing folder.
- Add `rvow rebind` to record a repository's roots and remote in its rvow tree, and to move a home tree to the path its remote implies.
- Add `rvow install` to set up the skill, the Claude Code mod, the pi extension and the pre-push gate.
- Add a Claude Code mod with a plan pane, a needs-you band, `/rvow-status` and `/rvow-review`, and a pi extension with a widget.
- Print TOON for agents when stdout is not a terminal, with help lines and explicit empty states.
  Use `--human` or `--toon` to choose.
- Add `rvow ids` to mint real ids for the placeholders of an active change.
- Add `rvow view` to show living specs, branch tasks, waiting plans and archive history in human text or TOON.
- Add `rvow brief` to print the short generated brief for this branch.
- Add `rvow hook` to answer a harness hook with the hook JSON on stdin.
- Add `rvow gate --out` and `rvow ingest` to carry home-mode proofs through a file.
- Add `rvow bundle --out` and `rvow gate --bundle` to run accepted checks in CI without the home store.
- Record run identities for GitHub Actions, GitLab CI, CircleCI, Buildkite, Azure Pipelines, Jenkins and generic CI environments.
- Add `rvow prove --stale` to prove every stale task on the branch in one run.
- Add `rvow status --all` to show every active change and `rvow status --json` to print the view that the harness UIs draw.
- Add `rvow gate --rerun` to run every verify command again in a clean worktree of HEAD, and `rvow gate --strict` to fail on stale and flaky proofs.
  The rerun also covers the tasks of archived changes on the branch.
- Add `rvow confirm --sign` to sign a decision with your ssh key, and the `require_signature` setting to accept only signed decisions.
- Add `rvow review --packet` to build advisory evidence for a reviewer with fresh context, and `rvow review --record` to store the reviewer's findings.
- Add `rvow rebind --commit-map` to find a repository that `git filter-repo` rewrote.
- Add the `rvow_ask` tool so the agent asks the user the open questions of a plan and the answers are recorded.
- Add a bottom dock to the pi extension with tabs that stays open while you type, and a status pane with review, accept and archive.
  Both respond to the mouse, and `/rvow-dock` shows or hides the dock.
- Add a `code:` line to every error that rvow prints for an agent, so a script can match the failure.
- Add the `context` and `rules` settings to add project facts and per-artifact rules to `rvow guide`, and the `interview_cap` setting to limit the questions of a clarify round.
- Add `rvow hook <event>` as the neutral form of the harness hook.
  The old form `rvow hook claude <event>` still works.
- Add opt-in hidden checks and mutation runs to harden a task's proof, with a `mutants:` field in a task.

### Fixed

- Fix `rvow install` leaving the rvow plugin disabled when it was installed but turned off.
- Fix `rvow check` to report a plan with no tasks as not ready, and the brief to ask for `rvow check` instead of accept.
- Fix the planning brief to print only the counters that are not zero.
