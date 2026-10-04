use rvow_core::brief;
use rvow_core::error::Result;
use rvow_core::lease;
use rvow_core::store;

use super::{cwd, project};
use crate::output::Reply;

pub fn delegate(task: &str) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let d = lease::issue(&p, &home, &cwd()?, task)?;
    let mut r = Reply::lines(brief::delegate_brief(&d));
    r.set("lease", d.lease.task.as_str());
    r.set("change", d.lease.change.as_str());
    r.help("Put this brief at the start of the subagent's prompt, with its first line unchanged");
    Ok(r)
}
