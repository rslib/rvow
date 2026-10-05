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
- Confirm: a change that alters the plan, such as a new requirement, a changed title or a wider `files:` scope.
- Weaken: a change that could make a check easier to pass, such as a changed verify command, an edited test that a proof relied on or a changed config.

The gate fails on confirm and weaken drift until the user decides.
Free drift passes without a question, so small work does not wait for a person.
rvow cannot tell that an agent special-cased a tested input.
It can tell that a test file or a verify command changed, and that is where it asks.

## Amendments and the user-only confirm

The plan must be able to change, because plans are wrong.
The agent edits the plan files and runs `rvow amend` with a reason.
That records the edit as an amendment.
The user reads each amendment with its reason and runs `rvow confirm` to accept or reject it.
With `require_signature` set, the decision also needs an ssh signature bound to the content.

Confirm belongs to the user for one reason.
An agent that could approve its own amendment would make drift detection a formality.
The agent can do two things instead.
It can amend and wait, or it can run `rvow block` when a task cannot pass as written.

## The proof and what stale means

A proof ties a passing run to what the run saw: the task, the requirements it covers, the content of the files in scope and the recorded environment.
A proof is stale when one of those has changed since the run.
The reasons are task changed, requirement changed, inputs changed, private policy changed and environment changed.
A stale task needs a new proof, and `rvow prove --stale` runs them all.

Stale is not failed.
It means the old evidence no longer covers the current tree.
`rvow gate --strict` fails on stale and flaky proofs.

## The gate

`rvow gate` checks the active changes of the branch.
It fails on unproved tasks, failed proofs, pending amendments, blocking drift and files changed outside every task scope.

`rvow gate --rerun` goes further.
It runs every verify command again in a clean worktree of HEAD, so a proof that depended on an uncommitted file or a local state fails.
The rerun also covers tasks of archived changes on the branch, so a later change cannot break what an earlier change proved.
`rvow gate --out` writes the rerun results to a file, and `rvow ingest` adds them to the ledger of a home store.

A rerun skips a task when an earlier clean pass has the same key.
The key holds the content of the task's files at HEAD, the task and its requirements, the environment, the tools you list and the keys of the tasks it depends on.
A rebase or an amend does not change the key, but a changed byte in scope does.
The gate writes a pass only when the task ran tests, and signs each entry with a key that agents cannot read.
`rvow gate --rerun --no-cache` runs every task, and so does `--out`.
A scoped `setup` or `checks` entry runs only when a task that runs or a changed file matches it, so a Python change does not rebuild the C++ code.
When the cache answers every task, the gate makes no worktree and runs no `setup`.
[docs/settings.md](settings.md) lists the keys that tune the cache.

## Subagents and leases

A subagent meets the same rules as the main agent, but it does not know which task is its own.
`rvow delegate <task>` records a lease for one task and prints a short brief.
The first line of the brief is `lease: <task id>`, so put the brief at the start of the subagent's prompt.
The Claude mod reads that line when the subagent starts and binds the new agent id to the lease.
From then on the hook denies that subagent's edits of files outside the task's `files:`, and the reason names the task.
A lease ends when its task is proved or blocked, when the subagent stops or after 4 hours.
The lease file is closed to agents, like the gate cache.

Two limits stay.
A subagent without the mod is unbound, so it meets the usual rules.
The hook sees files and not what a shell command writes, so a lease scopes the edit tools only.

## Measuring the hooks

The hooks should help and stay quiet, so rvow counts what they do.
Each denial and each stop is one line in `hook-stats.jsonl` in the project folder.
A line holds the time, the hook, the outcome, the session id and, for a denial, one rule name such as `owner-command`, `outside-scope` or `protected-test`.
It never holds a command or a path.
`rvow stats` prints the counts of the last 7 days, or of `--days N`, with the stop blocks per 100 stops.
Use it to find the rule that fires most before you change any rule.
Agents cannot read or write the stats file.

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
