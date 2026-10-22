use std::path::PathBuf;

use rvow_core::brief;
use rvow_core::change;
use rvow_core::error::{Error, Result};
use rvow_core::git;
use rvow_core::lease;
use rvow_core::proof;
use rvow_core::store;
use rvow_core::worktree;

use super::{cwd, project};
use crate::output::Reply;

/// Leases one task, in the worktree the flag names or one made for it.
pub fn delegate(task: &str, worktree: Option<Option<PathBuf>>) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let cwd = cwd()?;
    // The task is checked before a worktree is made, so a proved task leaves nothing behind.
    let (task_id, change_id) = lease::open_task(&p, &home, &cwd, task)?;
    let place = match worktree {
        None => None,
        Some(Some(path)) => Some(worktree::existing(&git::toplevel(&cwd)?, &path, &change_branch(&p, &change_id)?)?),
        Some(None) => {
            let top = git::toplevel(&cwd)?;
            let base = change_branch(&p, &change_id)?;
            Some(worktree::make(&top, &base, &change_id, &task_id)?)
        }
    };
    let d = lease::issue_at(&p, &home, &cwd, &task_id, place)?;
    let mut r = Reply::lines(brief::delegate_brief(&d));
    r.set("lease", d.lease.task.as_str());
    r.set("change", d.lease.change.as_str());
    if let Some(w) = &d.lease.worktree {
        r.set("worktree", w.display().to_string());
    }
    if let Some(b) = &d.lease.branch {
        r.set("branch", b.as_str());
    }
    r.help("Put this brief at the start of the subagent's prompt, with its first line unchanged");
    Ok(r)
}

/// The branch of the change that `id` names, from its proposal.
fn change_branch(project: &store::Project, id: &str) -> Result<String> {
    let (living, _) = change::load_living(project)?;
    let changes = proof::active(project, &living)?;
    changes
        .iter()
        .find(|c| c.dir.id.to_string() == id)
        .map(|c| c.loaded.proposal.branch.clone())
        .ok_or_else(|| Error::new("worktree", format!("change {id} is not active"), "run rvow status to see the active changes"))
}
