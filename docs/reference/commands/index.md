# Commands

Every rvow command. Each page comes from `rvow docs`, so it matches `rvow <command> --help`.

| Command | What it does |
| --- | --- |
| [`init`](init.md) | Create the rvow tree for this repository |
| [`rebind`](rebind.md) | Repair this repository's rvow tree: record its roots and remote, and move a home tree to its remote path |
| [`new`](new.md) | Create a change in backlog/ |
| [`validate`](validate.md) | Check the specs and every open change |
| [`accept`](accept.md) | Record the vow of a plan (user only) |
| [`review`](review.md) | Print a plan and the accept checklist; runs no verify command |
| [`ls`](ls.md) | List every change with its stage, branch and title |
| [`path`](path.md) | Print where a change, task or requirement lives |
| [`show`](show.md) | Print one requirement, task or change in full |
| [`put`](put.md) | Replace one part of a change, or one task or requirement block, with the text on stdin |
| [`spec`](spec.md) | Read the living specs |
| [`spec list`](spec-list.md) | List each capability with its requirement ids and titles |
| [`import`](import.md) | Convert an OpenSpec tree into specs and backlog changes |
| [`import openspec`](import-openspec.md) | Import the living specs and open changes of an OpenSpec folder |
| [`export`](export.md) | Write rvow's specs and changes as an OpenSpec tree, one way |
| [`export openspec`](export-openspec.md) | Write an OpenSpec tree under &lt;dir&gt;/openspec; never overwrites |
| [`guide`](guide.md) | Print the short guide for writing one plan artifact |
| [`ids`](ids.md) | Mint real ids for the placeholders of an active change |
| [`view`](view.md) | Print the shared dashboard with living specs and branch tasks |
| [`brief`](brief.md) | Print the short generated brief for this branch |
| [`next`](next.md) | Print the next task of this branch |
| [`install`](install.md) | Install the skill, the Claude Code hooks, the pi extension and the pre-push gate |
| [`check`](check.md) | Report on a plan before accept: findings, checks that cannot run, open questions |
| [`claim`](claim.md) | Move a backlog plan to the current branch, so it can be accepted here |
| [`clarify`](clarify.md) | List the open questions of a plan, each with its recommended answer |
| [`answer`](answer.md) | Record the user's answer to an open question and remove its marker |
| [`hook`](hook.md) | Answer a harness hook: reads the hook JSON on stdin, prints the response |
| [`stats`](stats.md) | Show how often the hooks denied or blocked in the last days |
| [`yolo`](yolo.md) | Relax the hooks for a while: audit, auto or bypass. Without a tier it shows what is on |
| [`delegate`](delegate.md) | Lease one task to a subagent and print the brief to give it |
| [`block`](block.md) | Record that a task cannot be finished as written |
| [`note`](note.md) | Append a decision, surprise or handoff note to this branch's change |
| [`amend`](amend.md) | Record the current plan edits as an amendment for the user to confirm |
| [`confirm`](confirm.md) | List pending amendments, or accept or reject them (user only) |
| [`prove`](prove.md) | Run a task's stored verify command and record the proof |
| [`status`](status.md) | Show the state of each task on this branch |
| [`archive`](archive.md) | Merge an active change into the living specs and move it to archive/done/ |
| [`drop`](drop.md) | Give up a backlog or active change; the specs stay as they are |
| [`supersede`](supersede.md) | Replace a backlog or active change with another open change |
| [`trace`](trace.md) | List every change that touched a requirement, oldest first |
| [`bundle`](bundle.md) | Export accepted verification data for a CI checkout without the home store |
| [`gate`](gate.md) | Check the active changes of this branch |
| [`ingest`](ingest.md) | Add proofs that a `gate --out` file carried to the ledger of the project each run names |
| [`attest`](attest.md) | Export the current proofs of a change as in-toto attestations |
| [`grammar`](grammar.md) | Fill the grammar cache, or show the pinned languages and their cache state |
| [`grammar fetch`](grammar-fetch.md) | Download pinned grammars into the cache; without names, every pinned language |
| [`grammar list`](grammar-list.md) | Show each pinned language and whether the cache holds its grammar |
