use rvow_core::error::Result;
use rvow_core::proof::{self, Pick};
use rvow_core::store;

use super::{cwd, project};
use crate::output::Reply;

/// What follows the decision, so the user and the agent know the next step.
fn next(pick: Pick, task: &str) -> String {
    match pick {
        Pick::Now => format!("run rvow prove {task}"),
        Pick::Archive => format!("rvow archive proves {task} again"),
        Pick::Keep => format!("the proof of {task} stands until its files change again"),
    }
}

/// Records the user's decision about the stale relaxed task `task`.
pub fn decide(task: &str, pick: Pick) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let task = proof::decide(&p, &home, &cwd()?, task, pick)?;
    let next = next(pick, &task);
    let mut r = Reply::new();
    r.line(format!("decided: {task} {}", pick.name()));
    r.line(format!("next: {next}"));
    r.set("decided", format!("{task} {}", pick.name()));
    r.set("task", task.clone());
    r.set("pick", pick.name());
    r.set("next", next);
    Ok(r)
}

/// The stale relaxed tasks that wait for the user's decision.
pub fn list() -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let waiting = proof::undecided(&p, &home, &cwd()?)?;
    let mut r = Reply::new();
    let mut rows = Vec::new();
    for (task, title) in &waiting {
        r.line(format!("  {task} {title}"));
        rows.push(obj! { "id" => task, "title" => title });
    }
    if waiting.is_empty() {
        r.line("0 tasks wait for a decision");
    }
    r.table("undecided", rows, "0 tasks wait for a decision");
    r.help("Ask the user with the rvow_ask tool; each answer runs rvow decide <task> now|archive|keep");
    Ok(r)
}
