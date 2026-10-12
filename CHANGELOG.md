# Changelog

All notable changes to this project are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) with rules from [Common Changelog](https://common-changelog.org/), and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed

- Measure the vow and the gate from the newer merge base of the local and the remote base branch, so unpushed commits on the base branch never count as outside every task.
  When the two merge bases diverge, the remote one wins.
- Fail `rvow gate` on code that differs from the base while a backlog plan names the branch and no change is active on it, naming the waiting plan.
  Files under the in-repo rvow tree and files that a done change of the branch covers still pass.
- Trust a proof only when its seal checks and its verify programs match, and rerun a task whose scope holds an ignored file.
  A proof from before seals existed reruns once.
- Mark a task stale with `dependency-changed` when a file in a `depends_on:` task's scope changed since the dependent task's proof.
- Seal the yolo switch.
  A yolo file with no valid seal turns every tier off.
- Stop rerunning a task on a push when its proof is current and its files are committed, and never rerun the tasks of an archived change. Set `rerun_proven = true` to rerun proven tasks.
- Allow commands that only read the ledger, the ids and the stats, lease, yolo and cache files, and text that only names the hidden checks, such as an `echo` or a commit message.
- Audit an edit outside every task scope and an edit of a protected test instead of denying it. The hook counts it, and the gate still blocks the branch.
- Ask the user after 3 denials in a row in one session, instead of denying a fourth time.
- Tell the agent when an accept, a confirm or an archive that the user ran fails, with the command and the reason.
- Run the baseline of an added or changed check at amend time: `rvow amend` runs each added task's verify command and each changed verify command on the vow's base commit, in a clean worktree set up like the gate rerun, and refuses with the code `baseline` a check that passes there without `baseline: pass`, or a `baseline: pass` check that fails there.
  The amendment records each run's task id, exit code and test count, and `rvow confirm` lists weaken items first with each baseline result.
- Read the base commit from the vow, also after a confirmed amendment, so `rvow amend` and the mutants diff keep working.
  When the vow has no base commit, the amend error names `rvow block` instead of accept.
- Mint placeholder ids before the amend snapshot, so a new task keeps its real id after confirm and `rvow ids` mints nothing afterwards.
- Call a task flaky only on the same worktree: each prove event records a worktree fingerprint over tracked and untracked files that are not ignored, and a pass and a failure count as flaky only with the same run key and fingerprint.
  An old failure with no fingerprint does not count.
- Count a removed task and a task that gains `baseline: pass` as weaken drift.
  Dropping the flag or changing a timeout stays free.
- Treat a change of a task title as free drift.
- Stop showing `config changed` drift for a new default value of a newer rvow.
  A value that your config files set, now or at the baseline, still counts.
- Stop showing a placeholder that `rvow ids` minted as a removed and an added item, and run no amend baseline for it.
- Start the pi judge session with `--no-skills` and `--no-context-files`.
- Let a subagent that holds a lease prove only its own task.
- Make the brief name missing or failed work of the newest change before a stale task of an older change.
- Make the Stop hook give each reason once per session, and let subagent stops and stops during a rebase, merge or cherry-pick pass.
- Let the hook allow read-only shell commands on the enforcement files and the git hooks folder, and text that only names a protected action.
- Shorten the plan-changed message to three items and a count.
- Name the amendment and its decision when a confirm names one that already has a decision, such as `AMD-x is already accepted`, with the code `decided`.
  An unknown id keeps the code `amendment`.
- Allow a shell command that only reads through a glob that expands to the ledger, the ids and the stats, lease, yolo and cache files.
  A write through such a glob stays denied, and a glob that reaches the hidden checks stays denied, also for a read.
- Name the slash command the user can run when they decline the accept, archive or confirm dialog, so the agent says what to run next.

### Added

- Protect nested test folders by default: `tests/`, `__tests__/`, `spec/` and `src/test/` at any depth, with `*_spec.rb` and `*Test.java` names, and the `tox.ini` setup file.
  Setting `test_globs` or `protect_infra` still replaces the defaults.
- Add an optional spec-conformance judge: `rvow guide judge` prints the shared judge prompt, and `rvow review --record <file> --judge --harness <harness> --model <model>` records its findings against the current packet (`--record -` reads stdin).
  The gate shows each finding as a warning without failing, and `rvow archive` names `/rvow-judge` when the current packet has no judge note.
  The hook denies `rvow review --judge` for agents, and `/rvow-judge` with the `rvow_judge` tool in the Claude mod and pi runs the judge in a clean read-only session with no rvow extension, recording the model it ran on.
- Add a seal to each proof so a hand-written proof no longer counts.
  The seal is an HMAC under the rerun cache key over the event kind, time, parents and data.
- Print each task's `verify:`, `files:`, `baseline:` and `mutants:` in `rvow review`, and list a weak-check warning above the checklist.
  `rvow check` warns with the code `weak-verify` about a stub command and about `baseline: pass` on a command with no counted test runner.
- Add a warning from `rvow check` for a task whose `files:` cover more than 40 tracked files.
- Add the tools `rvow_accept`, `rvow_archive`, `rvow_confirm` and `rvow_yolo` to the Claude mod and the pi extension.
  The user answers a dialog before anything is accepted, confirmed or switched on, and a failure comes back to the agent with its reason.
- Add `rvow yolo`, and `/rvow-yolo` in the Claude mod and pi, for the user to relax the hooks for a while, in three tiers: `audit`, `auto` and `bypass`.
  It ends by itself after 2 hours by default and at most 24, the status line and brief show it, and the gate still checks at push.
- Add `rvow stats` to show how often each hook denied a tool call or blocked a stop, by rule, with no command text or path stored.
- Add `rvow delegate` to lease one task to a subagent.
  The Claude mod binds the subagent to the lease, and the hook then denies its edits outside the files of that task.
- Add a signed cache to `rvow gate --rerun` that skips a task when an earlier clean pass had the same key.
  Use `--no-cache` to run every task, and see `docs/settings.md` for `rerun_cache`, `cache_with_links` and `tools`.
- Add `max_age:` and `depends_on:` task fields for the gate cache.
- Add scoped `setup` and `checks` entries, `{ run, files }`, that run only when a task or a changed file matches.
- Add `RVOW_GIT` to run a chosen git binary for every git call.
  Set it where the git on `PATH` cannot run, such as a sandbox that stops the system git shim.
- Add `rvow attest <change> --out <file>` to export the current proofs as in-toto statements, one Test Result line per proved or failed task and one summary line for the change.
  Pass `--sign` to write an ssh signature of the file with the `signing_key` of the global config.
  The rerun worktree and `setup` now wait until a task needs them.

- Write the rendered skill to `skills/rvow/SKILL.md` in the pi agent folder on `rvow install`, keeping a user edit and reporting a stale copy in the install check.
- Add one `loop:` line to the brief with the next task, naming `rvow prove`, `rvow amend` with the `rvow_confirm` tool, and `rvow show`.
  A branch with no active change and no backlog plan gets one line naming `rvow new` and `rvow guide proposal`.
- Add `rvow next` to print the next task of the branch with its id, title, state, verify command and files, or to say that no task is next.
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
- Add `rvow confirm --sign` to sign a decision with your ssh key, and the `require_signature` setting to accept only signed decisions.
- Add `rvow review --packet` to build advisory evidence for a reviewer with fresh context, and `rvow review --record` to store the reviewer's findings.
- Add `rvow rebind --commit-map` to find a repository that `git filter-repo` rewrote.
- Add the `rvow_ask` tool so the agent asks the user the open questions of a plan and the answers are recorded.
- Add a bottom dock to the pi extension with tabs that stays open while you type, and a status pane with review, accept and archive.
  Both respond to the mouse, and `/rvow-dock` shows or hides the dock.
- Add a `code:` line to every error that rvow prints for an agent, so a script can match the failure.
- Add the `context` and `rules` settings to add project facts and per-artifact rules to `rvow guide`, and the `interview_cap` setting to limit the questions of a clarify round.
- Open one dialog for `/rvow-confirm` with no argument or `/rvow-confirm all` in the Claude mod to accept all pending plan edits, reject them all or decide later.
- Add `rvow hook <event>` as the neutral form of the harness hook.
  The old form `rvow hook claude <event>` still works.
- Add opt-in hidden checks and mutation runs to harden a task's proof, with a `mutants:` field in a task.
- Run a task's `mutants:` command with `RVOW_DIFF` set to a file holding the diff of the task's files against the base commit, and `RVOW_CHANGED_FILES` listing the changed paths one per line, so `cargo mutants --in-diff "$RVOW_DIFF"` tests only the changed lines.
  Record the missed count as `survived` inside the `mutants` object of the proof event when the output holds a cargo-mutants summary, and print `mutants: N survived` in `rvow show`.

### Fixed

- Fix the hook reading owner command text as data from a committed test file.
  A committed test file with no change against HEAD runs, and every changed, untracked or non-test script still gets the check.
- Fix the gate counting the files of every done change of the branch, so a done change whose work is already in the base no longer widens the scope.
  A done change counts only while its archive HEAD is not a strict ancestor of the merge base.
- Fix `rvow archive` failing when the judge note lookup fails.
  The archive now goes on and names `/rvow-judge`.
- Fix `rvow review --judge` reporting a bad record for a record file that is missing.
  The error now names the file.
- Fix the Claude mod keeping every subagent answer for the whole session.
  It keeps a judge answer only while a judge run waits, and never hands the main agent's answer to a judge run.
- Keep a tampering denial a denial after 3 denials in a row, deny a shell glob that expands to a protected file, and hold the stop for each unproved task that no pending amendment names.
- Fix `rvow install` leaving the rvow plugin disabled when it was installed but turned off.
- Fix `rvow check` to report a plan with no tasks as not ready, and the brief to ask for `rvow check` instead of accept.
- Fix the planning brief to print only the counters that are not zero.
- Fix the review packet and the task diff failing for a task file whose name git reads as pathspec magic, such as `:(bad`.
  Task file names now reach git as literal paths.
- Fix the hook reading a data file as a script when an interpreter takes the script from stdin or runs inline code.
- Fix drift flagging a protected test file that only grew.
  Added tests and standalone added checks stay free, and a lost or changed line still counts.
