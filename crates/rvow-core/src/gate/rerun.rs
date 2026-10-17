use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::Report;
use crate::config::Step;
use crate::error::{Error, Result};
use crate::fingerprint::{self, globset};
use crate::git;
use crate::proof::{self, Active, Now};
use crate::runner;
use crate::tasks::Task;
use crate::vow::sha256_hex;

/// How many times a warm tree lock is tried before the run falls back to a temporary tree.
const LOCK_TRIES: u32 = 10;
/// The wait between two tries of the warm tree lock.
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_millis(10);

/// A detached worktree of one commit. This is the one place where rvow writes
/// to git. The gate rerun uses a clean temporary one; the base commit baseline
/// uses the project's warm one, set up the same way.
pub(crate) struct Worktree {
    repo: PathBuf,
    pub(crate) path: PathBuf,
    /// The held lock of a warm tree, which drop keeps. A temporary tree has
    /// none, and drop removes it.
    warm: Option<std::fs::File>,
}

impl Worktree {
    /// A temporary clean checkout at `commit`, removed on drop.
    pub(crate) fn add_at(repo: &Path, commit: &str, links: &[String]) -> Result<Worktree> {
        let b = Error::random::<6>()?;
        let name: String = b.iter().map(|x| format!("{x:02x}")).collect();
        let path = std::env::temp_dir().join(format!("rvow-rerun-{name}"));
        let p = path.to_string_lossy().into_owned();
        git::run(repo, &["worktree", "add", "--detach", "--quiet", &p, commit])?;
        let tree = Worktree { repo: repo.to_path_buf(), path, warm: None };
        tree.link(links)?;
        Ok(tree)
    }

    /// The clean checkout at `commit` in the project's warm tree at `dir`, so
    /// ignored build output from earlier runs stays and builds start warm. A
    /// busy warm tree, or any git step that fails on it, falls back to a
    /// temporary tree, so a run never waits and never fails because of it.
    pub(crate) fn warm(repo: &Path, commit: &str, links: &[String], dir: &Path) -> Result<Worktree> {
        match Self::try_warm(repo, commit, dir) {
            Some(tree) => {
                tree.link(links)?;
                Ok(tree)
            }
            None => Self::add_at(repo, commit, links),
        }
    }

    fn try_warm(repo: &Path, commit: &str, dir: &Path) -> Option<Worktree> {
        // `git clean` runs here, so only ever on a folder under a `worktrees` folder.
        let parent = dir.parent()?;
        if parent.file_name()? != "worktrees" {
            return None;
        }
        std::fs::create_dir_all(parent).ok()?;
        let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.with_extension("lock")).ok()?;
        // A child process that another thread starts holds a copy of the lock
        // for a moment, so a few short tries tell that from a real other run.
        let mut tries = 0;
        while lock.try_lock().is_err() {
            tries += 1;
            if tries == LOCK_TRIES {
                return None;
            }
            std::thread::sleep(LOCK_WAIT);
        }
        // A name such as HEAD means the repository's commit, not the warm tree's.
        let sha = git::run(repo, &["rev-parse", "--verify", &format!("{commit}^{{commit}}")]).ok()?;
        let p = dir.to_string_lossy().into_owned();
        let moved = same_repo(repo, dir)
            && git::run(dir, &["checkout", "--detach", "--force", "--quiet", &sha]).is_ok()
            && git::run(dir, &["clean", "-fdq"]).is_ok();
        if !moved {
            let _ = std::fs::remove_dir_all(dir);
            let _ = git::run(repo, &["worktree", "prune"]);
            git::run(repo, &["worktree", "add", "--detach", "--quiet", &p, &sha]).ok()?;
        }
        Some(Worktree { repo: repo.to_path_buf(), path: dir.to_path_buf(), warm: Some(lock) })
    }

    /// Links each `rerun_links` directory of the real repo into the tree.
    /// Nothing is linked unless the config names it: the tree stays a clean
    /// checkout of HEAD.
    fn link(&self, links: &[String]) -> Result<()> {
        let bad = |name: &str, why: &str| {
            Error::new(
                "config",
                format!("rerun_links entry {name:?} {why}"),
                "fix or remove that name in the rerun_links key of config.toml",
            )
        };
        let real = self.repo.canonicalize().map_err(|e| Error::io(&self.repo, e))?;
        for name in links {
            let rel = Path::new(name);
            if name.is_empty() || !rel.components().all(|c| matches!(c, std::path::Component::Normal(_))) {
                return Err(bad(name, "is not a relative path inside the repository"));
            }
            if !git::run(&self.repo, &["ls-files", "--", name])?.trim().is_empty() {
                return Err(bad(name, "is tracked by git"));
            }
            let src = self.repo.join(rel);
            let Ok(src) = src.canonicalize() else { continue };
            if !src.starts_with(&real) {
                return Err(bad(name, "resolves outside the repository"));
            }
            if !src.is_dir() {
                continue;
            }
            let dst = self.path.join(rel);
            // A warm tree keeps the link, or build output, of an earlier run. Clear it only when
            // its folder is really inside the tree: through an earlier link it is the user's.
            let inside = || {
                let tree = self.path.canonicalize().ok()?;
                dst.parent()?.canonicalize().ok().filter(|p| p.starts_with(&tree))
            };
            if self.warm.is_some()
                && let Ok(meta) = dst.symlink_metadata()
                && inside().is_some()
            {
                let _ = if meta.is_dir() { std::fs::remove_dir_all(&dst) } else { std::fs::remove_file(&dst) };
            }
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
            }
            symlink_dir(&src, &dst)?;
        }
        Ok(())
    }
}

