//! The last brief that each session got at its start, so the same brief is sent once.

//! Each record lives in a `hook-state` folder: the project's, or the rvow home's outside a project.

use std::path::{Path, PathBuf};

fn file(dir: &Path, session: &str) -> Option<PathBuf> {
    let name: String = session.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
    (!name.is_empty()).then(|| dir.join(format!("brief-{name}")))
}

/// The last brief sent to `session`, or `None` when nothing was sent yet.
pub(super) fn load(dir: &Path, session: &str) -> Option<String> {
    std::fs::read_to_string(file(dir, session)?).ok()
}

/// Records `text` as the last brief of `session` and drops old records. A write error is ignored.
pub(super) fn store(dir: &Path, session: &str, text: &str) {
    let Some(file) = file(dir, session) else {
        return;
    };
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(&file, text);
    super::session_files::prune(dir);
}

/// Forgets the last brief of `session`, so the next start sends the full brief.
pub(super) fn clear(dir: &Path, session: &str) {
    if let Some(file) = file(dir, session) {
        let _ = std::fs::remove_file(file);
    }
}
