//! Where a project's rvow tree lives and how a repository finds it.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::git;

pub const PROJECT_FILE: &str = "project.toml";
pub const CONFIG_FILE: &str = "config.toml";
pub const IDS_FILE: &str = "ids.jsonl";
pub const LOCK_FILE: &str = ".lock";
/// The local index in the rvow home from a repository to its project.
pub const INDEX_FILE: &str = "project-index.json";
/// The folder in the rvow home that holds one warm worktree per project.
pub const WORKTREES_DIR: &str = "worktrees";
pub const REPO_DIR: &str = "rvow";
const DIRS: [&str; 7] = ["specs", "backlog", "active", "archive/done", "archive/dropped", "archive/superseded", "objects"];
const MAX_DEPTH: usize = 6;

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

/// The file that remembers the rvow home for processes that do not inherit the
/// user's shell environment, such as an editor plugin started from a GUI.
fn pointer_path(xdg_config: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    match (xdg_config, home) {
        (Some(x), _) => Some(PathBuf::from(x).join("rvow/home")),
        (None, Some(h)) => Some(PathBuf::from(h).join(".config/rvow/home")),
        (None, None) => None,
    }
}

/// Picks the rvow home: `RVOW_HOME`, else the path `rvow init` or
/// `rvow install` recorded, else `$XDG_DATA_HOME/rvow`, else
/// `~/.local/share/rvow`.
fn resolve_home(rvow_home: Option<&str>, recorded: Option<&str>, xdg_data: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    rvow_home
        .or(recorded)
        .map(PathBuf::from)
        .or_else(|| xdg_data.map(|x| PathBuf::from(x).join("rvow")))
        .or_else(|| home.map(|h| PathBuf::from(h).join(".local/share/rvow")))
}

fn env(key: &str) -> Option<String> {
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

/// Refuses a repository whose identity git cannot report, and returns its roots.
fn identity(top: &Path) -> Result<Vec<String>> {
    if git::is_shallow(top)? {
        return Err(Error::new(
            "shallow",
            "a shallow clone hides the root commit that identifies the project",
            "run git fetch --unshallow",
        ));
    }
    let roots = git::root_commits(top)?;
    if roots.is_empty() {
        return Err(Error::new("no-commit", "the repository has no commit to identify it by", "make a first commit"));
    }
    Ok(roots)
}

fn read_meta(root: &Path) -> Result<Meta> {
    let path = root.join(PROJECT_FILE);
    let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
    toml::from_str(&text).map_err(|e| {
        Error::new("project-file", format!("{}: {e}", path.display()), "run rvow rebind to write it again from this repository")
    })
}

/// Creates the rvow tree for the repository that contains `dir`.
pub fn init(dir: &Path, home: &Path, mode: Mode) -> Result<Project> {
    let top = git::toplevel(dir)?;
    let roots = identity(&top)?;
    let remote = git::origin_url(&top).and_then(|u| git::remote_path(&u));
    let root = match mode {
        Mode::Repo => top.join(REPO_DIR),
        Mode::Home => {
            let name = top.file_name().and_then(|n| n.to_str()).unwrap_or("repo").to_lowercase();
            home.join(remote.clone().unwrap_or_else(|| format!("local/{name}")))
        }
    };
    if let Some(found) = find(&top, home)? {
        return Err(Error::new(
            "exists",
            format!("this repository already has an rvow tree at {}", found.root.display()),
            "use the existing tree",
        ));
    }
    if root.join(PROJECT_FILE).exists() {
        return Err(Error::new(
            "name-taken",
            format!("{} belongs to another repository with the same name", root.display()),
            "rename one of the folders, or give this one a remote",
        ));
    }
    for d in DIRS {
        let p = root.join(d);
        std::fs::create_dir_all(&p).map_err(|e| Error::io(&p, e))?;
    }
    let meta = Meta { roots, remote, mode };
    let text = toml::to_string(&meta).map_err(|e| Error::internal(format!("project metadata does not serialize: {e}")))?;
    for (name, body) in [(PROJECT_FILE, text.as_str()), (CONFIG_FILE, ""), (IDS_FILE, "")] {
        let p = root.join(name);
        std::fs::write(&p, body).map_err(|e| Error::io(&p, e))?;
    }
    match mode {
        Mode::Repo => {
            add_union_merge(&top)?;
            let lines: Vec<String> = local_files().iter().map(|f| format!("{REPO_DIR}/{f}")).collect();
            add_lines(&top.join(".gitignore"), &lines.iter().map(String::as_str).collect::<Vec<_>>())?;
        }
        Mode::Home => {
            let key = format!("{}/{}", crate::seal::KEY_FOLDER, crate::seal::KEY_FILE);
            let mut lines = local_files().to_vec();
            lines.extend([format!("{}/", crate::install::MARKETPLACE_DIR), key, INDEX_FILE.to_string(), format!("{WORKTREES_DIR}/")]);
            add_lines(&home.join(".gitignore"), &lines.iter().map(String::as_str).collect::<Vec<_>>())?;
        }
    }
    Ok(Project { root, meta })
}

/// What `rebind` did to the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rebind {
    Unchanged(PathBuf),
    Updated(PathBuf),
    Moved { from: PathBuf, to: PathBuf },
}

