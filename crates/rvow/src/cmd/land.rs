use rvow_core::error::Result;
use rvow_core::land::{self, Options};
use rvow_core::store;

use super::{cwd, project, which};
use crate::output::Reply;

/// Lands the proved task branches of a change on its branch, gated.
pub fn land(target: &str, merge: bool) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let rgit = which("rgit").map(|path| path.display().to_string());
    let report = land::land(&p, &home, &cwd()?, target, Options { merge }, rgit, &mut std::io::stderr())?;
    let mut r = Reply::new();
    if report.nothing {
        r.line(format!("nothing to land on {}", report.branch));
        r.set("branch", report.branch.as_str());
        r.set("tip", report.tip.as_str());
        r.set("landed", "none");
        r.set("nothing", true);
        r.help("run rvow delegate <task> --worktree for a task that is not proved yet");
        return Ok(r);
    }
    for l in &report.landed {
        r.line(format!("landed {} {}", l.task, l.branch));
    }
    for c in &report.conflicts {
        let files = c.files.join(", ");
        r.line(format!("conflict {} {}: {files}", c.task, c.branch));
        for step in &c.steps {
            r.line(format!("  {step}"));
        }
    }
    for f in &report.failed {
        r.line(format!("failed: {f}"));
    }
    if report.tip == report.before {
        r.line(format!("{} stays at {}", report.branch, report.before));
    } else {
        r.line(format!("{} moved to {}", report.branch, report.tip));
    }
    let landed: Vec<_> = report.landed.iter().map(|l| obj! { "task" => l.task.clone(), "branch" => l.branch.clone() }).collect();
    r.table("landed", landed, "none");
    let conflicts: Vec<_> = report
        .conflicts
        .iter()
        .map(|c| {
            obj! { "task" => c.task.clone(), "branch" => c.branch.clone(), "files" => c.files.clone(), "steps" => c.steps.clone() }
        })
        .collect();
    r.table("conflicts", conflicts, "none");
    r.table("failed", report.failed.iter().map(|f| obj! { "check" => f.clone() }).collect(), "none");
    r.set("branch", report.branch.as_str());
    r.set("tip", report.tip.as_str());
    r.ok = report.ok();
    if !report.conflicts.is_empty() {
        r.help("rebase each conflicting task branch on the change branch in its worktree, prove it, then run rvow land again");
    }
    Ok(r)
}
