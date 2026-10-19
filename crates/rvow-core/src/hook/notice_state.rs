//! The tasks each session has already been told to prove, so the first edit is denied once per task.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::store::Project;

/// A notice record older than this is removed when another session records: 7 days.
const KEEP: Duration = Duration::from_secs(7 * 86_400);

fn file(project: &Project, session: &str) -> Option<PathBuf> {
    let name: String = session.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
    (!name.is_empty()).then(|| project.root.join("hook-state").join(format!("notice-{name}")))
}

/// True when `session` was already told about `task`.
pub(super) fn told(project: &Project, session: &str, task: &str) -> bool {
    file(project, session).is_some_and(|f| std::fs::read_to_string(f).is_ok_and(|s| s.lines().any(|l| l == task)))
}

/// Records that `session` was told about `task`. A session without a usable name records nothing.
pub(super) fn mark_told(project: &Project, session: &str, task: &str) {
    let Some(file) = file(project, session) else {
        return;
    };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
        prune(dir);
    }
    if let Ok(mut out) = std::fs::OpenOptions::new().create(true).append(true).open(&file) {
        let _ = writeln!(out, "{task}");
    }
}

/// Removes the `notice-*` records in `dir` that last changed more than `KEEP` ago.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("notice-") {
            continue;
        }
        let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > KEEP);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