fn write_meta(root: &Path, meta: &Meta) -> Result<bool> {
    let text = toml::to_string(meta).map_err(|e| Error::internal(format!("project metadata does not serialize: {e}")))?;
    let path = root.join(PROJECT_FILE);
    if std::fs::read_to_string(&path).is_ok_and(|old| old == text) {
        return Ok(false);
    }
    std::fs::write(&path, text).map_err(|e| Error::io(&path, e))?;
    Ok(true)
}

/// Makes the tree of the repository that contains `dir` match the repository again: it records the
/// current root commits and origin remote in `project.toml`, and moves a home tree to the path its
/// remote implies. Running it twice changes nothing.
pub fn rebind(dir: &Path, home: &Path) -> Result<Rebind> {
    rebind_impl(dir, home, None)
}

/// `rebind` with a filter-repo commit map, so a rewritten history whose root
/// commits no longer match is found again through the map.
pub fn rebind_with_map(dir: &Path, home: &Path, map_path: &Path) -> Result<Rebind> {
    let map = read_commit_map(map_path)?;
    rebind_impl(dir, home, Some(&map))
}

/// Reads a `git filter-repo` commit map: `<old> <new>` per line.
fn read_commit_map(path: &Path) -> Result<BTreeMap<String, String>> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        if let (Some(old), Some(new)) = (parts.next(), parts.next()) {
            map.insert(old.to_string(), new.to_string());
        }
    }
    Ok(map)
}

/// Maps each root through the commit map, keeping an unmapped root as it is.
fn map_roots(roots: &[String], map: &BTreeMap<String, String>) -> Vec<String> {
    roots.iter().map(|r| map.get(r).cloned().unwrap_or_else(|| r.clone())).collect()
}

/// A project whose recorded roots map through `map` to `roots`, found the
/// same way `find` matches: one candidate, or one by remote, else ambiguous.
fn find_mapped(dir: &Path, home: &Path, roots: &[String], map: &BTreeMap<String, String>) -> Result<Option<Project>> {
    let remote = git::origin_url(dir).and_then(|u| git::remote_path(&u));
    let mut hits = Vec::new();
    walk(home, 0, &mut |root| {
        if let Ok(meta) = read_meta(root) {
            let mapped = map_roots(&meta.roots, map);
            if mapped.iter().any(|r| roots.contains(r)) {
                hits.push(Project { root: root.to_path_buf(), meta });
            }
        }
    });
    if hits.len() > 1 {
        hits.retain(|p| p.meta.remote.is_some() && p.meta.remote == remote);
    }
    match hits.len() {
        0 => Ok(None),
        1 => Ok(hits.pop()),
        _ => Err(Error::new(
            "ambiguous-project",
            format!(
                "several rvow projects match this repository through the map: {}",
                hits.iter().map(|p| p.root.display().to_string()).collect::<Vec<_>>().join(", ")
            ),
            "give each clone a distinct origin remote",
        )),
    }
}

