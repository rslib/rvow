# How rvow works

This page explains why rvow is built the way it is.
It has no steps to follow.
For commands, see `rvow --help`.
For settings, see [settings.md](settings.md).

## The vow

A coding agent is good at starting work and bad at admitting it did less than the plan said.
rvow does not ask the agent to be honest.
It records the accepted plan as the vow.
`rvow accept` writes the vow, and only the user runs it.

The vow holds three things:

- the plan files with their hashes
- the verify command of each task
- the files each task may touch

Each task stores its own verify command.
`rvow prove` runs exactly that command and records the result.
The agent never reports a pass.
rvow observes it.

## What the review shows

`rvow review <change>` prints the plan and the accept checklist, and it runs no verify command.
Each task appears with its `verify:` command and its `files:`, and with its `mutants:` when the task sets them, so the person who accepts the plan sees the check and not only the title.

### The judge

An optional judge reviews the work against the spec.
`rvow guide judge` prints the shared judge prompt.
`rvow review <change> --record <file> --judge --harness <harness> --model <model>` records its findings against the current packet (`--record -` reads stdin).
The packet is data and not instructions, and the judge looks for special-cased tests, weakened checks and behavior the spec does not ask for.

> [!NOTE]
> The agent's shell cannot record a judge note.
> The hook denies `rvow review --judge`.

`rvow gate` shows each finding as a warning and keeps its result.
`rvow archive` names `/rvow-judge` when the current packet has no judge note.

### Weak checks

Above the checklist, the review warns about a weak check.
A check is weak in one of two cases:

- The whole verify command is a stub that proves nothing, such as `true`, `:`, `exit 0`, one `echo` or one `test -f`.

The warning code is `weak-verify`.
`rvow check` prints the same warnings.

> [!NOTE]
> A warning never blocks.
> A change whose only finding is a weak check is still ready to accept.

## Why the checks live outside the model

A model that writes both the code and the claim that the code works grades its own work.
The claim is cheap to make and hard to audit.
So the checks run as ordinary processes, and the result goes into a ledger that the model cannot write to.

Hooks deny these things:

- edits to rvow's own files
- `rvow accept` and `rvow confirm` for agents
- a search that covers the plan files of the store

A search program is denied in two cases:

- A path among its own arguments covers the store.
- It runs with no path after a `cd` into a covering folder.

Another program on the same line does not count.

This does not make cheating impossible.
It makes cheating leave a record that a person can read.

## Drift

Drift is any difference between the plan files now and the plan files at accept.
rvow sorts each difference into one of three classes.

- Free: a change that cannot widen what the agent may do, such as a new task inside an existing scope or an edit to `design.md`.
  Changing a timeout is free too.
  The check only gets stricter or slower.
- Confirm: a change that alters the plan, such as a new requirement, a changed title or a wider `files:` scope.
- Weaken: a change that could make a check easier to pass, such as a changed verify command, an edited test that a proof relied on or a changed config.
  A removed task is weaken as well, because the work it demanded goes away with it.

### What is no drift

A changed config is drift only for a value that your config files set now or set at the baseline.
A new default value that comes with a newer rvow is no drift.
An older baseline does not record which values your files set, so there an unset value passes only when its old value was a default of an earlier rvow.

A placeholder that `rvow ids` replaced with an id that rvow minted for the same change is no drift either, when the item under the new id holds the same content.

- A protected test file that only grew is not drift.
- Added tests and standalone added checks stay free.
- A lost or changed line is weaken drift.

### What the gate does with drift

The gate fails on confirm and weaken drift until the user decides.
Free drift passes without a question, so small work does not wait for a person.

> [!WARNING]
> rvow cannot tell that an agent special-cased a tested input.
> It can tell that a test file or a verify command changed, and that is where it asks.

## Amendments and the user-only confirm

The plan must be able to change, because plans are wrong.
The agent replaces the changed part with `rvow put` and runs `rvow amend` with a reason.
That records the edit as an amendment.

### What `rvow amend` does

- `rvow amend` runs no verify command.
- `rvow amend` mints placeholder ids before it takes the snapshot, as accept does, so a new task keeps its real id after confirm and a later `rvow ids` mints nothing.

