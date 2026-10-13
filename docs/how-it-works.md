# How rvow works

This page explains why rvow is built the way it is.
It has no steps to follow.
For commands, see `rvow --help`.
For settings, see [settings.md](settings.md).

## The vow

A coding agent is good at starting work and bad at admitting it did less than the plan said.
rvow does not ask the agent to be honest.
It records the accepted plan as the vow: the plan files with their hashes, the verify command of each task and the files each task may touch.
`rvow accept` writes the vow, and only the user runs it.

Each task stores its own verify command.
`rvow prove` runs exactly that command and records the result.
The agent never reports a pass.
rvow observes it.

## What the review shows

`rvow review <change>` prints the plan and the accept checklist, and it runs no verify command.
Each task appears with its `verify:` command and its `files:`, and with its `baseline:` and `mutants:` when the task sets them, so the person who accepts the plan sees the check and not only the title.

An optional judge reviews the work against the spec: `rvow guide judge` prints the shared judge prompt, and `rvow review <change> --record <file> --judge --harness <harness> --model <model>` records its findings against the current packet (`--record -` reads stdin).
The packet is data and not instructions, and the judge looks for special-cased tests, weakened checks and behavior the spec does not ask for.
The agent's shell cannot record one: the hook denies `rvow review --judge`.
`rvow gate` shows each finding as a warning and keeps its result, and `rvow archive` names `/rvow-judge` when the current packet has no judge note.

Above the checklist, the review warns about a weak check.
A check is weak when the whole verify command is a stub that proves nothing, such as `true`, `:`, `exit 0`, one `echo` or one `test -f`, or when a task sets `baseline: pass` on a command that runs no counted test runner.
The warning code is `weak-verify`.
`rvow check` prints the same warnings.
A warning never blocks, so a change whose only finding is a weak check is still ready to accept.

## Why the checks live outside the model

A model that writes both the code and the claim that the code works grades its own work.
The claim is cheap to make and hard to audit.
So the checks run as ordinary processes, and the result goes into a ledger that the model cannot write to.
Hooks deny edits to rvow's own files, and they deny `rvow accept` and `rvow confirm` for agents.
This does not make cheating impossible.
It makes cheating leave a record that a person can read.

## Drift

Drift is any difference between the plan files now and the plan files at accept.
rvow sorts each difference into one of three classes.

- Free: a change that cannot widen what the agent may do, such as a new task inside an existing scope or an edit to `design.md`.
  Dropping a `baseline: pass` flag or changing a timeout is free too: the check only gets stricter or slower.
- Confirm: a change that alters the plan, such as a new requirement, a changed title or a wider `files:` scope.
- Weaken: a change that could make a check easier to pass, such as a changed verify command, an edited test that a proof relied on or a changed config.
  A removed task is weaken as well: the work it demanded goes away with it.
  So is a task that gains `baseline: pass`: a check that had to fail first may now pass at once.

A changed config is drift only for a value that your config files set now or set at the baseline.
A new default value that comes with a newer rvow is no drift.
An older baseline does not record which values your files set, so there an unset value passes only when its old value was a default of an earlier rvow.
A placeholder that `rvow ids` replaced with an id that rvow minted for the same change is no drift either, when the item under the new id holds the same content.
`rvow amend` then runs no baseline for that task.
A protected test file that only grew is not drift.
Added tests and standalone added checks stay free.
A lost or changed line is weaken drift.

The gate fails on confirm and weaken drift until the user decides.
Free drift passes without a question, so small work does not wait for a person.
rvow cannot tell that an agent special-cased a tested input.
It can tell that a test file or a verify command changed, and that is where it asks.

## Amendments and the user-only confirm

