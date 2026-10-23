//! Where a project's rvow tree lives and how a repository finds it.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use rvow_base::error::{Error, Result};
use rvow_base::hash::sha256_hex;
use rvow_base::layout::{CACHE_FILE, LEASE_FILE, STATS_FILE, YOLO_FILE};
use rvow_base::paths::{env, pointer_path};

pub use rvow_base::layout::{CONFIG_FILE, GATE_REPOS_FILE, IDS_FILE, INDEX_FILE, LOCK_FILE, PROJECT_FILE, REPO_DIR, WORKTREES_DIR};
pub use rvow_base::paths::home_dir;

#[doc(hidden)]
pub const DIRS: [&str; 7] = ["specs", "backlog", "active", "archive/done", "archive/dropped", "archive/superseded", "objects"];
#[doc(hidden)]
pub const MAX_DEPTH: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Home,
    Repo,
}

/// The identity of a project, written once by `init`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    pub roots: Vec<String>,
    pub remote: Option<String>,
    pub mode: Mode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub root: PathBuf,
    pub meta: Meta,
}

/// Records `home` so processes without `RVOW_HOME` in their environment find
/// the same store. Returns the pointer file, or `None` without a home folder.
pub fn record_home(home: &Path) -> Result<Option<PathBuf>> {
    let Some(path) = pointer_path(env("XDG_CONFIG_HOME").as_deref(), env("HOME").as_deref()) else {
        return Ok(None);
    };
    let text = format!("{}\n", home.display());
    if std::fs::read_to_string(&path).is_ok_and(|t| t == text) {
        return Ok(Some(path));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    std::fs::write(&path, text).map_err(|e| Error::io(&path, e))?;
    Ok(Some(path))
}

#[doc(hidden)]
pub fn read_meta(root: &Path) -> Result<Meta> {
    let path = root.join(PROJECT_FILE);
    let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
    toml::from_str(&text).map_err(|e| {
        Error::new("project-file", format!("{}: {e}", path.display()), "run rvow rebind to write it again from this repository")
    })
}

#[doc(hidden)]
pub fn write_meta(root: &Path, meta: &Meta) -> Result<bool> {
    let text = toml::to_string(meta).map_err(|e| Error::internal(format!("project metadata does not serialize: {e}")))?;
    let path = root.join(PROJECT_FILE);
    if std::fs::read_to_string(&path).is_ok_and(|old| old == text) {
        return Ok(false);
    }
    std::fs::write(&path, text).map_err(|e| Error::io(&path, e))?;
    Ok(true)
}

#[doc(hidden)]
pub fn add_union_merge(top: &Path) -> Result<()> {
    add_lines(&top.join(".gitattributes"), &[&format!("{REPO_DIR}/**/*.jsonl merge=union")])
}

/// The project files that belong to one machine, so git never tracks them.
#[doc(hidden)]
pub fn local_files() -> [String; 7] {
    [
        LOCK_FILE.to_string(),
        "hook-state/".to_string(),
        "provenance/".to_string(),
        STATS_FILE.to_string(),
        LEASE_FILE.to_string(),
        YOLO_FILE.to_string(),
        CACHE_FILE.to_string(),
    ]
}

/// Appends each of `lines` to the file at `path` unless it is already there.
#[doc(hidden)]
pub fn add_lines(path: &Path, lines: &[&str]) -> Result<()> {
    let mut text = std::fs::read_to_string(path).unwrap_or_default();
    let before = text.len();
    for line in lines {
        if text.lines().any(|l| l.trim() == *line) {
            continue;
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(line);
        text.push('\n');
    }
    if text.len() == before {
        return Ok(());
    }
    std::fs::write(path, text).map_err(|e| Error::io(path, e))
}

/// The warm worktree of `project`: one folder per project under the rvow
/// home, kept between runs so build output stays. It is local to one machine,
/// so an older home gets its ignore line here.
pub fn warm_tree(home: &Path, project: &Project) -> PathBuf {
    let ignore = home.join(".gitignore");
    if ignore.is_file() {
        let _ = add_lines(&ignore, &[&format!("{WORKTREES_DIR}/")]);
    }
    let key = sha256_hex(project.root.to_string_lossy().as_bytes());
    home.join(WORKTREES_DIR).join(&key[..16])
}

/// What the index remembers about one repository: its project folder and the
/// root commits that project named when the entry was written.
#[doc(hidden)]
#[derive(Debug, Serialize, Deserialize)]
pub struct IndexEntry {
    pub project: PathBuf,
    pub roots: Vec<String>,
}

/// The index, keyed by git common dir so every worktree shares an entry. A
/// missing, unreadable or corrupt file is an empty index.
#[doc(hidden)]
pub fn read_index(home: &Path) -> BTreeMap<String, IndexEntry> {
    std::fs::read_to_string(home.join(INDEX_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Records the project of `common`. The index is a cache, so a failed write is
/// ignored and the next lookup walks again. The file is renamed into place, so
/// a concurrent reader never sees half of it.
#[doc(hidden)]
pub fn index_store(home: &Path, common: &str, project: &Project, roots: &[String]) {
    let mut index = read_index(home);
    index.insert(common.to_string(), IndexEntry { project: project.root.clone(), roots: roots.to_vec() });
    let Ok(text) = serde_json::to_string(&index) else {
        return;
    };
    let tmp = home.join(format!("{INDEX_FILE}.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, home.join(INDEX_FILE))).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    // A home made before the index existed lacks the ignore line `init` writes.
    let ignore = home.join(".gitignore");
    if ignore.is_file() {
        let _ = add_lines(&ignore, &[INDEX_FILE]);
    }
}

/// The git common dirs whose pre-push gate `rvow install` wrote. A missing or
/// corrupt file is an empty list.
pub fn gate_repos(home: &Path) -> Vec<PathBuf> {
    std::fs::read_to_string(home.join(GATE_REPOS_FILE)).ok().and_then(|t| serde_json::from_str::<Vec<PathBuf>>(&t).ok()).unwrap_or_default()
}

/// Remembers the gate at `common` once, so `rvow install --all` finds it again.
/// The git common dir is the one `rev-parse --path-format=absolute` prints. The
/// list is local to one machine, so a failed write is ignored, like the index's.
pub fn remember_gate(home: &Path, common: &Path) {
    let mut repos = gate_repos(home);
    if repos.iter().any(|path| path == common) {
        return;
    }
    repos.push(common.to_path_buf());
    repos.sort();
    let Ok(text) = serde_json::to_string(&repos) else {
        return;
    };
    let tmp = home.join(format!("{GATE_REPOS_FILE}.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, home.join(GATE_REPOS_FILE))).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    // A home made before this file existed lacks the ignore line `init` writes.
    let ignore = home.join(".gitignore");
    if ignore.is_file() {
        let _ = add_lines(&ignore, &[GATE_REPOS_FILE]);
    }
}

/// The git common dirs that the project index knows.
pub fn indexed_repos(home: &Path) -> Vec<PathBuf> {
    read_index(home).into_keys().map(PathBuf::from).collect()
}

/// An exclusive lock on the project. Every write holds one until it is dropped.
pub struct Lock {
    _file: File,
}

impl Project {
    pub fn lock(&self) -> Result<Lock> {
        lock_file(&self.root.join(LOCK_FILE))
    }

    /// The project lock, or `None` when another process still holds it after `wait`.
    pub fn lock_within(&self, wait: std::time::Duration) -> Result<Option<Lock>> {
        lock_file_within(&self.root.join(LOCK_FILE), wait)
    }
}

/// An exclusive lock on the file at `path`, created when missing.
#[doc(hidden)]
pub fn lock_file(path: &Path) -> Result<Lock> {
    let file = open_lock(path)?;
    file.lock().map_err(|e| Error::io(path, e))?;
    Ok(Lock { _file: file })
}

/// The lock on the file at `path`, or `None` when another process still holds it after `wait`.
#[doc(hidden)]
pub fn lock_file_within(path: &Path, wait: std::time::Duration) -> Result<Option<Lock>> {
    let file = open_lock(path)?;
    let end = std::time::Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(Lock { _file: file })),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < end => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(std::fs::TryLockError::Error(e)) => return Err(Error::io(path, e)),
        }
    }
}

fn open_lock(path: &Path) -> Result<File> {
    File::options().create(true).truncate(false).write(true).open(path).map_err(|e| Error::io(path, e))
}