The agent records each plan edit with `rvow amend --reason` and keeps working.
The Stop hook and the brief name only that step.

### The user confirms in the archive dialog

The user confirms every pending amendment of the change once, in the archive dialog.
The archive tool of the Claude mod and pi lists them, amendments with weaken items first, and a yes accepts them by id and then archives.
A no leaves them pending and the change active.
An amendment recorded while the dialog is open makes it ask again, so no unseen amendment is accepted.

> [!NOTE]
> The push gate keeps failing while any amendment is pending, so nothing unreviewed leaves the machine.

`rvow amend --unlock` keeps its own confirm, since a protected test opens only after the user says yes.

### Deciding earlier

A user who wants to decide earlier reads each amendment with its reason and runs `rvow confirm` to accept or reject it.
`rvow confirm` lists weaken items first.

> [!NOTE]
> With `require_signature` set, the decision also needs an ssh signature bound to the content.

### The confirm dialog of the Claude mod

In the Claude mod, `/rvow-confirm` with no argument or with `all` lists the pending edits and asks once whether to accept all, reject all or decide later.

- With one amendment pending the dialog is a yes or no question that shows its id, reason and items.
- With more than one pending, the dialog lists each amendment with its id, reason and items and asks about each in turn, with the answers accept, reject or later.

Each answer runs `rvow confirm <AMD-id> --accept` or `--reject`, and a later leaves that amendment pending, so an edit that arrived later waits for the next round.
The `rvow_confirm` tool takes an optional `id` and then asks only about that amendment.

### Reading and deciding by id

`rvow show AMD-x` prints an amendment of an active or archived change with its change, its reason, its items and its decision, which is pending, accepted or rejected.
An unknown AMD- id fails with `no id matches`.
`/rvow-confirm accept <id>` and `/rvow-confirm reject <id>` decide those ids with no dialog.

- A confirm that names an amendment that already has a decision fails with the code `decided` and names the amendment and its decision, such as `AMD-x is already accepted`.
- An unknown id keeps the code `amendment`.

### Why only the user confirms

Confirm belongs to the user for one reason.
An agent that could approve its own amendment would make drift detection a formality.

The agent can do two things instead:

- It can amend and wait.
- It can run `rvow block` when a task cannot pass as written.

## The stop holds only the work of the session

The Stop hook holds a session only for work that session started.

### What the hooks record

The hooks record each file that a session changed:

- the files of its Edit and Write calls
- the file names in a shell command that does not only read

For a shell command they read these things:

- its path and file-name words
- the files its globs match
- the value of an option such as `--out=report.txt`
- the path-like text of heredoc bodies and inline scripts such as `python3 -c "..."` when an interpreter or a shell runs that text

These things name no changed file:

- The text given to any other program, such as `rvow put`, `rvow note`, `cat` or `git commit -m`, is data.
- An `rvow` command records no file.
- A `cd` of the line only moves the folder those words resolve from.
- Folders themselves are never recorded.
- A formatter run such as `cargo fmt` records nothing beyond the files it names.
- Git read commands such as `git diff` and `sed` without `-i` change nothing.

`rvow put` counts as a change of the plan.
A subagent's edits count for that subagent, not for the main session that spawned it.

> [!NOTE]
> Script text counts only where it names an existing file, or a path with a `/` whose folder exists.
> `src/a.rs` counts, while code words such as `stack.pop` do not.

### How the hooks track a change

Around each checked call that may write, the hooks snapshot the work tree with a private git index and object store.
They record each changed path with its new git blob id, or deleted, for the session or the subagent of the call.
rvow sorts a tool once by its name, with the tool names of Claude Code, pi, OpenCode, Codex and Gemini CLI in one table.
The classes are an editor, a shell, a read-only tool and any other tool.

Each class takes snapshots in its own way:

- An editor or a shell write takes a snapshot before and after the call.
  An editor call that names its files hashes only those files, so its cost does not grow with the repository.
  When git rejects a named file, such as an ignored one, that call takes a full snapshot.
- Any other tool that may write, such as an MCP tool or codemode, takes one snapshot after it and compares it with the session's last recorded tree.
- A read-only tool takes none.
  A read takes no snapshot.

