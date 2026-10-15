//! The files each session changed, so a stop is held only for the work of that session.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::PathBuf;

use crate::store::Project;

/// The entry for a formatter or fixer run, which may have changed any file.
pub(super) const ANY: &str = ":any";

/// The entry for a `rvow put`, which may have changed the plan of the change.
pub(super) const PLAN: &str = ":plan";

fn file(project: &Project, session: &str) -> Option<PathBuf> {
    let name: String = session.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
    (!name.is_empty()).then(|| project.root.join("hook-state").join(format!("files-{name}")))
}

/// Adds repo-relative `paths` to the record of `session`. A write error is ignored.
pub(super) fn record(project: &Project, session: &str, paths: &[String]) {
    let Some(file) = file(project, session) else {
        return;
    };
    let known = load(project, session);
    let new: Vec<&String> = paths.iter().filter(|p| !known.contains(*p)).collect();
    if new.is_empty() {
        return;
    }
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(mut out) = std::fs::OpenOptions::new().create(true).append(true).open(&file) else {
        return;
    };
    let text: String = new.iter().map(|p| format!("{p}\n")).collect();
    let _ = out.write_all(text.as_bytes());
}

/// The repo-relative paths `session` changed, or none for a session with no record.
pub(super) fn load(project: &Project, session: &str) -> BTreeSet<String> {
    let Some(file) = file(project, session) else {
        return BTreeSet::new();
    };
    std::fs::read_to_string(file).unwrap_or_default().lines().filter(|l| !l.is_empty()).map(String::from).collect()
}