fn rebind_impl(dir: &Path, home: &Path, map: Option<&BTreeMap<String, String>>) -> Result<Rebind> {
    let top = git::toplevel(dir)?;
    let roots = identity(&top)?;
    let remote = git::origin_url(&top).and_then(|u| git::remote_path(&u));
    let repo_root = top.join(REPO_DIR);
    if repo_root.join(CONFIG_FILE).exists() {
        let changed = write_meta(&repo_root, &Meta { roots, remote, mode: Mode::Repo })?;
        return Ok(if changed { Rebind::Updated(repo_root) } else { Rebind::Unchanged(repo_root) });
    }
    let found = match find(&top, home)? {
        Some(f) => Some(f),
        None => match map {
            Some(m) => find_mapped(&top, home, &roots, m)?,
            None => None,
        },
    };
    let Some(found) = found else {
        return Err(Error::new("no-project", "no rvow tree matches this repository", "run rvow init or give it a commit map"));
    };
    let _lock = found.lock()?;
    let meta = Meta { roots: roots.clone(), remote: remote.clone().or(found.meta.remote.clone()), ..found.meta.clone() };
    let want = remote.map(|r| home.join(r)).unwrap_or_else(|| found.root.clone());
    if want == found.root {
        let changed = write_meta(&found.root, &meta)?;
        return Ok(if changed { Rebind::Updated(found.root) } else { Rebind::Unchanged(found.root) });
    }
    if want.join(PROJECT_FILE).exists() {
        return Err(Error::new(
            "name-taken",
            format!("{} already holds another rvow tree", want.display()),
            format!("move or remove {}, then run rvow rebind again", want.display()),
        ));
    }
    if let Some(parent) = want.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    std::fs::rename(&found.root, &want).map_err(|e| Error::io(&found.root, e))?;
    write_meta(&want, &meta)?;
    Ok(Rebind::Moved { from: found.root, to: want })
}

fn add_union_merge(top: &Path) -> Result<()> {
    add_lines(&top.join(".gitattributes"), &[&format!("{REPO_DIR}/**/*.jsonl merge=union")])
}

/// The project files that belong to one machine, so git never tracks them.
fn local_files() -> [String; 7] {
    [
        LOCK_FILE.to_string(),
        "hook-state/".to_string(),
        "provenance/".to_string(),
        crate::stats::STATS_FILE.to_string(),
        crate::lease::LEASE_FILE.to_string(),
        crate::yolo::YOLO_FILE.to_string(),
        crate::gate::cache::CACHE_FILE.to_string(),
    ]
}