A change that no call recorded, such as one from a background job, counts for the session at its next shell call, which `rvow prove` is, and at the Stop hook.

A session whose hooks send no after-call record, such as an older mod that sent only pre-tool-use, falls back to today's text record.

### The private store

The private store lists the repository's own objects as an alternate and lives under the project folder, so nothing is written into the user's `.git`.
The private objects are transient.
The records keep only blob ids, and when the private store passes 64 MiB and no call is in flight, it is deleted and rebuilt from the repository's own objects.
Records are dropped after 7 days, and a call that never reports back (cancelled or killed) stops counting as in flight after 6 hours.

### Who owns a file

A session owns a file only when both of these hold:

- The file's current blob is the blob its latest call on that file produced, or the file is still gone after the session deleted it.
- No call of another session or of a subagent that overlapped that call in time recorded the same path and blob.

Overlapping same-blob calls leave no owner.

### When a session is held

A session is held only for a task or drift item whose file it changed.

- It is held for a task with no proof when it changed a file that the `files:` of the task match.
- It is held for drift when it changed a file that a drift item names or a protected file.

A session that changed no file, one that only read, a subagent and a call with no session id are never held.
A session held for more than one item gets one block that lists every open item: at most 3, then `and N more`.
The same block is given once to a session.

> [!NOTE]
> A session is not held for a task while the task has a live lease that another worker holds, or that no worker took yet.
> The session handed the task off with `rvow delegate`.
> When the lease ends, the hold comes back, and the session that holds the lease is held as before.

### A pending amendment releases the stop

While a change has any pending amendment, the Stop hook holds nothing for that change, since only the user can decide it.
Stale proofs never hold a stop.
They are left to the push gate, and `rvow gate --strict` still fails on them.

## The baseline: the code at accept

The baseline of a task is the code at accept.
rvow runs nothing for it, records nothing and holds no task back for it.
`rvow accept` runs no verify command, so a slow build never stands between the accept and the first task.

- The first `rvow prove` is an ordinary proof: a pass proves the task and a fail fails it.
- A check that fails before the work and passes after it is the best proof, but rvow does not demand the failing run.
  The agent writes the test first because that is the sound order, not because a hook asks for it.
- The first source edit of a task is never denied.
- No prove builds the base commit.
- A line `baseline: pass` in an older plan is read and ignored.
- When a task compares with the old state, such as a speed or a size, the agent measures the old number before the work and writes it in the task or in a note.

A runner that writes a JUnit report holds the proof to it: the report must hold one passed case and no failed one.
A vow from an older rvow may name failed cases, and the proof then needs those to pass.

## The proof and what stale means

A proof ties a passing run to what the run saw:

- the task
- the requirements it covers
- the content of the files in scope
- the recorded environment

A proof is stale when one of those has changed since the run.
The reasons are task changed, requirement changed, inputs changed, private policy changed and environment changed.
A stale task needs a new proof, and `rvow prove --stale` runs them all.

### Re-proving

`rvow archive` re-proves the stale tasks of the change before its gate, outside the project lock, and a failed re-proof stops the archive with the code `gate`.
When the agent's turn ends and every task of a change on the branch is proved or stale, the Stop hook starts `rvow reprove` in the background, so the archive usually finds the proofs current.
Its output is in `hook-state/reprove.log` in the project folder, and `rvow archive` waits for a run that is still going.

> [!NOTE]
> A proof whose task files changed while its check ran is not recorded.

### Flaky tasks

A task is flaky only when a pass and a failure share the same run key and the same worktree fingerprint.
Each prove event records a worktree fingerprint over tracked and untracked files that are not ignored, so a fix outside the task's files changes the fingerprint and clears the failure instead of marking it flaky.
The fingerprint also records which files git does not track, so a failure that a `git add` fixes does not mark the task flaky.
A failure recorded before worktree fingerprints existed has none and does not count as flaky.

### Mutants

A task can also set `mutants:`, an extra check that runs after `verify:` passes.
rvow runs it with two variables:

- `RVOW_DIFF` is set to a file holding the diff of the task's files against the vow's base commit.
- `RVOW_CHANGED_FILES` lists the changed paths one per line.