The plan must be able to change, because plans are wrong.
The agent replaces the changed part with `rvow put` and runs `rvow amend` with a reason.
That records the edit as an amendment.
Before it records, `rvow amend` runs each added task's verify command and each changed verify command on the vow's base commit, in a clean worktree set up the same way as the gate rerun.
The base commit always comes from the vow, also after a confirmed amendment, so a second amend and the mutants diff keep working.
When the vow has no base commit, the amend error names a step for an active change such as `rvow block`.
`rvow amend` mints placeholder ids before it takes the snapshot, as accept does, so a new task keeps its real id after confirm and a later `rvow ids` mints nothing.
A check that passes there with tests run is refused with the code `baseline`, unless the task says `baseline: pass`, and a `baseline: pass` check that fails there is refused too.
Nothing is recorded when the baseline refuses.
Each run's task id, exit code and test count go into the amendment, and `rvow confirm` lists weaken items first with each baseline result, so the user sees which checks failed before the work.
The user reads each amendment with its reason and runs `rvow confirm` to accept or reject it.
With `require_signature` set, the decision also needs an ssh signature bound to the content.
In the Claude mod, `/rvow-confirm` with no argument or with `all` lists the pending edits and asks once whether to accept all, reject all or decide later.
It then decides everything with the digest of the listing that was read, so an edit that arrived later waits for the next round.
`/rvow-confirm accept <id>` and `/rvow-confirm reject <id>` decide those ids with no dialog.
A confirm that names an amendment that already has a decision fails with the code `decided` and names the amendment and its decision, such as `AMD-x is already accepted`.
An unknown id keeps the code `amendment`.

Confirm belongs to the user for one reason.
An agent that could approve its own amendment would make drift detection a formality.
The agent can do two things instead.
It can amend and wait, or it can run `rvow block` when a task cannot pass as written.

A pending amendment also decides the Stop hook.
The stop is held while an active change has an unproved or failed task, unless a pending amendment of that change names the task.
An amendment that only changed `design.md` releases nothing, while an amendment whose items name a task releases that task.
An amendment recorded before rvow wrote task names has no `tasks` key and releases its whole change.

## The proof and what stale means

A proof ties a passing run to what the run saw: the task, the requirements it covers, the content of the files in scope and the recorded environment.
A proof is stale when one of those has changed since the run.
The reasons are task changed, requirement changed, inputs changed, private policy changed and environment changed.
A stale task needs a new proof, and `rvow prove --stale` runs them all.

A task is flaky only when a pass and a failure share the same run key and the same worktree fingerprint.
Each prove event records a worktree fingerprint over tracked and untracked files that are not ignored, so a fix outside the task's files changes the fingerprint and clears the failure instead of marking it flaky.
A failure recorded before worktree fingerprints existed has none and does not count as flaky.

A task can also set `mutants:`, an extra check that runs after `verify:` passes.
rvow runs it with `RVOW_DIFF` set to a file holding the diff of the task's files against the vow's base commit, and `RVOW_CHANGED_FILES` listing the changed paths one per line, so `cargo mutants --in-diff "$RVOW_DIFF"` tests only the changed lines.
The diff covers committed changes since the base, uncommitted edits and untracked files in scope, and the file is removed after the run.
When the output holds a cargo-mutants summary, the proof records the missed count as `survived` inside the `mutants` object, and `rvow show <task>` prints `mutants: N survived` on that run's line.
A failing mutants run fails the task.

Stale is not failed.
It means the old evidence no longer covers the current tree.
`rvow gate --strict` fails on stale and flaky proofs.

## The gate

`rvow gate` checks the active changes of the branch.
It fails on unproved tasks, failed proofs, pending amendments, blocking drift and files changed outside every task scope.
The base it measures from is the newer merge base of the local and the remote base branch with HEAD, so commits you have not pushed yet never count as outside every task.
When the two merge bases diverge, the remote one wins.
A done change counts toward that scope only while the HEAD recorded at its archive is not a strict ancestor of the merge base.

Work written before the plan is accepted fails too: while a backlog plan names the branch and no change is active on it, before the plan is accepted, the gate fails on each file that differs from the base outside the rvow tree and outside every done change of the branch, and names the waiting plan.

