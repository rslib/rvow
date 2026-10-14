//! `rvow block` and `rvow note`: ledger events the agent writes by hand.

use std::path::Path;

use serde_json::Map;

use crate::change::{self, LEDGER};
use crate::error::{Error, Result};
use crate::git;
use crate::id::Id;
use crate::lease;
use crate::ledger::{self, Kind};
use crate::proof;
use crate::store::Project;
use crate::tasks::Task;
use crate::vow::task_hash;

pub const HANDOFF_LINES: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteKind {
    Decision,
    Surprise,
    Handoff,
}

impl NoteKind {
    pub fn name(self) -> &'static str {
        match self {
            NoteKind::Decision => "decision",
            NoteKind::Surprise => "surprise",
            NoteKind::Handoff => "handoff",
        }
    }
}

/// Records that the agent cannot finish `input` as written. The block holds
/// until a confirmed amendment changes the task.
pub fn block(project: &Project, input: &str, reason: &str) -> Result<(Id, String)> {
    if reason.trim().is_empty() {
        return Err(Error::new("reason", "a block needs a reason", "say what stops the task with --reason \"...\""));
    }
    let (living, _) = change::load_living(project)?;
    let changes = proof::active(project, &living)?;
    let (c, task): (_, &Task) = proof::find_task(&changes, input)?;
    let mut data = Map::new();
    data.insert("task".into(), task.id.to_string().into());
    data.insert("task_hash".into(), task_hash(task).into());
    data.insert("reason".into(), reason.trim().into());
    let lock = project.lock()?;
    ledger::append(&c.dir.path.join(LEDGER), &lock, Kind::Block, data)?;
    drop(lock);
    lease::end_task(project, &task.id.to_string())?;
    Ok((c.dir.id.clone(), task.id.to_string()))
}

/// Appends a note to the first active change on the current branch. A handoff
/// keeps only its first 5 lines, so it cannot crowd out the brief.
pub fn note(project: &Project, cwd: &Path, kind: NoteKind, text: &str) -> Result<(Id, bool)> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Error::new("text", "the note is empty", "write the note text"));
    }
    let (living, _) = change::load_living(project)?;
    let changes = proof::active(project, &living)?;
    let branch = git::branch(&git::toplevel(cwd)?)?.unwrap_or_default();
    let c = proof::on_branch(&changes, &branch)
        .into_iter()
        .next()
        .ok_or_else(|| Error::new("no-change", "this branch has no active change", "accept a change on this branch first"))?;
    let lines: Vec<&str> = text.lines().collect();
    let cut = kind == NoteKind::Handoff && lines.len() > HANDOFF_LINES;
    let kept = if cut { lines[..HANDOFF_LINES].join("\n") } else { text.to_string() };
    let mut data = Map::new();
    data.insert("note".into(), kind.name().into());
    data.insert("text".into(), kept.into());
    let lock = project.lock()?;
    ledger::append(&c.dir.path.join(LEDGER), &lock, Kind::Note, data)?;
    Ok((c.dir.id.clone(), cut))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amend::{self, Decision};
    use crate::config;
    use crate::fingerprint;
    use crate::proof::{Now, State, tests::accepted};

    fn task_state(p: &Project, home: &Path, repo: &Path) -> State {
        let (living, _) = change::load_living(p).unwrap();
        let changes = proof::active(p, &living).unwrap();
        let cfg = config::load(p, home).unwrap();
        let now = Now { top: repo, config: &cfg, env: fingerprint::env(repo, &cfg).unwrap(), living: &living };
        proof::state(&changes[0], &changes[0].loaded.tasks[0], &now).unwrap()
    }

    #[test]
    fn a_block_holds_until_the_task_changes() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        assert_eq!(block(&p, &task, "").unwrap_err().code, "reason");
        block(&p, &task[4..8], "the spec and the test disagree").unwrap();
        assert_eq!(task_state(&p, &home, &repo), State::Blocked);

        let dir = std::fs::read_dir(p.root.join("active")).unwrap().next().unwrap().unwrap().path();
        let tasks = dir.join("tasks.md");
        let text = std::fs::read_to_string(&tasks).unwrap();
        std::fs::write(&tasks, text.replace("Write ok", "Write ok, spec fixed")).unwrap();
        let ids: Vec<String> =
            amend::amend(&p, &home, &repo, None, "fix the task", None).unwrap().iter().map(|a| a.id.to_string()).collect();
        amend::confirm(&p, &ids, Decision::Accept, None).unwrap();
        assert_eq!(task_state(&p, &home, &repo), State::Missing);
    }

    #[test]
    fn notes_land_on_the_branch_change_and_handoffs_are_cut() {
        let (_tmp, p, repo, _) = accepted();
        assert!(!note(&p, &repo, NoteKind::Decision, "use JSON").unwrap().1);
        let long = "1\n2\n3\n4\n5\n6\n7";
        assert!(note(&p, &repo, NoteKind::Handoff, long).unwrap().1);
        let dir = std::fs::read_dir(p.root.join("active")).unwrap().next().unwrap().unwrap().path();
        let (events, _) = ledger::read(&dir.join(LEDGER)).unwrap();
        let last = events.last().unwrap();
        assert_eq!(last.data["note"], "handoff");
        assert_eq!(last.data["text"], "1\n2\n3\n4\n5");
        assert_eq!(note(&p, &repo, NoteKind::Surprise, " ").unwrap_err().code, "text");
    }
}
