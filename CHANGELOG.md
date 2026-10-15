# Changelog

All notable changes to this project are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) with rules from [Common Changelog](https://common-changelog.org/), and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- A task verify can name a project runner with `@<runner> key=value`: rvow fills the runner's `run` command from the `[runners]` table of the project config with the task's keys and the runner's defaults, applies its `env` and `timeout`, and `rvow review` shows the filled command.
  A changed runner is weaken drift for each task that uses it, and a runner no task uses changes with no drift.
- Compare a protected test file by syntax unit when its grammar is in the cache: a unit is an item with the attributes, decorators and comments right before it, and a module, class, namespace or `@group` call splits into a header unit plus one unit per body item.
  Each old unit must keep its canonical form as many times as before, and a unit keeps its container.
  A new unit is free only when a shipped query marks it a test with a name the old text does not use, and its attributes are test attributes: a moved test and a new test are not drift, while a new helper, hook, fixture, import or shadowing macro, a removed `#[test]`, a changed assert and a deleted duplicate are. Record such drift with `rvow amend`.
  A file whose grammar does not load or whose text does not parse cleanly keeps the raw rules.
- Query files name the changed test: shipped queries mark the test units of JavaScript and TypeScript, Python, Rust, C++ and Go, and `test_queries = { <lang> = "<path>" }` in the project config adds a query file per language.
  A gtest `TEST(Suite, Name)` is named `Suite.Name`, and a `@group` capture marks a call as a container that splits into a header plus its body items.
  The drift item prints the test's title or function name, a unit no query marks keeps today's item text, and `rvow check` warns with `test-query` about a query file that is missing or does not compile.
- A runner that fills `{junit}` writes a JUnit report that accept and prove read: accept records the failed test names and refuses when the report names none, and a proof passes only when each recorded name passes and no test case failed.
  A missing `needs` path refuses the run and names the path, `rvow check` warns `configure-in-verify` about a CMake configure step in a verify, and a `Re-running CMake` output adds a note to the prove result.

- `rvow show <change>` prints an outline of the change (each proposal section with its word count, each requirement with its title, each task with its state, and the parts), and `rvow show <change> <part>` prints the full text of one part: `proposal`, `tasks` or `spec:<name>`.
- `rvow put` replaces one part of an open change, or one task or requirement block, with the text on stdin, writes it atomically and prints `ok` or the check findings of the change.
- The hooks deny a direct read, write or search of the plan files in the rvow store of a home-mode project, and the reason names the `rvow show` or `rvow put` command for the need; a search from the root names `rvow ls` and `rvow show`.
- `rvow check` warns with `full-suite` about a task whose verify runs the whole suite instead of one target, the mods show how many baseline checks accept will run and size the accept timeout to the plan's task timeouts, and `rvow archive` prints each gate check on stderr while it runs.
- Compare a protected file by its syntax tree when both texts parse with the grammar of its language, so a file that only lost a blank line or a comment is not drift.
  `rvow grammar fetch [<lang>...]` fills the grammar cache with pinned checksummed `.wasm` grammars and `rvow grammar list` shows each pinned language and whether the cache holds it.
- Count comment text in the syntax compare, fetch missing grammars in prove and gate with a short timeout, and save each compare result under `grammars/` for reuse.
- Name tests in Java, Kotlin, C#, Ruby and PHP: shipped queries mark JUnit, xUnit/NUnit/MSTest, RSpec/Minitest and PHPUnit tests, so a new test is free and a changed test is named in the drift item, while setup and teardown hooks and a test with an extra annotation such as `@Disabled` stay drift.
- Name doctest, Catch2 and Unity tests: the shipped C++ query marks `TEST_CASE`, `TEST_CASE_FIXTURE`, `TEST_CASE_METHOD`, `TEST_CASE_TEMPLATE`, `TEMPLATE_TEST_CASE` and `SCENARIO` blocks by their string and `TEST_SUITE` as a container, and a new C query marks the `test_` functions and `RUN_TEST` lines of a Unity file.
  A new case or `RUN_TEST` line is free and a changed case is named in the drift item, while a `SUBCASE` or `SECTION` change names its case, and a new case that exits, skips or hides itself with `doctest::skip()` or a Catch2 `[.]` tag stays drift.
  A doctest macro parses with a missing `;`, and rvow accepts that one error in a C++ file instead of falling back to the raw rules.
- Count the tests of `ctest`, doctest, GoogleTest and Unity runs from their summary lines, so a `ctest` run that finds no tests (`No tests were found!!!`) is 0 tests and no proof, and `ctest` is a counted runner for `baseline: pass`.

### Changed

- Run `rvow accept`, `rvow amend`, `rvow archive` and `rvow check` in the worktree where the branch of the change is checked out, so a plan made in a second worktree works from the main checkout.
  When no worktree has the branch, accept names `git checkout <branch>` and `git worktree add <path> <branch>`.
- Print one hint when an accept baseline run takes more than 60 seconds and the project sets neither `rerun_env` nor `rerun_links`.
  The hint names `rerun_env = { CARGO_TARGET_DIR = "<shared dir>" }` and `rerun_links = ["target"]`.

- Let a project query free a test: a query file named in `test_queries` is protected at accept, a change to it is drift, and a unit that a trusted query marks `@test` with a fresh name is free under the same rules as a shipped mark.
  A query file that was not protected at accept, such as one named after accept, still only names tests.
- Free a new `mod NAME;` line in a protected Rust test file when its module file is new since accept, its only attributes are `#[path = "..."]` and `#[cfg(test)]`, and the module file calls no process exit and focuses no run.
- Hold a stop only for the work of the session: a session is held only for a task or drift item whose file it changed, a pending amendment of a change releases its stop, and stale proofs no longer hold a stop and are left to the push gate; `rvow stats` lists the stop blocks by reason, `drift`, `no-proof` or `unknown`.
- Count only the segments that are not a file check (`grep`, `rg`, `test`, `[`, `diff`, `cmp`) toward the `full-suite` chain warning, so a verify of file checks before one test no longer warns.
- Refuse a blank required proposal section: `rvow check` reports a structure error that names the section and its line, and one word is enough.
- Count the issues of a backlog plan in the dashboard with the same function as `rvow check`, so a task whose runner fails to fill counts once and the plan is not ready.
- Build the release binary with symbols stripped, thin LTO and one codegen unit, which cuts it from 20.7 MB to 13.7 MB.
- Skip a configured `checks` command that already passed on the same commit, keyed by the command text, the HEAD tree id and the tool fingerprint, and print `cached check: <cmd>` on the check stream.
  An uncommitted change in the check's scope runs the check again and records nothing, and `rerun_cache = "off"` turns the check cache off.
- Do not try a grammar fetch again for one hour after it fails in prove or gate, treating the language as one with no grammar; `rvow grammar fetch` always tries and a success clears the record.
- In pi, wait for the gate checks on archive and for the plan's whole baseline on accept instead of a fixed 60 s or 10 min, and say when a run timed out instead of returning no output.
- Keep the accept dialog short: it shows at most 10 checklist lines and names `rvow review <id>` for the rest, and the accept tool asks nothing again while the plan is unchanged, returning the earlier reason with the `/rvow-accept <id>` command the user can run.
- Measure drift from the snapshot of the accepted amendment recorded last, so a confirm of several amendments at once no longer leaves drift that the user just confirmed.
- Make `cargo test --locked` fast: one integration test binary, optimized wasmtime and cranelift in the dev profile, no nested `cargo build` or npm audit in a plain run (CI passes `--include-ignored`), and `RVOW_GRAMMAR_OFFLINE` stops tests from downloading grammars.
- Read only the added lines of a changed test file, not the whole file, so committed owner command text no longer denies a test run.
- Ignore slash command names such as `/rvow-accept` in script text, so a name that cannot run from a script is not treated as an owner command.

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

- Fix `rvow install` and the gate disagreeing about a pre-push hook that another tool wrote: install now moves that hook to `pre-push.rvow-prev` and writes its own hook, which runs the old hook first with the same arguments and pushed refs and stops the push when it fails.
  When a tool rewrites the hook later, the gate says to run `rvow install` again.
  Under `core.hooksPath` install prints `'<rvow>' gate --strict --rerun || exit 1` to add as the first command of the hook, and the gate accepts that hook only when it runs as a shell script.
- Hold no session for the files it only named in plan or note text: heredoc and quoted text count for changed files only when an interpreter or a shell runs it, and an `rvow` command records no file.
- Fix the session record counting formatter runs, folders and code words as changes: a formatter run records nothing beyond the files it names, folders are never recorded, and script text counts only for an existing file or a path with a `/` in an existing folder.

- Fix `rvow check` and `rvow clarify` reading the worktree of the current checkout for a change named by id: they now read the worktree of that change's branch, so a check from the main checkout reports the same as a check from the worktree of the branch.
- Fix a protected C or C++ test file falling back to the raw line rules when its text holds a parse error such as a default argument of `= {}`: it is now compared by syntax unit, with each error region part of the unit around it, so a change inside one is drift.
- Fix a new test reaching a process exit through a string or a symbol going unnoticed: `getattr(sys, "exit")(0)`, `process["exit"](0)`, `Reflect.get(process, "exit")(0)` and `Kernel.send(:exit)` are drift, while a string that only names exit stays free.
- Fix a runner that prints its summary in color hiding its test count: the color codes are removed before the count is read, so colored vitest output counts like plain output.
- Fix a verify that chains several runners hiding one that ran no tests: the counts of the runners are added, and a run in which any of them ran 0 tests records 0 and is no proof.
- Fix `rvow show AMD-x` saying no id matches: it now prints the amendment of an active or archived change with its reason, its items and whether it is pending, accepted or rejected.
- Fix the confirm dialog of the mods deciding every pending amendment at once: with more than one pending it asks about each amendment in turn, the `rvow_confirm` tool takes an optional `id` for one amendment, and the `rvow_ask` tool takes an optional `change` to ask the open questions of that change.
- Fix the drift item of an inline Rust test module that only gained items saying lines were lost: it now names the added helpers, such as `adds helpers row_text, row_ids`.
- Fix a search denial that took the `/` of `tmux send-keys /` for a search from the root: a search program is denied only for a path among its own arguments, or after a `cd` into a covering folder.
- Fix the state of a task after two overlapping runs: it now comes from the run that started last, and a failed run whose inputs changed since is stale, so `rvow prove --stale` runs it again.
- Fix `rvow put` writing the first plan when an id sits in two open plans.
  It now refuses and names both changes.
  Put the whole part instead.
- Fix a subagent bind being refused because of an old lease whose task is already proved or blocked.
  A bind is now refused only while the task of the old lease is still open.
- Remove old session records under `hook-state` after 7 days.
- Report a new protected infra file as drift: a `conftest.py` or `.cargo/config.toml` added after accept is `protected file <path> added`, while a new test file stays free.
- A new test that calls a process exit or abort, such as `std::process::exit`, `sys.exit`, `process.exit`, `os.Exit` or `exit()`, or that focuses the run with `focus: true` or `{ only: true }`, is drift in a protected test file, and rvow removes saved syntax compare results older than 30 days.
- A `node --test`, vitest or mocha verify that ran 0 tests no longer proves a task.
- End every lease of a task when the task is proved or blocked, bound or not, so a lease no longer comes back when the task goes stale.
- Let a pi session take the next lease from a later `lease:` line once its old lease ended, and refuse the bind with a `reason` while the old lease is live.
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