So `cargo mutants --in-diff "$RVOW_DIFF"` tests only the changed lines.
The diff covers committed changes since the base, uncommitted edits and untracked files in scope, and the file is removed after the run.
When the output holds a cargo-mutants summary, the proof records the missed count as `survived` inside the `mutants` object, and `rvow show <task>` prints `mutants: N survived` on that run's line.
A failing mutants run fails the task.

### Stale is not failed

Stale is not failed.
It means the old evidence no longer covers the current tree.

> [!IMPORTANT]
> `rvow gate --strict` fails on stale and flaky proofs.

## The gate

`rvow gate` checks the active changes of the branch.
It fails on these things:

- unproved tasks
- failed proofs
- pending amendments
- blocking drift
- files changed outside every task scope

### The base of the scope

The base it measures from is the newer merge base of the local and the remote base branch with HEAD, so commits you have not pushed yet never count as outside every task.
When the two merge bases diverge, the remote one wins.
A done change counts toward that scope only while the HEAD recorded at its archive is not a strict ancestor of the merge base.

In a stacked branch, a done or active change of another branch counts too, when the tip of that branch is an ancestor of HEAD and not an ancestor of the merge base, so the work of the parent branch is not outside every task.

- A sibling branch, or a parent that is already in the base, does not count.
- A branch whose tip is HEAD itself counts too, since a parent with no commit on the child yet looks the same.

### Work before the accept

Work written before the plan is accepted fails too.
While a backlog plan names the branch and no change is active on it, before the plan is accepted, the gate fails on each file that differs from the base outside the rvow tree and outside every done change of the branch, and names the waiting plan.

### The rerun

`rvow gate --rerun` goes further.
It runs every verify command again in a clean worktree of HEAD, so a proof that depended on an uncommitted file or a local state fails.

- The rerun skips a task whose proof is current and whose files are all committed.
  It was proved on this very content, so the state "proved" is what the push trusts.
- It runs the tasks that are not proved, or whose files changed since the proof.
- A task of an archived change is never rerun, because archiving needs every task proved, after it re-proves the stale ones, and is a decision of the user.

> [!TIP]
> Set `rerun_proven = true` to rerun proven tasks as well.

`rvow gate --out` writes the rerun results to a file, and `rvow ingest` adds them to the ledger of a home store.

### The rerun cache

A rerun skips a task when an earlier clean pass has the same key.
The key holds these things:

- the content of the task's files at HEAD
- the task and its requirements
- the environment
- the tools you list
- the keys of the tasks it depends on

A rebase or an amend does not change the key, but a changed byte in scope does.
The gate writes a pass only when the task ran tests, and signs each entry with a key that agents cannot read.

- `rvow gate --rerun --no-cache` runs every task that is active on the branch, and so does `--out`.
- A scoped `setup` or `checks` entry runs only when a task that runs or a changed file matches it, so a Python change does not rebuild the C++ code.
- When the cache answers every task, the gate makes no worktree and runs no `setup`.

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

> [!NOTE]
> A missing seal or a bad seal never fails the gate by itself.
> The task reruns and the gate warns with its id.
> A proof from before seals existed reruns once, and the cache answers it after that.

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

- Each task with a current proof gets one Test Result statement with its result, its verify command, its test count and each file in scope with its SHA-256.
- A task with a stale proof or no proof gets no line.
- The last line sums up the change with the vow hash, the newest confirm hash, the gate result and the tasks without a current proof.

`rvow attest <change> --out <file> --sign` also writes an ssh signature of the file in the `rvow` namespace with the `signing_key` of the global config.

## Subagents and leases

A subagent meets the same rules as the main agent, but it does not know which task is its own.
`rvow delegate <task>` records a lease for one task and prints a short brief.
The first line of the brief is `lease: <task id>`, so put the brief at the start of the subagent's prompt.
The Claude mod reads that line when the subagent starts and binds the new agent id to the lease.

In pi a subagent is a session of its own that loads the rvow extension, so the extension reads the same line from a prompt of that session and binds the session id.

### What a lease denies

From then on the hook denies these things:

- that subagent's edits of files outside the task's `files:`, and the reason names the task
- an `rvow prove` of any other task or `rvow prove --stale`

So one subagent cannot prove another's work.