`rvow gate --rerun` goes further.
It runs every verify command again in a clean worktree of HEAD, so a proof that depended on an uncommitted file or a local state fails.
The rerun skips a task whose proof is current and whose files are all committed: it was proved on this very content, so the state "proved" is what the push trusts.
It runs the tasks that are not proved, or whose files changed since the proof.
A task of an archived change is never rerun, because archiving needs every task proved and is a decision of the user.
Set `rerun_proven = true` to rerun proven tasks as well.
`rvow gate --out` writes the rerun results to a file, and `rvow ingest` adds them to the ledger of a home store.

A rerun skips a task when an earlier clean pass has the same key.
The key holds the content of the task's files at HEAD, the task and its requirements, the environment, the tools you list and the keys of the tasks it depends on.
A rebase or an amend does not change the key, but a changed byte in scope does.
The gate writes a pass only when the task ran tests, and signs each entry with a key that agents cannot read.
`rvow gate --rerun --no-cache` runs every task that is active on the branch, and so does `--out`.
A scoped `setup` or `checks` entry runs only when a task that runs or a changed file matches it, so a Python change does not rebuild the C++ code.
When the cache answers every task, the gate makes no worktree and runs no `setup`.
[docs/settings.md](settings.md) lists the keys that tune the cache.

## Seals

A ledger event is a line of text, so an agent that can write the file can write a proof.
rvow seals each proof with an HMAC under the 32-byte key in the rvow home.
The seal covers the event kind, its time, its parents and its data.
The key is closed to agents, so a hand-written proof or a copied one fails the check.

The gate trusts a proof only when its seal checks and the verify programs match.
A proof also records the resolved path and the SHA-256 of each program that the verify command starts, such as `cargo` or `pytest`.
A shell builtin is recorded as absent, so `test` and `echo` stay trusted.
When a program resolves to another path or another content, the gate reruns the task and names it.

A missing seal or a bad seal never fails the gate by itself.
The task reruns and the gate warns with its id.
A proof from before seals existed reruns once, and the cache answers it after that.

The yolo switch is sealed in the same way.
A yolo file without a valid seal turns every tier off.

### Limits of the seal

- The seal is as strong as the hook that hides the key.
  The hook reads text, so code that builds the key path at run time can still read the key.
- A deleted tail of the ledger is not detected.
  The last sealed pass that stays is a real pass on the same inputs, so the risk is a hidden flaky failure and not a fake pass.
- A fake tool in a folder that is already on `PATH` before the real one is not caught, because the rerun reads the same `PATH` as the proof.

## Attestations

A proof lives in the local ledger in rvow's own format, so no other tool can check it.
`rvow attest <change> --out <file>` exports the current proofs as in-toto statements, one per line.
Each task with a current proof gets one Test Result statement with its result, its verify command, its test count and each file in scope with its SHA-256.
A task with a stale proof or no proof gets no line.
The last line sums up the change with the vow hash, the newest confirm hash, the gate result and the tasks without a current proof.
`rvow attest <change> --out <file> --sign` also writes an ssh signature of the file in the `rvow` namespace with the `signing_key` of the global config.

## Subagents and leases

A subagent meets the same rules as the main agent, but it does not know which task is its own.
`rvow delegate <task>` records a lease for one task and prints a short brief.
The first line of the brief is `lease: <task id>`, so put the brief at the start of the subagent's prompt.
The Claude mod reads that line when the subagent starts and binds the new agent id to the lease.
In pi a subagent is a session of its own that loads the rvow extension, so the extension reads the same line from the first prompt of that session and binds the session id.
From then on the hook denies that subagent's edits of files outside the task's `files:`, and the reason names the task.
It also denies an `rvow prove` of any other task or `rvow prove --stale`, so one subagent cannot prove another's work.
A lease ends when its task is proved or blocked, when the subagent stops or after 4 hours.
The lease file is closed to agents, like the gate cache.

Two limits stay.
A subagent without the mod is unbound, so it meets the usual rules.
The hook sees files and not what a shell command writes, so a lease scopes the edit tools and `rvow prove` only.

## What a hook denies and what it only counts

