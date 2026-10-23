//! `rvow delegate <task> --worktree [path]` makes a worktree for the task or
//! records one that already exists (REQ-tq0e80, REQ-kd9986).

use std::path::{Path, PathBuf};
use std::process::Command;

use rvow_core::lease::{self, LEASE_FILE};
use rvow_core::store::{self, Project};

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");
const PASS: &str = "echo 'test result: ok. 1 passed; 0 failed;'";

struct Fx {
    root: PathBuf,
    repo: PathBuf,
    change: String,
    task: String,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = Command::new("git").args(["worktree", "prune"]).current_dir(&self.repo).output();
        rvow_core::git::testrepo::remove_tree(&self.root);
    }
}

fn field<'a>(out: &'a str, key: &str) -> &'a str {
    out.lines().find_map(|l| l.strip_prefix(&format!("{key}: "))).unwrap_or_else(|| panic!("no {key} in {out}"))
}

impl Fx {
    /// A repository on branch `feat/a` with an accepted one-task change.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-delegate-worktree-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        let root = root.canonicalize().unwrap();
        let mut fx = Fx { repo: root.join("repo"), root, change: String::new(), task: String::new() };
        let repo = fx.repo.clone();
        std::fs::write(repo.join("out.txt"), "").unwrap();
        fx.git(&repo, &["init", "-q", "-b", "main"]);
        fx.git(&repo, &["add", "."]);
        fx.git(&repo, &["commit", "-q", "-m", "init"]);
        fx.git(&repo, &["checkout", "-q", "-b", "feat/a"]);

        fx.rvow(&repo, &["init"]);
        let out = fx.rvow(&repo, &["new", "--spec", "Out"]);
        let dir = PathBuf::from(field(&out, "path"));
        std::fs::write(
            dir.join("tasks.md"),
            format!("- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: grep -q ok out.txt && {PASS}\n  files: out.txt\n"),
        )
        .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
        )
        .unwrap();
        fx.change = field(&out, "created").to_owned();
        fx.rvow(&repo, &["accept", &fx.change.clone(), "--yes"]);
        let located = fx.rvow(&repo, &["path", &fx.change.clone()]);
        let tasks = std::fs::read_to_string(PathBuf::from(field(&located, "path")).join("tasks.md")).unwrap();
        fx.task = tasks.lines().find_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).unwrap();
        fx
    }

    fn env(&self, c: &mut Command, dir: &Path) {
        // No `rgit` on this PATH, so the worktree is made with git.
        c.current_dir(dir)
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.root.join("home"))
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env_remove("RVOW_GIT")
            .env_remove("EVAL_RVOW_GIT")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t");
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let mut c = Command::new("git");
        self.env(&mut c, dir);
        let out = c.args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Runs rvow in the repository: stdout and exit code.
    fn run(&self, args: &[&str]) -> (String, i32) {
        let mut c = Command::new(RVOW);
        let repo = self.repo.clone();
        self.env(&mut c, &repo);
        let out = c.args(args).output().unwrap();
        (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap())
    }

    fn rvow(&self, dir: &Path, args: &[&str]) -> String {
        let mut c = Command::new(RVOW);
        self.env(&mut c, dir);
        let out = c.args(args).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn project(&self) -> Project {
        store::find(&self.repo, &self.root.join("home")).unwrap().unwrap()
    }

    fn leases(&self) -> Vec<lease::Lease> {
        lease::leases(&self.project()).unwrap_or_default()
    }

    fn lease_lines(&self) -> usize {
        std::fs::read_to_string(self.project().root.join(LEASE_FILE)).map_or(0, |t| t.lines().count())
    }

    fn branch(&self) -> String {
        format!("rvow/{}/{}", self.change, self.task)
    }

    fn wanted(&self) -> PathBuf {
        self.root.join("repo.worktrees").join(&self.task)
    }

    fn has_branch(&self, branch: &str) -> bool {
        let mut c = Command::new("git");
        self.env(&mut c, &self.repo.clone());
        c.args(["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).output().unwrap().status.success()
    }
}

#[test]
fn delegate_worktree_makes_the_branch_and_the_worktree() {
    let fx = Fx::new("make");
    let tip = fx.git(&fx.repo, &["rev-parse", "feat/a"]);
    let (out, code) = fx.run(&["delegate", &fx.task, "--worktree"]);
    assert_eq!(code, 0, "{out}");
    let branch = fx.branch();
    assert!(out.contains(&format!("worktree: {}", fx.wanted().display())), "{out}");
    assert!(out.contains(&format!("branch: {branch}")), "{out}");
    assert_eq!(fx.git(&fx.repo, &["rev-parse", &branch]), tip, "the branch starts at the tip of the change's branch");
    assert_eq!(fx.git(&fx.wanted(), &["rev-parse", "--abbrev-ref", "HEAD"]), branch);
    let leases = fx.leases();
    assert_eq!(leases.len(), 1);
    assert_eq!((leases[0].task.as_str(), leases[0].branch.as_deref()), (fx.task.as_str(), Some(branch.as_str())));
    assert!(leases[0].worktree.as_deref().is_some_and(|w| w.canonicalize().ok() == fx.wanted().canonicalize().ok()));
}

#[test]
fn delegate_worktree_an_existing_branch_fails_and_records_no_lease() {
    let fx = Fx::new("taken");
    let branch = fx.branch();
    fx.git(&fx.repo, &["branch", &branch, "feat/a"]);
    let (out, code) = fx.run(&["delegate", &fx.task, "--worktree"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(&branch), "{out}");
    assert_eq!(fx.lease_lines(), 0, "{out}");
    assert!(!fx.wanted().exists(), "no worktree is made");
}

#[test]
fn delegate_worktree_records_a_worktree_that_exists() {
    let fx = Fx::new("record");
    let wt = fx.root.join("w1");
    fx.git(&fx.repo, &["worktree", "add", "-q", "-b", "rmaestro/b", &wt.to_string_lossy()]);
    let (out, code) = fx.run(&["delegate", &fx.task, "--worktree", &wt.to_string_lossy()]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains(&format!("worktree: {}", wt.display())), "{out}");
    assert!(out.contains("branch: rmaestro/b"), "{out}");
    let leases = fx.leases();
    assert_eq!(leases.len(), 1);
    assert_eq!(leases[0].branch.as_deref(), Some("rmaestro/b"));
    assert!(leases[0].worktree.as_deref().is_some_and(|w| w.canonicalize().ok() == wt.canonicalize().ok()));
    assert!(!fx.has_branch(&fx.branch()), "no new branch is made");
}

#[test]
fn delegate_worktree_a_folder_that_is_no_worktree_fails_and_records_no_lease() {
    let fx = Fx::new("empty");
    let empty = fx.root.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let (out, code) = fx.run(&["delegate", &fx.task, "--worktree", &empty.to_string_lossy()]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("worktree"), "{out}");
    assert_eq!(fx.lease_lines(), 0, "{out}");
}

#[test]
fn delegate_worktree_the_main_checkout_fails_and_records_no_lease() {
    let fx = Fx::new("main");
    let (out, code) = fx.run(&["delegate", &fx.task, "--worktree", &fx.repo.to_string_lossy()]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("main checkout") && out.contains("worktree of its own"), "{out}");
    assert_eq!(fx.lease_lines(), 0, "{out}");
}