#[cfg(unix)]
fn symlink_dir(src: &Path, dst: &Path) -> Result<()> {
    std::os::unix::fs::symlink(src, dst).map_err(|e| {
        Error::new(
            "link",
            format!("cannot link {} to {}: {e}", dst.display(), src.display()),
            "remove the name from rerun_links in config.toml",
        )
    })
}

#[cfg(not(unix))]
fn symlink_dir(_src: &Path, dst: &Path) -> Result<()> {
    Err(Error::new(
        "link",
        format!("cannot link {}: rerun_links needs symlinks", dst.display()),
        "remove rerun_links from config.toml on this platform",
    ))
}

/// Whether `dir` is a worktree of the repository at `repo`.
fn same_repo(repo: &Path, dir: &Path) -> bool {
    let common = |d: &Path| git::run(d, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).ok();
    dir.join(".git").exists() && common(dir).is_some() && common(dir) == common(repo)
}

impl Drop for Worktree {
    fn drop(&mut self) {
        if self.warm.is_some() {
            return;
        }
        let p = self.path.to_string_lossy().into_owned();
        if git::run(&self.repo, &["worktree", "remove", "--force", &p]).is_err() {
            let _ = std::fs::remove_dir_all(&self.path);
            let _ = git::run(&self.repo, &["worktree", "prune"]);
        }
    }
}

/// What decides whether a scoped `setup` or `checks` entry runs: the tasks
/// that will run, and the files that the branch changed.
pub(crate) struct Scope {
    head: Vec<String>,
    tasks: Vec<globset::GlobSet>,
    changed: Option<Vec<String>>,
}

impl Scope {
    pub(crate) fn new(top: &Path, base: &str, misses: &[&Task]) -> Result<Scope> {
        let head = git::run(top, &["ls-tree", "-r", "--name-only", "HEAD"])?.lines().map(String::from).collect();
        let tasks = misses.iter().map(|t| globset(&t.files)).collect::<Result<Vec<_>>>()?;
        let changed = git::base_ref(top, base)
            .and_then(|b| git::run(top, &["merge-base", "HEAD", &b]).ok())
            .and_then(|mb| changed_files(top, &mb).ok());
        Ok(Scope { head, tasks, changed })
    }

    /// A plain entry always runs. A scoped entry runs when a task that runs
    /// shares a file with its globs, or a changed file matches one of them. An
    /// unknown base counts as a match, so nothing is skipped by mistake.
    pub(crate) fn wants(&self, step: &Step) -> Result<bool> {
        let Some(files) = step.files() else { return Ok(true) };
        let set = globset(files)?;
        if self.head.iter().any(|f| set.is_match(f) && self.tasks.iter().any(|t| t.is_match(f))) {
            return Ok(true);
        }
        Ok(self.changed.as_ref().is_none_or(|changed| changed.iter().any(|f| set.is_match(f))))
    }
}

/// The rerun worktree, made on first use together with the setup commands, so
/// a gate that every cached task answers creates neither.
pub(super) struct Lazy<'a> {
    pub(super) top: &'a Path,
    pub(super) links: Vec<String>,
    pub(super) setup: Vec<String>,
    pub(super) timeout: std::time::Duration,
    pub(super) env: &'a [(String, String)],
    tree: Option<Worktree>,
    failed: bool,
}