/// Appends each of `lines` to the file at `path` unless it is already there.
fn add_lines(path: &Path, lines: &[&str]) -> Result<()> {
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

/// Finds the rvow tree of the repository that contains `dir`: repo mode when
/// `rvow/config.toml` exists, else the home project whose roots match.
pub fn find(dir: &Path, home: &Path) -> Result<Option<Project>> {
    // One git call for both, since every hook runs this lookup.
    let out = git::run(dir, &["rev-parse", "--show-toplevel", "--path-format=absolute", "--git-common-dir"])?;
    let mut lines = out.lines();
    let top = PathBuf::from(lines.next().unwrap_or_default());
    let common = lines.next().filter(|c| !c.is_empty()).map(String::from);
    let repo_root = top.join(REPO_DIR);
    if repo_root.join(CONFIG_FILE).exists() {
        let meta = read_meta(&repo_root)?;
        return Ok(Some(Project { root: repo_root, meta }));
    }
    if let Some(project) = common.as_deref().and_then(|c| index_hit(&top, home, c)) {
        return Ok(Some(project));
    }
    let roots = identity(&top)?;
    let remote = git::origin_url(&top).and_then(|u| git::remote_path(&u));
    let mut hits = Vec::new();
    walk(home, 0, &mut |root| {
        if let Ok(meta) = read_meta(root)
            && meta.roots.iter().any(|r| roots.contains(r))
        {
            hits.push(Project { root: root.to_path_buf(), meta });
        }
    });
    if hits.len() > 1 {
        hits.retain(|p| p.meta.remote.is_some() && p.meta.remote == remote);
    }
    match hits.len() {
        0 => Ok(None),
        1 => {
            let project = hits.pop();
            if let (Some(c), Some(p)) = (common.as_deref(), project.as_ref()) {
                index_store(home, c, p, &roots);
            }
            Ok(project)
        }
        _ => Err(Error::new(
            "ambiguous-project",
            format!(
                "several rvow projects match this repository: {}",
                hits.iter().map(|p| p.root.display().to_string()).collect::<Vec<_>>().join(", ")
            ),
            "give each clone a distinct origin remote",
        )),
    }
}

/// The warm worktree of `project`: one folder per project under the rvow
/// home, kept between runs so build output stays. It is local to one machine,
/// so an older home gets its ignore line here.
pub fn warm_tree(home: &Path, project: &Project) -> PathBuf {
    let ignore = home.join(".gitignore");
    if ignore.is_file() {
        let _ = add_lines(&ignore, &[&format!("{WORKTREES_DIR}/")]);
    }
    let key = crate::vow::sha256_hex(project.root.to_string_lossy().as_bytes());
    home.join(WORKTREES_DIR).join(&key[..16])
}

/// What the index remembers about one repository: its project folder and the
/// root commits that project named when the entry was written.
#[derive(Debug, Serialize, Deserialize)]
struct IndexEntry {
    project: PathBuf,
    roots: Vec<String>,
}

/// The index, keyed by git common dir so every worktree shares an entry. A
/// missing, unreadable or corrupt file is an empty index.
fn read_index(home: &Path) -> BTreeMap<String, IndexEntry> {
    std::fs::read_to_string(home.join(INDEX_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// The project the index maps `common` to, when its folder is inside `home`
/// and still exists, its recorded roots still share a root commit with the
/// entry, and the repository at `top` still holds that commit, so a repository
/// cloned again at the same path never inherits the old project.
fn index_hit(top: &Path, home: &Path, common: &str) -> Option<Project> {
    let entry = read_index(home).remove(common)?;
    if !entry.project.starts_with(home) {
        return None;
    }
    let meta = read_meta(&entry.project).ok()?;
    let shared = meta.roots.iter().find(|r| entry.roots.contains(r))?;
    git::run(top, &["cat-file", "-e", &format!("{shared}^{{commit}}")]).ok()?;
    Some(Project { root: entry.project, meta })
}

/// Records the project of `common`. The index is a cache, so a failed write is
/// ignored and the next lookup walks again. The file is renamed into place, so
/// a concurrent reader never sees half of it.
fn index_store(home: &Path, common: &str, project: &Project, roots: &[String]) {
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

fn walk(dir: &Path, depth: usize, f: &mut impl FnMut(&Path)) {
    if dir.join(PROJECT_FILE).is_file() {
        f(dir);
        return;
    }
    if depth >= MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            walk(&e.path(), depth + 1, f);
        }
    }
}

/// An exclusive lock on the project. Every write holds one until it is dropped.
pub struct Lock {
    _file: File,
}

impl Project {
    pub fn lock(&self) -> Result<Lock> {
        let path = self.root.join(LOCK_FILE);
        let file = File::options().create(true).truncate(false).write(true).open(&path).map_err(|e| Error::io(&path, e))?;
        file.lock().map_err(|e| Error::io(&path, e))?;
        Ok(Lock { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;

    fn ignored(repo: &Path, rel: &str) -> bool {
        std::process::Command::new(git::bin())
            .args(["-C", repo.to_str().unwrap(), "check-ignore", "-q", "--no-index", rel])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap()
            .success()
    }

    #[test]
    fn init_ignores_local_state() {
        let tmp = TempDir::new();
        let local = [
            "hook-state/files-s1",
            "provenance/k/records.jsonl",
            "hook-stats.jsonl",
            "leases.jsonl",
            "yolo.json",
            "rerun-cache.jsonl",
            ".lock",
        ];
        let shared = ["ids.jsonl", "config.toml", "project.toml", "spec-record.json", "backlog/x/proposal.md"];

        let home = tmp.0.join("home");
        git(&tmp.0, &["init", "-q", home.to_str().unwrap()]);
        let p = init(&repo_with_commit(&tmp.0, "r"), &home, Mode::Home).unwrap();
        let rel = p.root.strip_prefix(&home).unwrap().to_str().unwrap().to_string();
        for f in local {
            assert!(ignored(&home, &format!("{rel}/{f}")), "home mode must ignore {f}");
        }
        for f in shared {
            assert!(!ignored(&home, &format!("{rel}/{f}")), "home mode must track {f}");
        }
        assert!(ignored(&home, INDEX_FILE), "the project index is local to one machine");
        let key = crate::seal::key_path(&home);
        assert!(ignored(&home, key.strip_prefix(&home).unwrap().to_str().unwrap()), "the seal key is a secret");

        let repo = repo_with_commit(&tmp.0, "o");
        init(&repo, &tmp.0.join("home2"), Mode::Repo).unwrap();
        for f in local {
            assert!(ignored(&repo, &format!("{REPO_DIR}/{f}")), "repo mode must ignore {f}");
        }
        for f in shared {
            assert!(!ignored(&repo, &format!("{REPO_DIR}/{f}")), "repo mode must track {f}");
        }
    }

    #[test]
    fn init_writes_the_ignore_lines() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let home = tmp.0.join("home");
        init(&repo, &home, Mode::Home).unwrap();
        let text = std::fs::read_to_string(home.join(".gitignore")).unwrap();
        let key = format!("{}/{}", crate::seal::KEY_FOLDER, crate::seal::KEY_FILE);
        assert_eq!(
            text,
            format!(
                ".lock\nhook-state/\nprovenance/\nhook-stats.jsonl\nleases.jsonl\nyolo.json\nrerun-cache.jsonl\nclaude-plugin/\n{key}\nproject-index.json\nworktrees/\n"
            )
        );

        let other = repo_with_commit(&tmp.0, "o");
        std::fs::write(other.join(".gitignore"), "target/").unwrap();
        init(&other, &tmp.0.join("home2"), Mode::Repo).unwrap();
        let text = std::fs::read_to_string(other.join(".gitignore")).unwrap();
        assert_eq!(
            text,
            "target/\nrvow/.lock\nrvow/hook-state/\nrvow/provenance/\nrvow/hook-stats.jsonl\nrvow/leases.jsonl\nrvow/yolo.json\nrvow/rerun-cache.jsonl\n"
        );
        add_lines(&other.join(".gitignore"), &["rvow/.lock", "x"]).unwrap();
        let text = std::fs::read_to_string(other.join(".gitignore")).unwrap();
        assert!(text.ends_with("rvow/rerun-cache.jsonl\nx\n"), "{text}");
        assert_eq!(text.matches("rvow/.lock").count(), 1, "a line already there is not added again");
    }

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
    fn init_and_find_in_home_mode() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "MyRepo");
        assert_eq!(find(&repo, &home).unwrap(), None);

        let p = init(&repo, &home, Mode::Home).unwrap();
        assert_eq!(p.root, home.join("local/myrepo"));
        for d in DIRS {
            assert!(p.root.join(d).is_dir(), "{d}");
        }
        assert_eq!(find(&repo, &home).unwrap(), Some(p.clone()));
        assert_eq!(init(&repo, &home, Mode::Home).unwrap_err().code, "exists");

        git(&repo, &["worktree", "add", "-q", "-b", "wt", "../wt"]);
        assert_eq!(find(&tmp.0.join("wt"), &home).unwrap(), Some(p));
    }

    #[test]
    fn remote_decides_the_path_and_splits_forks() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let up = repo_with_commit(&tmp.0, "up");
        git(&up, &["remote", "add", "origin", "git@github.com:a/rvow.git"]);
        let p = init(&up, &home, Mode::Home).unwrap();
        assert_eq!(p.root, home.join("github.com/a/rvow"));

        let fork = tmp.0.join("fork");
        git(&tmp.0, &["clone", "-q", up.to_str().unwrap(), fork.to_str().unwrap()]);
        git(&fork, &["remote", "set-url", "origin", "git@github.com:b/rvow.git"]);
        let f = init(&fork, &home, Mode::Home);
        assert_eq!(f.unwrap_err().code, "exists");
    }

    #[test]
    fn same_name_without_remote_is_refused() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let a = repo_with_commit(&tmp.0.join("a"), "utils");
        let b = tmp.0.join("b/utils");
        std::fs::create_dir_all(&b).unwrap();
        git(&b, &["init", "-q"]);
        std::fs::write(b.join("other.txt"), "other").unwrap();
        git(&b, &["add", "other.txt"]);
        git(&b, &["commit", "-q", "-m", "other"]);
        init(&a, &home, Mode::Home).unwrap();
        assert_eq!(init(&b, &home, Mode::Home).unwrap_err().code, "name-taken");
    }

    #[test]
    fn repo_mode_wins_and_adds_union_merge() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &home, Mode::Repo).unwrap();
        assert_eq!(p.root, repo.join("rvow"));
        assert_eq!(find(&repo, &home).unwrap().unwrap().root, p.root);
        let attrs = std::fs::read_to_string(repo.join(".gitattributes")).unwrap();
        assert_eq!(attrs, "rvow/**/*.jsonl merge=union\n");
        add_union_merge(&repo).unwrap();
        let again = std::fs::read_to_string(repo.join(".gitattributes")).unwrap();
        assert_eq!(again, attrs);
    }

    #[test]
    fn refuses_empty_and_shallow_repositories() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let empty = tmp.0.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        git(&empty, &["init", "-q"]);
        assert_eq!(init(&empty, &home, Mode::Home).unwrap_err().code, "no-commit");

        let src = repo_with_commit(&tmp.0, "src");
        std::fs::write(src.join("b.txt"), "b").unwrap();
        git(&src, &["add", "b.txt"]);
        git(&src, &["commit", "-q", "-m", "two"]);
        let url = format!("file://{}", src.display());
        let shallow = tmp.0.join("shallow");
        git(&tmp.0, &["clone", "-q", "--depth", "1", &url, shallow.to_str().unwrap()]);
        assert_eq!(init(&shallow, &home, Mode::Home).unwrap_err().code, "shallow");
    }

    #[test]
    fn rebind_moves_a_home_tree_to_its_remote_path() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &home, Mode::Home).unwrap();
        assert_eq!(rebind(&repo, &home).unwrap(), Rebind::Unchanged(p.root.clone()));

        git(&repo, &["remote", "add", "origin", "git@github.com:a/r.git"]);
        let to = home.join("github.com/a/r");
        assert_eq!(rebind(&repo, &home).unwrap(), Rebind::Moved { from: p.root.clone(), to: to.clone() });
        assert!(!p.root.exists());
        let found = find(&repo, &home).unwrap().unwrap();
        assert_eq!((found.root, found.meta.remote.as_deref()), (to.clone(), Some("github.com/a/r")));
        assert_eq!(rebind(&repo, &home).unwrap(), Rebind::Unchanged(to));
    }

    #[test]
    fn rebind_refuses_to_overwrite_another_tree() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let a = repo_with_commit(&tmp.0, "a");
        init(&a, &home, Mode::Home).unwrap();
        git(&a, &["remote", "add", "origin", "git@github.com:x/a.git"]);
        let other = home.join("github.com/x/a");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join(PROJECT_FILE), "").unwrap();
        assert_eq!(rebind(&a, &home).unwrap_err().code, "name-taken");
    }

    #[test]
    fn rebind_rewrites_a_broken_repo_project_file() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &tmp.0.join("home"), Mode::Repo).unwrap();
        std::fs::write(p.root.join(PROJECT_FILE), "not toml [").unwrap();
        assert_eq!(find(&repo, &tmp.0.join("home")).unwrap_err().code, "project-file");
        assert_eq!(rebind(&repo, &tmp.0.join("home")).unwrap(), Rebind::Updated(p.root.clone()));
        assert_eq!(find(&repo, &tmp.0.join("home")).unwrap(), Some(p));
    }

    #[test]
    fn rebind_without_a_tree_says_to_init() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        assert_eq!(rebind(&repo, &tmp.0.join("home")).unwrap_err().code, "no-project");
    }

    #[test]
    fn rebind_with_map_finds_a_rewritten_repo() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let old = repo_with_commit(&tmp.0, "old");
        let p = init(&old, &home, Mode::Home).unwrap();
        let a = git::run(&old, &["rev-parse", "HEAD"]).unwrap().trim().to_string();

        // A fresh repository with different content stands in for the same
        // project after a rewrite, so its root commits differ.
        let new = tmp.0.join("new");
        std::fs::create_dir_all(&new).unwrap();
        git(&new, &["init", "-q", "-b", "main"]);
        std::fs::write(new.join("a.txt"), "rewritten").unwrap();
        git(&new, &["add", "a.txt"]);
        git(&new, &["commit", "-q", "-m", "init"]);
        let b = git::run(&new, &["rev-parse", "HEAD"]).unwrap().trim().to_string();
        assert_ne!(a, b, "the two repositories have different roots after a rewrite");

        let map = tmp.0.join("commit-map");
        std::fs::write(&map, format!("{a} {b}\n")).unwrap();

        // Without the map the new repo matches no recorded root.
        assert_eq!(find(&new, &home).unwrap(), None);
        let changed = rebind_with_map(&new, &home, &map).unwrap();
        assert_eq!(changed, Rebind::Updated(p.root.clone()));

        // Now a plain find sees the project at its new root.
        let found = find(&new, &home).unwrap().unwrap();
        assert_eq!(found.root, p.root);
        assert_eq!(found.meta.roots, vec![b]);
    }
}