### When a lease ends

A lease ends when its task is proved or blocked, when the subagent stops or after 4 hours.

> [!NOTE]
> A prove or block ends the lease for good, bound or not, so the task going stale later does not bring it back.

### A new lease in pi

A pi session takes the next lease from a later `lease:` line when its old lease ended.
The hook binds the session to the new task and judges its edits by that task.
When the old lease is still live, the hook refuses the bind, the session stays on the old task and the user sees `this session still holds the lease of <task>; prove or block <task>, or start a new session`.

> [!NOTE]
> The lease file is closed to agents, like the gate cache.

### Limits of a lease

Two limits stay:

- A subagent without the mod is unbound, so it meets the usual rules.
- The hook judges a shell command only by the files it names as write targets, so a lease does not cover a file that a program writes without naming it, such as a build output.

## What a hook denies and what it only counts

A rule has a mode.

### Enforced rules

Tampering is enforced:

- the ledger
- the enforcement files
- the hidden checks
- the git hooks
- the cache, lease and stats files
- `rvow accept` and `rvow confirm`
- edits of archived changes and living specs

These are enforced too:

- a lease that you chose with `rvow delegate`
- an edit before the plan is accepted

### Audited rules

These are audited:

- An edit outside the files of every task.
  The gate blocks the branch for it as a file outside every task scope.
- An edit of a protected test.
  The gate blocks the branch for it as weak drift that you must confirm.

An audited call goes through, and `rvow stats` counts it.

### The denial streak

After 3 denials in a row in one session, the next one is a question to you instead of a denial.
Any call that goes through ends the streak.
Without a session id or without a person to ask, the call stays denied.

> [!IMPORTANT]
> A tampering denial is never an ask, whatever the streak.
> It stays a denial and keeps the streak for the next non-tampering denial.

### Shell commands and other tools

A shell command is judged like an edit for each repository file it names as a target, such as `echo x > src/a.rs` or `sed -i ... src/b.rs`.

- Before the plan is accepted, it is denied as `unaccepted-plan`.
- From a leased subagent outside its task, it is denied as `lease-scope`.

The Monitor tool runs a shell command, so its command is judged as a Bash command.
An MCP tool call whose arguments set `no_verify` to true or name `core.hooksPath` is denied as `git-hooks`.

### When the hook fails

A hook that cannot run must not turn the checks off.
The Claude mod and the pi extension deny each tool that writes, with the error, and let the reading tools run, in each of these cases:

- The hook cannot start.
- The hook exits with an error.
- The hook prints text that is not JSON.
- The hook runs past its 10 s limit.

The reading tools that run are Read, Grep and Glob in the Claude mod, and read, grep, find and ls in pi.
The status line says so until a later call works.

> [!NOTE]
> A repository with no rvow store, and a folder outside git, are not projects, so nothing there is denied.

A snapshot takes the snapshot lock of its worktree, not the project lock.
So a prove, a lease or an agent in another worktree never makes it wait.
The hook waits at most 5 s for the snapshot lock.
When another rvow process holds it longer, a writing call is denied as busy and a read goes on.

### What the user made

The hooks read the command text, so a copy of the rvow binary under another name can run `accept` or `yolo` unseen.
So the Claude mod and the pi extension keep a record of what you did in the session and send it with each call as `user_made`.
The record holds these things:

- each change you accepted
- each yolo tier you switched on
- the time span of each confirm that the mod ran for you and that accepted

> [!IMPORTANT]
> An accept or a confirm made since the session started that is not in the record keeps every call that writes in the repository denied as `unseen-accept`.
> It stays so until you check the change and type `/rvow-accept <id>`.

The hook saves the record of each call to `witness.json` in the project folder and joins it with the record that the call brings.
So an accept that you made in one session counts in every other session of the project, also in one that was already running.
Only the hook call of the harness writes the file, and the hooks deny a shell call that names it or that runs `rvow hook`.

The check looks at where a call writes.
A call that writes nothing in the repository goes on, such as `ssh`, a file in `/tmp` or a scratch folder, or a script run from there.
A script that edits the repository is not seen before it runs, so the gate and the provenance check catch it as drift.