impl<'a> Lazy<'a> {
    pub(super) fn new(
        top: &'a Path,
        links: Vec<String>,
        setup: Vec<String>,
        timeout: std::time::Duration,
        env: &'a [(String, String)],
    ) -> Lazy<'a> {
        Lazy { top, links, setup, timeout, env, tree: None, failed: false }
    }

    /// The tree to run in, or `None` when a setup command failed, which is
    /// already in the report.
    pub(super) fn dir(&mut self, r: &mut Report, echo: &mut dyn Write) -> Result<Option<PathBuf>> {
        if self.failed {
            return Ok(None);
        }
        if let Some(tree) = &self.tree {
            return Ok(Some(tree.path.clone()));
        }
        // The gate promises a clean checkout of HEAD, so it never uses the warm tree.
        let tree = Worktree::add_at(self.top, "HEAD", &self.links)?;
        let dir = tree.path.clone();
        self.tree = Some(tree);
        for cmd in &self.setup {
            let _ = writeln!(echo, "setup: {cmd}");
            if !runner::run(cmd, &dir, self.env, self.timeout, echo)?.passed() {
                r.failures.push(format!("setup command failed: {cmd}"));
                self.failed = true;
                return Ok(None);
            }
        }
        Ok(Some(dir))
    }

    pub(super) fn failed(&self) -> bool {
        self.failed
    }

    /// The tree if one was made, else the repository itself.
    pub(super) fn current(&self) -> PathBuf {
        self.tree.as_ref().map_or_else(|| self.top.to_path_buf(), |t| t.path.clone())
    }
}

/// Files that differ from `mb` in the worktree, plus untracked files git does
/// not ignore.
pub(super) fn changed_files(top: &Path, mb: &str) -> Result<Vec<String>> {
    let mut files: Vec<String> = git::run(top, &["diff", "--name-only", mb])?.lines().map(String::from).collect();
    files.extend(git::run(top, &["ls-files", "-o", "--exclude-standard"])?.lines().map(String::from));
    files.sort();
    files.dedup();
    Ok(files)
}
/// What the rerun ran: the filled command, whether its junit check failed,
/// and the runner stamp `rvow prove` records.
pub(super) struct Reran<'a> {
    pub cmd: &'a str,
    pub junit_failed: bool,
    pub runner: Option<&'a str>,
}

/// The Prove event data for one rerun, matching what `rvow prove` records, so
/// an imported run is recognized as a proof. Computed in the rerun tree.
/// A failed junit check fails the proof even when the command passed, and
/// the runner stamp keeps an ingested rerun of an `@runner` task proved
/// instead of stale.
pub(super) fn prove_data(
    c: &Active,
    t: &Task,
    o: &runner::Outcome,
    now: &Now,
    inputs: &fingerprint::Inputs,
    ran: &Reran,
) -> Map<String, Value> {
    let cmd = ran.cmd;
    let junit_failed = ran.junit_failed;
    let runner = ran.runner;
    let thash = crate::vow::task_hash(t);
    let reqs = proof::covered(t, &c.loaded, now.living);
    let env_key = now.env.key();
    let key = sha256_hex(serde_json::json!([thash, reqs, inputs.hash, env_key]).to_string().as_bytes());
    let mut data = Map::new();
    data.insert("task".into(), t.id.to_string().into());
    data.insert("task_hash".into(), thash.into());
    data.insert("requirements".into(), serde_json::json!(reqs));
    data.insert("verify".into(), cmd.into());
    data.insert("proved".into(), (o.passed() && o.tests_run != Some(0) && !junit_failed).into());
    data.insert("exit".into(), serde_json::json!(o.exit));
    data.insert("timed_out".into(), o.timed_out.into());
    data.insert("tests_run".into(), serde_json::json!(o.tests_run));
    data.insert("started".into(), crate::ledger::now_ms().into());
    data.insert("duration_ms".into(), o.duration_ms.into());
    data.insert("tail".into(), serde_json::json!(o.tail));
    data.insert("inputs".into(), inputs.hash.clone().into());
    data.insert("input_files".into(), inputs.files.into());
    data.insert("depends".into(), serde_json::json!(proof::dependency_inputs(&c.loaded.tasks, t, now)));
    data.insert("env".into(), serde_json::json!(now.env));
    data.insert("env_key".into(), env_key.into());
    data.insert("key".into(), key.into());
    data.insert("runner".into(), serde_json::json!(runner));
    data.insert("programs".into(), serde_json::json!(proof::verify_programs(cmd)));
    data.insert(
        "run".into(),
        serde_json::json!({ "invocation": invocation(), "rvow": env!("CARGO_PKG_VERSION"), "machine": proof::machine(now.config), "agent": proof::agent(now.config), "head": git::run(now.top, &["rev-parse", "HEAD"]).unwrap_or_default(), "branch": git::branch(now.top).ok().flatten(), "ci": fingerprint::ci() }),
    );
    data
}

