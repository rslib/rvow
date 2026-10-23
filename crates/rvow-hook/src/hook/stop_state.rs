//! What each session has already been told at a stop, so one reason is given once.

use std::path::Path;

use crate::store::Project;
use crate::vow::sha256_hex;

fn file(project: &Project, session: &str) -> Option<std::path::PathBuf> {
    let name: String = session.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
    (!name.is_empty()).then(|| project.root.join("hook-state").join(format!("stop-{name}")))
}

/// True when `session` already got `reason` as its latest stop reason; otherwise records it.
pub(super) fn already_given(project: &Project, session: &str, reason: &str) -> bool {
    let Some(file) = file(project, session) else {
        return false;
    };
    let digest = sha256_hex(reason.as_bytes());
    if std::fs::read_to_string(&file).is_ok_and(|s| s.trim() == digest) {
        return true;
    }
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&file, digest);
    false
}

/// Forgets the session's last reason, so the same reason is given again if it returns.
pub(super) fn clear(project: &Project, session: &str) {
    if let Some(file) = file(project, session) {
        let _ = std::fs::remove_file(Path::new(&file));
    }
}