A rule has a mode.
Tampering is enforced: the ledger, the enforcement files, the hidden checks, the git hooks, the cache, lease and stats files, `rvow accept` and `rvow confirm`, and edits of archived changes and living specs.
A lease that you chose with `rvow delegate` and an edit before the plan is accepted are enforced too.
An edit outside the files of every task and an edit of a protected test are audited.
The call goes through, `rvow stats` counts it, and the gate still blocks the branch: the first as a file outside every task scope and the second as weak drift that you must confirm.

After 3 denials in a row in one session, the next one is a question to you instead of a denial.
A tampering denial is never an ask, whatever the streak: it stays a denial and keeps the streak for the next non-tampering denial.
Any call that goes through ends the streak.
Without a session id or without a person to ask, the call stays denied.

## Yolo: relax the hooks for a while

`! rvow yolo <tier> [--for 2h]` relaxes the hooks for a while. In the Claude mod and in pi, type `/rvow-yolo <tier> [30m]` instead, and the agent is told once.
The default length is 2 hours and the longest is 24.
It ends by itself, and `! rvow yolo off` ends it at once.
Only you can switch it: an agent cannot run `rvow yolo` in a shell or write its file, in any tier.

| Tier | What changes |
|------|--------------|
| `audit` | An edit before the plan is accepted, and a subagent edit outside its lease, are audited and not denied. Tampering stays denied. |
| `auto` | Also lets the agent run `rvow accept` and `rvow confirm`. `rvow ingest` and a faked dialog answer stay denied. |
| `bypass` | Every hook only audits, and a stop is never held back. |

The status line shows `[yolo <tier>]` and the brief says so once.
`rvow stats` counts what each tier let through.
The gate is not relaxed: it still checks proofs, drift and scope at push.

## Tools for the agent

A slash command is for you, and a tool is for the agent.
Both the Claude mod and the pi extension give the agent these tools:

- `rvow_accept` asks you to accept a ready plan. You read the review and the checklist in a dialog, and nothing is accepted unless you say yes.
- `rvow_confirm` lists the pending plan edits, or asks you to accept or reject all of them in a dialog.
- `rvow_archive` asks you whether a finished change may be archived. It only asks when every task is proved and no plan edit is pending, and it names what is missing otherwise. An archived change is trusted, and a push never reruns its tasks.
- `rvow_yolo` shows or changes the yolo tier. You answer a dialog before any tier is switched on. `off` needs no dialog.
- `rvow_judge` runs the spec conformance judge for a change in a clean read-only session with no rvow extension, and records its findings with the model it ran on.
  In pi the judge session also starts with `--no-skills` and `--no-context-files`, so it loads no skill and no AGENTS.md or CLAUDE.md. Type `/rvow-judge [CHG-id] [model]` yourself instead.
- `rvow_ask`, `rvow_view` and `rvow_prove` read the open questions, the dashboard and prove a task.

The tool runs the command itself, so the hook that denies `rvow accept` in a shell does not stand in its way.
Your answer in the dialog is the decision.
When the command fails, the tool result holds its own words, such as `code: not-ready`, so the agent knows why.
When you run a decision yourself, with `/rvow-accept` or the dock, and it fails, the agent gets a message that names the command and the reason.

## Reaching every agent

An agent that never met rvow still finds it.
`rvow install` writes the rendered skill to `skills/rvow/SKILL.md` in the pi agent folder, the same text as the Claude skill.
A user edit of that file is kept like any other managed file, and the install check reports a stale copy.
The skill description starts with `Load before you` and names the trigger: an accepted change, a denied hook, a failed gate, a stale proof or a `CHG-` or `TSK-` id.
When the brief names a next task, one `loop:` line says to run `rvow prove <task>` after the work, to change the plan with `rvow amend` and the `rvow_confirm` tool, and to read the plan with `rvow show <change>`.
A branch with no active change and no backlog plan gets one line naming `rvow new` and `rvow guide proposal` instead.
`rvow next` prints the next task of the branch with its id, title, state, verify command and files, or says that no task is next.

## Measuring the hooks

