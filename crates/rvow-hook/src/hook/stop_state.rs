//! Which open items each session has already been told at a stop, so one item is
//! given once: the file holds one sha256 hex per line, so it carries no path of a
//! reason and stays small.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::store::Project;
use crate::vow::sha256_hex;

fn file(project: &Project, session: &str) -> Option<PathBuf> {
    let name: String = session.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
    (!name.is_empty()).then(|| project.root.join("hook-state").join(format!("stop-{name}")))
}

/// The sha256 hex of one item key, the form the session's record holds it in.
pub(super) fn digest(key: &str) -> String {
    sha256_hex(key.as_bytes())
}

/// The item keys this session was told at an earlier stop, in the form of `digest`;
/// empty for a session with no record.
pub(super) fn told(project: &Project, session: &str) -> BTreeSet<String> {
    let Some(file) = file(project, session) else {
        return BTreeSet::new();
    };
    std::fs::read_to_string(file).unwrap_or_default().lines().filter(|l| !l.is_empty()).map(String::from).collect()
}

/// Remembers the `digests` of the items that are open now: a key that is no longer
/// open drops out, so an item that is fixed and comes back is told again.
pub(super) fn keep(project: &Project, session: &str, digests: &[String]) {
    let Some(file) = file(project, session) else {
        return;
    };
    let mut sorted: Vec<&String> = digests.iter().collect();
    sorted.sort();
    sorted.dedup();
    let mut text = String::with_capacity(sorted.len() * 65);
    for digest in sorted {
        text.push_str(digest);
        text.push('\n');
    }
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&file, text);
}

/// Forgets the session's items, so each one is given again if it comes back.
pub(super) fn clear(project: &Project, session: &str) {
    if let Some(file) = file(project, session) {
        let _ = std::fs::remove_file(Path::new(&file));
    }
}