- A yolo tier switched on since the session started counts only when you switched it on with `/rvow-yolo` or the yolo dialog.
- An accept under a yolo `auto` tier that you switched on counts, so a run with no dialog still works.
- An accept or a yolo tier from before the session started counts.
- A harness that sends no `user_made` field keeps the old rules.

## Yolo: relax the hooks for a while

`! rvow yolo <tier> [--for 2h]` relaxes the hooks for a while.
In the Claude mod and in pi, type `/rvow-yolo <tier> [30m]` instead, and the agent is told once.
There, a tier that `! rvow yolo` switches on during the session does not count, since the mod cannot tell it from a copied binary (see [What the user made](#what-the-user-made)).

- The default length is 2 hours and the longest is 24.
- It ends by itself, and `! rvow yolo off` ends it at once.

> [!IMPORTANT]
> Only you can switch it.
> An agent cannot run `rvow yolo` in a shell or write its file, in any tier.

| Tier | What changes |
|------|--------------|
| `audit` | An edit before the plan is accepted and a subagent edit outside its lease are audited and not denied. Tampering stays denied. |
| `auto` | Also lets the agent run `rvow accept` and `rvow confirm`. `rvow ingest` and a faked dialog answer stay denied. |
| `bypass` | Every hook only audits, and a stop is never held back. |

The status line shows `[yolo <tier>]` and the brief says so once.
`rvow stats` counts what each tier let through.

> [!NOTE]
> The gate is not relaxed.
> It still checks proofs, drift and scope at push.

## Tools for the agent

A slash command is for you, and a tool is for the agent.
Both the Claude mod and the pi extension give the agent these tools:

- `rvow_accept` asks you to accept a ready plan. You read the review and the checklist in a dialog, and nothing is accepted unless you say yes.
- `rvow_confirm` lists the pending plan edits, or asks you to accept or reject all of them in a dialog.
- `rvow_archive` asks you whether a finished change may be archived. It only asks when every task is proved or stale and no plan edit is pending, and it names what is missing otherwise, such as a missing, failed, flaky or blocked task. `rvow archive` then re-proves the stale tasks. An archived change is trusted, and a push never reruns its tasks.
- `rvow_yolo` shows or changes the yolo tier. You answer a dialog before any tier is switched on. `off` needs no dialog.
- `rvow_judge` runs the spec conformance judge for a change in a clean read-only session with no rvow extension, and records its findings with the model it ran on.
  In pi the judge session also starts with `--no-skills` and `--no-context-files`, so it loads no skill and no AGENTS.md or CLAUDE.md.
  Type `/rvow-judge [CHG-id] [model]` yourself instead.
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

### The brief

When the brief names a next task, one `loop:` line says to do three things:

- Run `rvow prove <task>` before the work and after it.
- Change the plan with `rvow amend --reason`.
- Read the plan with `rvow show <change>`.

rvow sends the same brief once per session.
It keeps the last brief it sent in `hook-state/brief-<session>` and sends nothing when the brief is the same.
It also sends nothing when only the counts changed, so a proof by another agent does not reach this session.

When the next task changes and the rest stays the same, it sends only the brief from the `next:` line to the end.
After a `compact` or a `clear`, or when a title, a note or a warning changes, it sends the full brief again.
A session that holds a live lease gets a brief that names its leased task as `next:`, so another agent's work does not move it.
A hook call with no session id gets the full brief each time, so an older mod keeps working.

A branch with no active change and no backlog plan gets one line naming `rvow new` and `rvow guide proposal` instead.

### The next task

`rvow next` prints the next task of the branch with its id, title, state, verify command and files, or says that no task is next.

## Measuring the hooks

The hooks should help and stay quiet, so rvow counts what they do.
Each denial and each stop is one line in `hook-stats.jsonl` in the project folder.

### What a line holds

A line holds these fields:

- the time
- the hook
- the outcome (deny, audit, ask, block or pass)
- the session id
- for a denial, an audit or an ask, one rule name such as `owner-command`, `outside-scope` or `protected-test`

A stop block also holds a `reason`, `drift` or `no-proof`.
It never holds a command or a path.

### What `rvow stats` prints

`rvow stats` prints the counts of the last 7 days, or of `--days N`, with the stop blocks per 100 stops and a `stop_reasons` list that counts the blocks of each reason.
A block line written before reasons existed counts as `unknown`.

### Durations

Each hook call that has a project also adds one duration line with the time, the hook, the session id and `duration_ms`, and nothing else.
`rvow stats` lists the median and the 95th percentile duration of each hook in a `hooks` list.
The percentile is the nearest rank of the sorted durations, so 20 calls of 10 ms and one of 200 ms give a median and a 95th percentile of 10 ms.
Duration lines are not events.

When the file is larger than 1 MiB, the next duration append moves it whole to `hook-state/hook-stats.old.jsonl`, replacing the copy before, and `rvow stats` reads both files.
A move never rewrites a line, so a line that another hook writes at the same time still counts.

> [!TIP]
> Use it to find the rule that fires most before you change any rule.

> [!NOTE]
> Agents cannot read or write the stats file.

## Agent evals

The evals check that an agent with the mod follows the loop, instead of trusting the hooks alone.
Each case under `evals/agent/cases/` holds a prompt, a setup script and graders.

- `evals/agent/run.sh` runs every case with the mod and with no mod on haiku by default.
- The runner installs the mod with the built binary into a throwaway home, so your store and your Claude folder stay untouched, and writes the report to `evals/agent/results/`.
- The offline test in `crates/rvow/tests/suite/audit_evals.rs` runs each setup with no model and checks the scaffold it leaves behind.

> [!TIP]
> Run one case live with `evals/agent/run.sh haiku --case <name> --runs 1` before you trust a new grader.

## Trigger evals

The trigger evals check that the skill loads when it must and stays quiet when it must not.
Each case under `evals/triggers/cases/` holds a prompt and a `fires.md` grader on the Skill tool.

- `evals/triggers/run.sh` renders the skill with the built binary, builds a plugin that holds that skill and no hooks, and runs the cases on haiku by default with no baseline arm, capped at $3.
- Results stay local under `evals/triggers/results/`.
- The offline test in `crates/rvow/tests/suite/audit_triggers.rs` builds the plugin with no model and checks its shape.

> [!TIP]
> Run one case live with `evals/triggers/run.sh haiku --case <name> --runs 1` before you trust a new grader.

> [!IMPORTANT]
> Change the skill description only from train failures, then check it on val.

## The pre-push gate

`rvow install` adds a pre-push hook that runs `rvow gate --strict --rerun`.
The hook sits at the last point where the agent cannot argue.
A push that carries stale or unproved work stops there.
The hook refuses to run an rvow binary that changed after install.

### A hook of another tool

When `.git/hooks/pre-push` already holds a hook of another tool, `rvow install` moves it to `.git/hooks/pre-push.rvow-prev` and writes its own hook.
The rvow hook runs the old hook first, with the same arguments and the same pushed refs on stdin.
When the old hook fails, the push stops and the gate does not run.

> [!NOTE]
> The old hook cannot skip the gate.
> The rvow hook is one shell group, so the shell reads all of it before the old hook runs, and the old hook can only fail the push.

`rvow install` again finds its own hook and changes nothing.

### A hook that a tool rewrites

When a tool rewrites `.git/hooks/pre-push` later, `rvow gate` and `rvow archive` fail with `installed pre-push hook <path> is missing or was changed; run rvow install again`.
`rvow install` then moves the new hook to `pre-push.rvow-prev` in place of the old copy.

### Under core.hooksPath

Under `core.hooksPath` another hook manager owns the folder, so `rvow install` prints a line to add as the first command of its pre-push hook:

```sh
'/path/to/rvow' gate --strict --rerun || exit 1
```

The gate accepts that hook only when the line is its first command and the hook runs as a shell script, so nothing before it can skip the gate.
It also accepts a hook whose last command runs the gate and whose earlier lines are plain `npx`, `npm`, `pnpm` or `yarn` commands.

## One store per remote

rvow keeps its tree in a home store, keyed by the origin remote of the repository.
Every clone and every worktree of one remote share one store, so a plan survives a re-clone.
`rvow init --in-repo` keeps the tree inside the repository instead.

### One repository per change

A change belongs to one repository and one branch.
The change names its branch in `proposal.md`, and the vow hashes files under that repository.
There is no change that spans several repositories, by design.
A cross-repository change would need one vow over two histories and two sets of files, and nothing could check that both moved together.

### Work that spans repositories

When work spans repositories, make one change in each.

- Write the shared intent in each proposal and link the other change by id in the text.
- Let each repository gate its own part.

`rvow bundle --out` and `rvow gate --bundle` carry one repository's checks into CI without the home store.

## Working in git worktrees

In home mode one store serves every worktree of a repository, so a plan made in one worktree shows in all of them.
`rvow accept`, `rvow amend`, `rvow archive`, `rvow check` and `rvow clarify` run in the worktree where the branch of the change is checked out, even when you start them from another worktree.
The base commit, the files and the fingerprints come from that worktree.

### Check and clarify

A `rvow check` or `rvow clarify` that names a change reads the worktree of that change's branch, so it reports the same from the main checkout as from the worktree of the branch.
With no change named, `rvow check` keeps the changes of the branch of the current worktree.

> [!NOTE]
> When no worktree has the branch, accept fails and names `git checkout <branch>` and `git worktree add <path> <branch>`.

### Sessions in a worktree

A session in a worktree is judged by the change of that worktree's branch.
Its hooks, its stop hold and the leases it delegates all use that change, and a change of another branch does not count.
`rvow confirm` reads no folder, so it works from any worktree.

> [!NOTE]
> `rvow init --in-repo` keeps one `rvow/` folder per checkout, so those checkouts do not share a store.

## Why one format and no custom schemas

Teams ask for custom schemas for specs, tasks and proposals.
rvow declines this (decision D3).

The failure that schemas try to fix is an agent that beautifies a format.
It adds headings, rewrites bullets and drifts the structure until a tool cannot read it.
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
The export is one way and never overwrites.

- It refuses a target that already holds an `openspec` folder.
- `--dry-run` lists the files without writing them.
- Each requirement keeps its rvow id in an HTML comment.
- The report names what the tree loses, such as ledgers, proofs and vow hashes.

> [!WARNING]
> A re-import reads the text back exactly, and it does not read the id comment or the deprecated marker.
> A deprecated requirement comes back as an ordinary one with a new id.

## Known limits

These are accepted for now.
Each one is caught later by the gate, or needs a person.

- **The accept and confirm deny reads text.**
  It follows `sh -c` in every common shell, `fish -c`, `eval`, wrappers, variables, awk programs, `git -c alias` and scripts on disk.
  It does not follow a name built at run time or a script written in the same command.
  The user record catches an accept or yolo that gets past it in the Claude mod and pi, but a record forged in an earlier session is not caught.
  A committed test file with no change against HEAD is not read, so its text as data does not count.
  `require_signature` is the real boundary, and it is off by default.
- **Hardware signing keys are untested.**
  The tests sign with a software key.
  A hardware key (`ed25519-sk`) has not been used yet.
- **The provenance snapshots see only the work tree, not background writes.**
  A write after a call ends, such as a background command, is not in the before or after snapshot of any call and does not count toward ownership.
  Submodule content is not seen either.
- **A timeout stops rvow and not its child.**
  The pi tool `rvow_prove` ends the rvow process after its timeout, and the verify command that rvow started can keep running.
- **`config.toml` has no edit deny.**
  A change to it is caught as weaken drift, in repository mode and in the global file.
- **A subagent can edit a protected test block.**
  The hook sees files and not the test blocks inside a Rust file, so the edit is found afterwards as weaken drift.
- **A lease needs the rvow hooks in the subagent.**
  A Claude subagent is bound by the mod, and a pi subagent binds itself when its session loads the rvow extension, which is the default of `pi-subagents`.
  A subagent that opts out (`extensions: false` or `isolated: true` in pi, or no mod in Claude Code) meets no rvow hook at all, so the lease does not apply and only the gate checks its work.
  The hook judges the edit tools, `rvow prove` and the files a shell command names as targets.
  A file that a program writes without naming it, such as a build output, is not seen.
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
  A shell word that holds `*`, `?` or `[` is expanded from the call's working folder, and a glob that expands to a protected file is denied under that file's rule, while a glob that matches nothing is allowed.
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
