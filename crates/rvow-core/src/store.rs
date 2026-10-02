//! Where a project's rvow tree lives and how a repository finds it.

use std::fs::File;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::git;

pub const PROJECT_FILE: &str = "project.toml";
pub const CONFIG_FILE: &str = "config.toml";
pub const IDS_FILE: &str = "ids.jsonl";
pub const LOCK_FILE: &str = ".lock";
pub const REPO_DIR: &str = "rvow";
const DIRS: [&str; 7] = [
    "specs",
    "backlog",
    "active",
    "archive/done",
    "archive/dropped",
    "archive/superseded",
    "objects",
];
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
fn resolve_home(
    rvow_home: Option<&str>,
    recorded: Option<&str>,
    xdg_data: Option<&str>,
    home: Option<&str>,
) -> Option<PathBuf> {
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
    resolve_home(
        env("RVOW_HOME").as_deref(),
        recorded.as_deref(),
        env("XDG_DATA_HOME").as_deref(),
        env("HOME").as_deref(),
    )
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
        return Err(Error::new(
            "no-commit",
            "the repository has no commit to identify it by",
            "make a first commit",
        ));
    }
    Ok(roots)
}

fn read_meta(root: &Path) -> Result<Meta> {
    let path = root.join(PROJECT_FILE);
    let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
    toml::from_str(&text).map_err(|e| {
        Error::new(
            "project-file",
            format!("{}: {e}", path.display()),
            "restore project.toml from backup or run rvow rebind",
        )
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
            let name = top
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("repo")
                .to_lowercase();
            home.join(remote.clone().unwrap_or_else(|| format!("local/{name}")))
        }
    };
    if let Some(found) = find(&top, home)? {
        return Err(Error::new(
            "exists",
            format!(
                "this repository already has an rvow tree at {}",
                found.root.display()
            ),
            "use the existing tree",
        ));
    }
    if root.join(PROJECT_FILE).exists() {
        return Err(Error::new(
            "name-taken",
            format!(
                "{} belongs to another repository with the same name",
                root.display()
            ),
            "rename one of the folders, or give this one a remote",
        ));
    }
    for d in DIRS {
        let p = root.join(d);
        std::fs::create_dir_all(&p).map_err(|e| Error::io(&p, e))?;
    }
    let meta = Meta {
        roots,
        remote,
        mode,
    };
    let text = toml::to_string(&meta).expect("Meta serializes");
    for (name, body) in [
        (PROJECT_FILE, text.as_str()),
        (CONFIG_FILE, ""),
        (IDS_FILE, ""),
    ] {
        let p = root.join(name);
        std::fs::write(&p, body).map_err(|e| Error::io(&p, e))?;
    }
    if mode == Mode::Repo {
        add_union_merge(&top)?;
    }
    Ok(Project { root, meta })
}

fn add_union_merge(top: &Path) -> Result<()> {
    let line = format!("{REPO_DIR}/**/*.jsonl merge=union");
    let path = top.join(".gitattributes");
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    if old.lines().any(|l| l.trim() == line) {
        return Ok(());
    }
    let sep = if old.is_empty() || old.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    std::fs::write(&path, format!("{old}{sep}{line}\n")).map_err(|e| Error::io(&path, e))
}

/// Finds the rvow tree of the repository that contains `dir`: repo mode when
/// `rvow/config.toml` exists, else the home project whose roots match.
pub fn find(dir: &Path, home: &Path) -> Result<Option<Project>> {
    let top = git::toplevel(dir)?;
    let repo_root = top.join(REPO_DIR);
    if repo_root.join(CONFIG_FILE).exists() {
        let meta = read_meta(&repo_root)?;
        return Ok(Some(Project {
            root: repo_root,
            meta,
        }));
    }
    let roots = identity(&top)?;
    let remote = git::origin_url(&top).and_then(|u| git::remote_path(&u));
    let mut hits = Vec::new();
    walk(home, 0, &mut |root| {
        if let Ok(meta) = read_meta(root)
            && meta.roots.iter().any(|r| roots.contains(r))
        {
            hits.push(Project {
                root: root.to_path_buf(),
                meta,
            });
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
                "several rvow projects match this repository: {}",
                hits.iter()
                    .map(|p| p.root.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            "give each clone a distinct origin remote",
        )),
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
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| Error::io(&path, e))?;
        file.lock().map_err(|e| Error::io(&path, e))?;
        Ok(Lock { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;

    #[test]
    fn home_order() {
        let r = |a, b, c, d| resolve_home(a, b, c, d).unwrap();
        assert_eq!(
            r(Some("/env"), Some("/rec"), Some("/x"), Some("/h")),
            PathBuf::from("/env")
        );
        assert_eq!(
            r(None, Some("/rec"), Some("/x"), Some("/h")),
            PathBuf::from("/rec")
        );
        assert_eq!(
            r(None, None, Some("/x"), Some("/h")),
            PathBuf::from("/x/rvow")
        );
        assert_eq!(
            r(None, None, None, Some("/h")),
            PathBuf::from("/h/.local/share/rvow")
        );
        assert_eq!(resolve_home(None, None, None, None), None);
        assert_eq!(
            pointer_path(Some("/cfg"), Some("/h")),
            Some(PathBuf::from("/cfg/rvow/home"))
        );
        assert_eq!(
            pointer_path(None, Some("/h")),
            Some(PathBuf::from("/h/.config/rvow/home"))
        );
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
        git(
            &up,
            &["remote", "add", "origin", "git@github.com:a/rvow.git"],
        );
        let p = init(&up, &home, Mode::Home).unwrap();
        assert_eq!(p.root, home.join("github.com/a/rvow"));

        let fork = tmp.0.join("fork");
        git(
            &tmp.0,
            &["clone", "-q", up.to_str().unwrap(), fork.to_str().unwrap()],
        );
        git(
            &fork,
            &["remote", "set-url", "origin", "git@github.com:b/rvow.git"],
        );
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
        assert_eq!(
            init(&empty, &home, Mode::Home).unwrap_err().code,
            "no-commit"
        );

        let src = repo_with_commit(&tmp.0, "src");
        std::fs::write(src.join("b.txt"), "b").unwrap();
        git(&src, &["add", "b.txt"]);
        git(&src, &["commit", "-q", "-m", "two"]);
        let url = format!("file://{}", src.display());
        let shallow = tmp.0.join("shallow");
        git(
            &tmp.0,
            &[
                "clone",
                "-q",
                "--depth",
                "1",
                &url,
                shallow.to_str().unwrap(),
            ],
        );
        assert_eq!(
            init(&shallow, &home, Mode::Home).unwrap_err().code,
            "shallow"
        );
    }
}
