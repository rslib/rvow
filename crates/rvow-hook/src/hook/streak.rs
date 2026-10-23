//! How many tool calls in a row a session was denied, so the hook can stop denying and ask the user.

use crate::store::Project;

/// Denials in a row after which the next one becomes a question to the user.
pub(super) const BACKSTOP: u32 = 3;

fn file(project: &Project, session: &str) -> Option<std::path::PathBuf> {
    let name: String = session.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
    (!name.is_empty()).then(|| project.root.join("hook-state").join(format!("denied-{name}")))
}

/// Adds one denial to the session's streak and returns it.
pub(super) fn add(project: &Project, session: &str) -> u32 {
    let Some(file) = file(project, session) else {
        return 1;
    };
    let n = std::fs::read_to_string(&file).ok().and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0) + 1;
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&file, n.to_string());
    n
}

/// Ends the streak: a call went through, or the user was asked.
pub(super) fn reset(project: &Project, session: &str) {
    if let Some(file) = file(project, session) {
        let _ = std::fs::remove_file(file);
    }
}
