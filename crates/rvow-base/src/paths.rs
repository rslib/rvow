//! Where the rvow home is, and the repo-relative keys that a project query file
//! has in the protected set.

use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};
use crate::layout::{KEY_FILE, KEY_FOLDER};

/// The seal key file under the rvow home, created on first use.
pub fn key_path(home: &Path) -> PathBuf {
    home.join(KEY_FOLDER).join(KEY_FILE)
}

/// The file that remembers the rvow home for processes that do not inherit the
/// user's shell environment, such as an editor plugin started from a GUI.
pub fn pointer_path(xdg_config: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    match (xdg_config, home) {
        (Some(x), _) => Some(PathBuf::from(x).join("rvow/home")),
        (None, Some(h)) => Some(PathBuf::from(h).join(".config/rvow/home")),
        (None, None) => None,
    }
}

/// Picks the rvow home: `RVOW_HOME`, else the path `rvow init` or
/// `rvow install` recorded, else `$XDG_DATA_HOME/rvow`, else
/// `~/.local/share/rvow`.
pub fn resolve_home(rvow_home: Option<&str>, recorded: Option<&str>, xdg_data: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    rvow_home
        .or(recorded)
        .map(PathBuf::from)
        .or_else(|| xdg_data.map(|x| PathBuf::from(x).join("rvow")))
        .or_else(|| home.map(|h| PathBuf::from(h).join(".local/share/rvow")))
}

pub fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// The rvow home for this process. See `resolve_home` for the order.
pub fn home_dir() -> Result<PathBuf> {
    let recorded = pointer_path(env("XDG_CONFIG_HOME").as_deref(), env("HOME").as_deref())
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    resolve_home(env("RVOW_HOME").as_deref(), recorded.as_deref(), env("XDG_DATA_HOME").as_deref(), env("HOME").as_deref())
        .ok_or_else(|| Error::new("no-home", "HOME is not set", "set RVOW_HOME"))
}

/// The key a project query file has in the protected set: its repo-relative
/// path, or `None` for a path that leaves the repository.
pub fn query_key(rel: &str) -> Option<String> {
    let path = Path::new(rel);
    let mut parts: Vec<&str> = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => parts.push(part.to_str()?),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The absolute, symlink-free form of `path`, also when the file does not
/// exist yet: the nearest existing ancestor is resolved and the rest appended.
pub fn real_path(path: &Path, cwd: &Path) -> PathBuf {
    let abs = if path.is_absolute() { path.to_path_buf() } else { cwd.join(path) };
    let mut clean = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::ParentDir => {
                clean.pop();
            }
            Component::CurDir => {}
            other => clean.push(other),
        }
    }
    let mut existing = clean.clone();
    let mut rest = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(name) => rest.push(name.to_os_string()),
            None => break,
        }
        if !existing.pop() {
            break;
        }
    }
    let mut out = existing.canonicalize().unwrap_or(existing);
    for name in rest.into_iter().rev() {
        out.push(name);
    }
    out
}

/// The form a path is compared in: macOS file systems ignore case by default,
/// so `Tests/T.sh` names `tests/t.sh`.
pub fn key(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    if cfg!(target_os = "macos") { s.to_lowercase() } else { s }
}

/// Whether the key `target` is the key `dir` or lies under it.
pub fn at_or_under(target: &str, dir: &str) -> bool {
    target == dir || under(target, dir)
}

/// Whether the key `target` lies under the key `dir`.
pub fn under(target: &str, dir: &str) -> bool {
    rel_under(target, dir).is_some()
}

/// The rest of the key `target` below the key `dir`, without the separator.
pub fn rel_under<'t>(target: &'t str, dir: &str) -> Option<&'t str> {
    target.strip_prefix(dir)?.strip_prefix('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_order() {
        let r = |a, b, c, d| resolve_home(a, b, c, d).unwrap();
        assert_eq!(r(Some("/env"), Some("/rec"), Some("/x"), Some("/h")), PathBuf::from("/env"));
        assert_eq!(r(None, Some("/rec"), Some("/x"), Some("/h")), PathBuf::from("/rec"));
        assert_eq!(r(None, None, Some("/x"), Some("/h")), PathBuf::from("/x/rvow"));
        assert_eq!(r(None, None, None, Some("/h")), PathBuf::from("/h/.local/share/rvow"));
        assert_eq!(resolve_home(None, None, None, None), None);
        assert_eq!(pointer_path(Some("/cfg"), Some("/h")), Some(PathBuf::from("/cfg/rvow/home")));
        assert_eq!(pointer_path(None, Some("/h")), Some(PathBuf::from("/h/.config/rvow/home")));
    }

    #[test]
    fn real_paths_resolve_dots_links_and_missing_files() {
        let dir = std::env::temp_dir().join(format!("rvow-base-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        // The temp dir is a symlink on macOS, so the path is resolved as a file's is.
        let dir = dir.canonicalize().unwrap();
        assert_eq!(real_path(Path::new("a/./b/../b/new.txt"), &dir), dir.join("a/b/new.txt"));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("a/b"), dir.join("link")).unwrap();
            assert_eq!(real_path(Path::new("link/x.rs"), &dir), dir.join("a/b/x.rs"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn query_keys_are_repo_relative_or_absent() {
        assert_eq!(query_key("queries/rust.scm").as_deref(), Some("queries/rust.scm"));
        assert_eq!(query_key("./queries//rust.scm").as_deref(), Some("queries/rust.scm"));
        assert_eq!(query_key("../rust.scm"), None);
        assert_eq!(query_key("/etc/rust.scm"), None);
        assert_eq!(query_key(""), None);
    }
}
