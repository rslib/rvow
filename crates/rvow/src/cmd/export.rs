use std::path::PathBuf;

use rvow_core::error::Result;
use rvow_core::export;
use rvow_core::store;

use super::project;
use crate::output::Reply;

pub fn export_openspec(to: Option<PathBuf>, dry_run: bool) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let to = to.unwrap_or_else(|| PathBuf::from("openspec-export"));
    let report = export::openspec(&p, &to, dry_run)?;
    let lost = report.lost();
    let verb = if dry_run { "would write" } else { "wrote" };
    let mut r = Reply::new();
    r.line(format!("openspec export{}", if dry_run { " (dry run)" } else { "" }));
    r.line(format!("{verb} {} files under {}", report.files.len(), report.target.join("openspec").display()));
    r.line(format!("specs: {}", report.specs));
    r.line(format!("requirements: {}", report.requirements));
    r.line(format!("open changes: {}", report.open));
    r.line(format!("archived changes: {}", report.archived));
    r.line(format!("not exported: {} dropped, {} superseded", report.dropped, report.superseded));
    r.line("lost or approximated:");
    for l in &lost {
        r.line(format!("  {l}"));
    }
    r.set("dry_run", report.dry_run);
    r.set("target", report.target.join("openspec").display().to_string());
    r.set("files", report.files.len());
    r.set("specs", report.specs);
    r.set("requirements", report.requirements);
    r.set("open_changes", report.open);
    r.set("archived_changes", report.archived);
    r.set("dropped", report.dropped);
    r.set("superseded", report.superseded);
    r.set("lost", lost);
    if dry_run {
        r.set("paths", report.files);
        r.help(format!("Run `rvow export openspec --to {}` to write the tree", report.target.display()));
    } else {
        r.help(format!(
            "Run `rvow import openspec --from {} --dry-run` in a fresh project to check the round trip",
            report.target.join("openspec").display()
        ));
    }
    Ok(r)
}