fn invocation() -> String {
    Error::random::<8>().map(|b| b.iter().map(|x| format!("{x:02x}")).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;

    /// A repository that ignores `build/`, and the warm tree path under a home.
    fn fixture(tmp: &TempDir) -> (PathBuf, PathBuf) {
        let repo = repo_with_commit(&tmp.0, "r");
        std::fs::write(repo.join(".gitignore"), "build/\n").unwrap();
        git(&repo, &["add", ".gitignore"]);
        git(&repo, &["commit", "-q", "-m", "ignore"]);
        (repo, tmp.0.join("home").join("worktrees").join("p"))
    }

    #[test]
    fn warm_worktree_reuses_the_folder_and_keeps_ignored_output() {
        let tmp = TempDir::new();
        let (repo, dir) = fixture(&tmp);
        let tree = Worktree::warm(&repo, "HEAD", &[], &dir).unwrap();
        assert_eq!(tree.path, dir);
        std::fs::create_dir_all(dir.join("build")).unwrap();
        std::fs::write(dir.join("build/out.o"), "o").unwrap();
        drop(tree);
        let tree = Worktree::warm(&repo, "HEAD", &[], &dir).unwrap();
        assert_eq!(tree.path, dir, "the same folder");
        assert!(dir.join("build/out.o").is_file(), "ignored build output stays");
    }

    #[test]
    fn warm_worktree_matches_the_commit() {
        let tmp = TempDir::new();
        let (repo, dir) = fixture(&tmp);
        drop(Worktree::warm(&repo, "HEAD", &[], &dir).unwrap());
        std::fs::write(dir.join("a.txt"), "edited").unwrap();
        std::fs::write(dir.join("stray.txt"), "s").unwrap();
        std::fs::write(repo.join("a.txt"), "b").unwrap();
        git(&repo, &["commit", "-q", "-am", "b"]);
        let tree = Worktree::warm(&repo, "HEAD", &[], &dir).unwrap();
        assert_eq!(std::fs::read_to_string(tree.path.join("a.txt")).unwrap(), "b", "the tracked file equals the new commit");
        assert!(!tree.path.join("stray.txt").exists(), "an untracked file that git does not ignore is gone");
    }

    #[test]
    fn warm_worktree_falls_back_when_busy() {
        let tmp = TempDir::new();
        let (repo, dir) = fixture(&tmp);
        let held = Worktree::warm(&repo, "HEAD", &[], &dir).unwrap();
        let other = Worktree::warm(&repo, "HEAD", &[], &dir).unwrap();
        assert_ne!(other.path, dir, "a busy warm tree falls back to a temporary one");
        assert!(other.path.join("a.txt").is_file());
        let temp = other.path.clone();
        drop(other);
        assert!(!temp.exists(), "the temporary tree is removed");
        drop(held);
        assert!(dir.join("a.txt").is_file(), "the warm tree stays");
    }

    #[test]
    fn warm_worktree_never_clears_a_folder_through_a_link() {
        // `build/sub` sits under the `build` link, so its place is the user's folder.
        let tmp = TempDir::new();
        let (repo, dir) = fixture(&tmp);
        std::fs::create_dir_all(repo.join("build/sub")).unwrap();
        std::fs::write(repo.join("build/sub/keep.txt"), "mine").unwrap();
        let links = ["build".to_string(), "build/sub".to_string()];
        let first = Worktree::warm(&repo, "HEAD", &links, &dir);
        drop(first);
        let second = Worktree::warm(&repo, "HEAD", &links, &dir);
        drop(second);
        assert!(repo.join("build/sub/keep.txt").is_file(), "the user's folder behind a link was deleted");
    }

    #[test]
    fn warm_worktree_is_made_again_after_removal() {
        let tmp = TempDir::new();
        let (repo, dir) = fixture(&tmp);
        drop(Worktree::warm(&repo, "HEAD", &[], &dir).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
        let tree = Worktree::warm(&repo, "HEAD", &[], &dir).unwrap();
        assert_eq!(tree.path, dir);
        assert!(dir.join("a.txt").is_file());
    }
}