The hooks should help and stay quiet, so rvow counts what they do.
Each denial and each stop is one line in `hook-stats.jsonl` in the project folder.
A line holds the time, the hook, the outcome (deny, audit, ask, block or pass), the session id and, for a denial, an audit or an ask, one rule name such as `owner-command`, `outside-scope` or `protected-test`.
It never holds a command or a path.
`rvow stats` prints the counts of the last 7 days, or of `--days N`, with the stop blocks per 100 stops.
Use it to find the rule that fires most before you change any rule.
Agents cannot read or write the stats file.

## Agent evals

The evals check that an agent with the mod follows the loop, instead of trusting the hooks alone.
Each case under `evals/agent/cases/` holds a prompt, a setup script and graders.
`evals/agent/run.sh` runs every case with the mod and with no mod on haiku by default.
The runner installs the mod with the built binary into a throwaway home, so your store and your Claude folder stay untouched, and writes the report to `evals/agent/results/`.
The offline test in `crates/rvow/tests/audit_evals.rs` runs each setup with no model and checks the scaffold it leaves behind.
Run one case live with `evals/agent/run.sh haiku --case <name> --runs 1` before you trust a new grader.

## Trigger evals

The trigger evals check that the skill loads when it must and stays quiet when it must not.
Each case under `evals/triggers/cases/` holds a prompt and a `fires.md` grader on the Skill tool.
`evals/triggers/run.sh` renders the skill with the built binary, builds a plugin that holds that skill and no hooks, and runs the cases on haiku by default with no baseline arm, capped at $3.
Results stay local under `evals/triggers/results/`.
The offline test in `crates/rvow/tests/audit_triggers.rs` builds the plugin with no model and checks its shape.
Run one case live with `evals/triggers/run.sh haiku --case <name> --runs 1` before you trust a new grader.
Change the skill description only from train failures, then check it on val.

## The pre-push gate

`rvow install` adds a pre-push hook that runs `rvow gate --strict --rerun`.
The hook sits at the last point where the agent cannot argue.
A push that carries stale or unproved work stops there.
The hook refuses to run an rvow binary that changed after install.

## One store per remote

rvow keeps its tree in a home store, keyed by the origin remote of the repository.
Every clone and every worktree of one remote share one store, so a plan survives a re-clone.
`rvow init --in-repo` keeps the tree inside the repository instead.

A change belongs to one repository and one branch.
The change names its branch in `proposal.md`, and the vow hashes files under that repository.
There is no change that spans several repositories, by design.
A cross-repository change would need one vow over two histories and two sets of files, and nothing could check that both moved together.

When work spans repositories, make one change in each.
Write the shared intent in each proposal and link the other change by id in the text.
Let each repository gate its own part.
`rvow bundle --out` and `rvow gate --bundle` carry one repository's checks into CI without the home store.

## Why one format and no custom schemas

Teams ask for custom schemas for specs, tasks and proposals.
rvow declines this (decision D3).
The failure that schemas try to fix is an agent that beautifies a format: it adds headings, rewrites bullets and drifts the structure until a tool cannot read it.
OpenSpec issue 824 describes this problem.
A schema moves the problem and adds a second one, because the agent must now learn each project's format.

rvow gives each file kind one fixed format and checks it with a strict parser.
A file that does not parse is an error with a line number and a fix.
The agent learns one format from `rvow guide`, and the parser rejects any beautified copy.
Project wording goes in `context` and `rules` instead, which change what the guide says and not what the parser reads.
See [authoring-context.md](authoring-context.md).

## Import and export

`rvow import openspec` converts an OpenSpec tree into specs and backlog changes.
The import is one way.
It keeps history and flags tasks that lack a verify command, and it never writes back to the OpenSpec tree.

`rvow export openspec` writes the living specs, the open changes and the done changes as an OpenSpec tree, so you can leave rvow.
The export is one way and never overwrites: it refuses a target that already holds an `openspec` folder, and `--dry-run` lists the files without writing them.
Each requirement keeps its rvow id in an HTML comment, and the report names what the tree loses, such as ledgers, proofs and vow hashes.
A re-import reads the text back exactly, and it does not read the id comment or the deprecated marker, so a deprecated requirement comes back as an ordinary one with a new id.

