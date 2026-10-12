//! Read-only git queries. rvow never writes to a repository through this module.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};

/// The git binary to run: `RVOW_GIT` when it names a file, else `git`.
/// A sandbox whose `git` on `PATH` cannot run sets `RVOW_GIT` to one that can.
/// `EVAL_RVOW_GIT` is the same setting through the eval harness, whose
/// sandbox passes only `EVAL_*` variables to the agent session.
pub fn bin() -> String {
    for key in ["RVOW_GIT", "EVAL_RVOW_GIT"] {
        if let Some(v) = std::env::var(key).ok().filter(|v| !v.is_empty()) {
            return v;
        }
    }
    "git".to_string()
}

pub fn run(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new(bin())
        .arg("-C")
        .arg(dir)
        .args(args)
        // A git hook sets these for its own repository, and they would
        // override `-C`.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .map_err(|e| Error::new("git", format!("cannot run git: {e}"), "install git"))?;
    if !out.status.success() {
        return Err(Error::new(
            "git",
            format!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()),
            "run the command in a git repository",
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// The base branch ref to measure from: the local or the remote base branch,
/// whichever has the newer merge base with HEAD. When the two merge bases are
/// equal or neither contains the other, the remote one wins, as it did when it
/// always came first. `None` when neither side exists.
pub fn base_ref(top: &Path, base: &str) -> Option<String> {
    let local = format!("refs/heads/{base}");
    let remote = format!("refs/remotes/origin/{base}");
    // A side with no merge base counts as missing, so unrelated histories
    // fall back to the other side instead of failing here.
    let mb = |r: &str| run(top, &["merge-base", "HEAD", r]).ok();
    match (mb(&local), mb(&remote)) {
        (None, None) => None,
        (Some(_), None) => Some(local),
        (None, Some(_)) => Some(remote),
        (Some(l), Some(r)) if l != r && run(top, &["merge-base", "--is-ancestor", &r, &l]).is_ok() => Some(local),
        _ => Some(remote),
    }
}

pub fn toplevel(dir: &Path) -> Result<PathBuf> {
    run(dir, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

pub fn is_shallow(dir: &Path) -> Result<bool> {
    Ok(run(dir, &["rev-parse", "--is-shallow-repository"])? == "true")
}

/// Every root commit reachable from any ref, sorted. Empty for a repository
/// with no commit.
pub fn root_commits(dir: &Path) -> Result<Vec<String>> {
    if run(dir, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_err() {
        return Ok(Vec::new());
    }
    let out = run(dir, &["rev-list", "--max-parents=0", "--all"])?;
    let mut roots: Vec<String> = out.lines().map(String::from).collect();
    roots.sort();
    roots.dedup();
    Ok(roots)
}

pub fn origin_url(dir: &Path) -> Option<String> {
    run(dir, &["remote", "get-url", "origin"]).ok().filter(|u| !u.is_empty())
}

/// The current branch, or `None` on a detached HEAD.
pub fn branch(dir: &Path) -> Result<Option<String>> {
    let name = run(dir, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    Ok((name != "HEAD").then_some(name))
}

/// The diff of one task's files against a base commit: the unified diff and
/// the changed paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDiff {
    pub diff: String,
    pub files: Vec<String>,
}

/// The diff of every file that matches `globs` against `base`: committed
/// changes since the base, uncommitted edits and untracked files. An
/// untracked file shows as an added file. Files outside the globs stay out.
/// Empty when nothing in scope changed.
/// Splits NUL-terminated git output (`-z`) into paths. `-z` leaves paths
/// unquoted, so a name with non-ASCII bytes still matches the task's globs.
fn nul_paths(out: &str) -> Vec<String> {
    out.strip_suffix('\0').unwrap_or(out).split('\0').filter(|l| !l.is_empty()).map(String::from).collect()
}

pub fn task_diff(top: &Path, base: &str, globs: &[String]) -> Result<TaskDiff> {
    let set = crate::fingerprint::globset(globs)?;
    let mut untracked = std::collections::HashSet::new();
    for path in nul_paths(&run(top, &["ls-files", "--others", "--exclude-standard", "-z"])?) {
        untracked.insert(path);
    }
    let mut names = nul_paths(&run(top, &["diff", "--no-renames", "--name-only", "-z", base, "--"])?);
    names.extend(untracked.iter().cloned());
    names.sort();
    names.dedup();
    let files: Vec<String> = names.into_iter().filter(|f| set.is_match(f)).collect();
    let mut diff = String::new();
    for f in &files {
        // quotePath=false keeps non-ASCII names raw in the patch headers,
        // so an --in-diff consumer sees the same path as RVOW_CHANGED_FILES.
        let part = if untracked.contains(f) {
            diff_no_index(top, f)?
        } else {
            run(top, &["--literal-pathspecs", "-c", "core.quotePath=false", "diff", "--no-renames", base, "--", f])?
        };
        if !part.is_empty() {
            if !diff.is_empty() {
                diff.push('\n');
            }
            diff.push_str(&part);
        }
    }
    Ok(TaskDiff { diff, files })
}

/// `git diff --no-index /dev/null <file>`: the added-file diff of an
/// untracked file. It exits 1 on a difference, which is the expected case.
fn diff_no_index(top: &Path, rel: &str) -> Result<String> {
    let out = Command::new(bin())
        .arg("-C")
        .arg(top)
        .args(["--literal-pathspecs", "-c", "core.quotePath=false", "diff", "--no-index", "--", "/dev/null", rel])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .map_err(|e| Error::new("git", format!("cannot run git: {e}"), "install git"))?;
    match out.status.code() {
        Some(0) | Some(1) => Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string()),
        _ => Err(Error::new(
            "git",
            format!("git diff --no-index failed: {}", String::from_utf8_lossy(&out.stderr).trim()),
            "run the command in a git repository",
        )),
    }
}

/// Turns a remote URL into a `host/owner/repo` path as ghq does, or `None` for a
/// local path or a URL with an unsafe part.
pub fn remote_path(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let rest = if let Some((_, rest)) = url.split_once("://") {
        let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
        let (host, path) = rest.split_once('/')?;
        let host = host.split(':').next()?;
        format!("{host}/{path}")
    } else {
        let (user_host, path) = url.split_once(':')?;
        let host = user_host.rsplit_once('@').map_or(user_host, |(_, h)| h);
        if host.is_empty() || host.contains('/') {
            return None;
        }
        format!("{host}/{path}")
    };
    let parts: Vec<&str> = rest.split('/').collect();
    let safe = parts.len() >= 3 && parts.iter().all(|p| !p.is_empty() && *p != "." && *p != ".." && !p.contains('\\'));
    safe.then(|| parts.join("/").to_lowercase())
}

#[cfg(test)]
pub(crate) mod testrepo {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A fresh directory under the system temp dir, removed on drop.
    pub struct TempDir(pub PathBuf);

    impl TempDir {
        pub fn new() -> TempDir {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("rvow-test-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir.canonicalize().unwrap())
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Runs git with the user's global and system config ignored, so fixture
    /// repositories never depend on or sign with the user's settings.
    pub fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new(super::bin())
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(ok.status.success(), "git {args:?}: {ok:?}");
    }

    pub fn repo_with_commit(parent: &Path, name: &str) -> PathBuf {
        let dir = parent.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-q", "-m", "init"]);
        dir
    }
}

#[cfg(test)]
mod tests {
    use super::testrepo::*;
    use super::*;

    #[test]
    fn remote_paths() {
        let cases = [
            ("git@github.com:Ray/rvow.git", Some("github.com/ray/rvow")),
            ("https://github.com/ray/rvow", Some("github.com/ray/rvow")),
            ("https://user@github.com/ray/rvow.git/", Some("github.com/ray/rvow")),
            ("ssh://git@host.x:2222/a/b/c.git", Some("host.x/a/b/c")),
            ("/srv/repos/rvow.git", None),
            ("../rvow", None),
            ("git@github.com:ray/../x.git", None),
            ("https://github.com/rvow", None),
        ];
        for (url, want) in cases {
            assert_eq!(remote_path(url).as_deref(), want, "{url}");
        }
    }

    #[test]
    fn queries_a_real_repository() {
        let tmp = TempDir::new();
        let empty = tmp.0.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        git(&empty, &["init", "-q"]);
        assert_eq!(root_commits(&empty).unwrap(), Vec::<String>::new());

        let repo = repo_with_commit(&tmp.0, "r");
        assert_eq!(root_commits(&repo).unwrap().len(), 1);
        assert!(!is_shallow(&repo).unwrap());
        assert_eq!(branch(&repo).unwrap().as_deref(), Some("main"));
        assert_eq!(origin_url(&repo), None);
        assert_eq!(toplevel(&repo.join(".")).unwrap(), repo);

        git(&repo, &["checkout", "-q", "--detach"]);
        assert_eq!(branch(&repo).unwrap(), None);
    }
}
