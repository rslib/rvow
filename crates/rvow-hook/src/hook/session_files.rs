//! The files each session changed, so a stop is held only for the work of that session.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::store::Project;

/// The entry for a formatter or fixer run, which may have changed any file.
pub(super) const ANY: &str = ":any";

/// The entry for a `rvow put`, which may have changed the plan of the change.
pub(super) const PLAN: &str = ":plan";

/// A session record older than this is removed when another session records: 7 days.
const KEEP: Duration = Duration::from_secs(7 * 86_400);

fn sanitize(name: &str) -> String {
    name.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect()
}

fn file(project: &Project, session: &str) -> Option<PathBuf> {
    let name = sanitize(session);
    (!name.is_empty()).then(|| project.root.join("hook-state").join(format!("files-{name}")))
}

fn agent_file(project: &Project, session: &str, agent: &str) -> Option<PathBuf> {
    let session = sanitize(session);
    let agent = sanitize(agent);
    (!session.is_empty() && !agent.is_empty()).then(|| project.root.join("hook-state").join(format!("files-{session}-agent-{agent}")))
}

/// Adds repo-relative `paths` to the record of `session`. A write error is ignored.
pub(super) fn record(project: &Project, session: &str, paths: &[String]) {
    let Some(file) = file(project, session) else {
        return;
    };
    record_to(&file, &load(project, session), paths);
}

/// Adds repo-relative `paths` to the record of the subagent `agent` of `session`.
/// A pi call that carries an agent id is the leased subagent session itself, so its
/// agent record stays under its own session and never touches the parent session.
pub(super) fn record_agent(project: &Project, session: &str, agent: &str, paths: &[String]) {
    let Some(file) = agent_file(project, session, agent) else {
        return;
    };
    let known = load_agent(project, session, agent);
    record_to(&file, &known, paths);
}

fn record_to(file: &PathBuf, known: &BTreeSet<String>, paths: &[String]) {
    let new: Vec<&String> = paths.iter().filter(|p| !known.contains(*p)).collect();
    if new.is_empty() {
        return;
    }
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(mut out) = std::fs::OpenOptions::new().create(true).append(true).open(file) else {
        return;
    };
    let text: String = new.iter().map(|p| format!("{p}\n")).collect();
    let _ = out.write_all(text.as_bytes());
    if let Some(dir) = file.parent() {
        prune(dir);
    }
}

/// Removes the per-session `files-*`, `stop-*` and `brief-*` records in `dir` that last changed more than `KEEP` ago.
pub(super) fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("files-") || name.starts_with("stop-") || name.starts_with("brief-")) {
            continue;
        }
        let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > KEEP);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The repo-relative paths `session` changed, or none for a session with no record.
pub(super) fn load(project: &Project, session: &str) -> BTreeSet<String> {
    let Some(file) = file(project, session) else {
        return BTreeSet::new();
    };
    read_paths(&file)
}

/// The repo-relative paths the subagent `agent` of `session` changed.
fn load_agent(project: &Project, session: &str, agent: &str) -> BTreeSet<String> {
    let Some(file) = agent_file(project, session, agent) else {
        return BTreeSet::new();
    };
    read_paths(&file)
}

fn read_paths(file: &PathBuf) -> BTreeSet<String> {
    std::fs::read_to_string(file).unwrap_or_default().lines().filter(|l| !l.is_empty()).map(String::from).collect()
}