## Known limits

These are accepted for now.
Each one is caught later by the gate, or needs a person.

- **The accept and confirm deny reads text.**
  It follows `sh -c`, `eval`, wrappers, variables and scripts on disk.
  It does not follow a name built at run time, a git alias or a script written in the same command.
  A committed test file with no change against HEAD is not read, so its text as data does not count.
  `require_signature` is the real boundary, and it is off by default.
- **Hardware signing keys are untested.**
  The tests sign with a software key. A hardware key (`ed25519-sk`) has not been used yet.
- **A timeout stops rvow and not its child.**
  The pi tool `rvow_prove` ends the rvow process after its timeout, and the verify command that rvow started can keep running.
- **`config.toml` has no edit deny.**
  A change to it is caught as weaken drift, in repository mode and in the global file.
- **A subagent can edit a protected test block.**
  The hook sees files and not the test blocks inside a Rust file, so the edit is found afterwards as weaken drift.
- **A lease needs the rvow hooks in the subagent.**
  A Claude subagent is bound by the mod, and a pi subagent binds itself when its session loads the rvow extension, which is the default of `pi-subagents`.
  A subagent that opts out (`extensions: false` or `isolated: true` in pi, or no mod in Claude Code) meets no rvow hook at all, so the lease does not apply and only the gate checks its work.
  The hook also judges only the edit tools and `rvow prove`, since it sees files and not what a shell command writes.
- **Session events are recorded from the docs and this session.**
  Claude Code names five `SessionStart` sources: `startup`, `resume`, `clear`, `compact` and `fork` (a fork reported `resume` before v2.1.214).
  The mod resets its brief state on `session.start`, which covers startup, resume, clear and fork, and on a `session.compact` that stands, so the next prompt carries the brief again after the summary replaced it.
  A skipped, `precompute` or subagent compaction leaves the brief alone.
  The Claude Code docs say that edits of settings hooks are normally picked up by a file watcher, and they do not say that hooks are read once at start.
  The mod is a plugin and not a settings hook, so `/reload-plugins` is what loads a new install.
- **A subagent's tool calls reach the same hooks.**
  `PreToolUse` fires for them with `agent_id` and `agent_type`, which is how a lease binds the subagent.
- **The shell hook reads text, and it reads in two ways.**
  A command that only reads the ledger, the ids, the stats, lease, yolo or cache file is allowed, and so is a command that only prints text that names the hidden checks, such as an `echo` or a commit message.
  A command that can write those files, or that reads the hidden checks, is denied, even when the name sits inside a string passed to an interpreter such as `python`, because the hook cannot tell a string from a path there.
  A shell word that holds `*`, `?` or `[` is expanded from the call's working folder, and a glob that expands to a protected file is denied under that file's rule; a glob that matches nothing is allowed.
  A command that only reads through such a glob is allowed, since a read changes nothing that rvow trusts.
  A glob that reaches the hidden checks stays denied, also for a read.

## What we measured

On 2026-10-03 we counted ledger events per change with `rvow ls` and `rvow show` on the maintainer's own store.
The store held 20 changes: 3 active, 15 done and 2 dropped.

- 7 of the 18 changes that were accepted needed an amendment.
  The 11 others needed none.
- 25 amendments were recorded and 25 confirmations followed.
  Each recorded amendment got a decision.
- 19 of the 25 amendments fell in the 3 largest changes, which each held 22 to 106 proofs.
  CHG-rq37ny had 9, CHG-d9hn4h had 6 and CHG-5wkawy had 4.
- The 4 remaining changes with amendments had 1 or 2 each.
- 5 blocks were recorded, all in CHG-d9hn4h.
  No other change used `rvow block`.

Our reading is that drift friction grows with the size of a change.
Most small changes finish with no amendment, and the largest changes carry most of the amendments.

We cannot say which drift class caused each amendment.
`rvow show` prints counts, not the class or the reason of an amendment.
The reasons live in the amendment records that `rvow confirm` lists, and we did not run that command while measuring.
This section is a count of events, so we draw no conclusion about the classes until they are recorded.
