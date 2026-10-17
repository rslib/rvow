//! The tasks each session has already been told to prove, so the notice is given once per task.

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
fn mark_told(project: &Project, session: &str, task: &str) {
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

fn stash_file(project: &Project, session: &str, call: &str) -> Option<PathBuf> {
    let clean = |s: &str| -> String { s.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect() };
    let (session, call) = (clean(session), clean(call));
    (!session.is_empty() && !call.is_empty()).then(|| project.root.join("hook-state").join(format!("notice-out-{session}-{call}")))
}

/// Keeps the notice about `task` for the post-tool-use of the call `call`.
pub(super) fn stash(project: &Project, session: &str, call: &str, task: &str, text: &str) {
    if let Some(file) = stash_file(project, session, call) {
        let _ = std::fs::write(file, format!("{task}\n{text}"));
    }
}

/// The notice that `stash` kept for the call, which is removed. The task counts as told only
/// here, once the call ran, so a call that is denied after its pre-tool-use loses no notice.
pub(super) fn take(project: &Project, session: &str, call: &str) -> Option<String> {
    let file = stash_file(project, session, call)?;
    let kept = std::fs::read_to_string(&file).ok()?;
    let _ = std::fs::remove_file(&file);
    let (task, text) = kept.split_once('\n')?;
    if text.is_empty() || told(project, session, task) {
        return None;
    }
    mark_told(project, session, task);
    Some(text.to_string())
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
