//! The gate counts the work of a parent branch in a stacked branch (CHG-22v53h).
//! Each test drives the built binary in a throwaway repository.

use std::path::{Path, PathBuf};
use std::process::Command;

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");

struct Fx {
    root: PathBuf,
    home: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        rvow_core::git::testrepo::remove_tree(&self.root);
    }
}

impl Fx {
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-stacked-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        Fx { home: root.join("rh"), root }
    }

    fn env(&self, c: &mut Command) {
        c.env("HOME", &self.root)
            .env("RVOW_HOME", &self.home)
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("PI_CODING_AGENT_DIR")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t");
    }

    fn git(&self, dir: &Path, args: &[&str]) {
        let mut c = Command::new("git");
        c.current_dir(dir);
        self.env(&mut c);
        let out = c.args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn rvow(&self, dir: &Path, args: &[&str]) -> String {
        let mut c = Command::new(RVOW);
        c.current_dir(dir);
        self.env(&mut c);
        let out = c.args(args).output().unwrap();
        assert!(out.status.success(), "rvow {args:?}: {}", String::from_utf8_lossy(&out.stdout));
        String::from_utf8(out.stdout).unwrap()
    }

    fn gate(&self, dir: &Path) -> (String, i32) {
        let mut c = Command::new(RVOW);
        c.current_dir(dir);
        self.env(&mut c);
        let out = c.args(["gate"]).output().unwrap();
        (String::from_utf8(out.stdout).unwrap(), out.status.code().unwrap())
    }

    /// A repository with one commit on `main`.
    fn repo(&self) -> PathBuf {
        let work = self.root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        self.git(&work, &["init", "-q", "-b", "main"]);
        std::fs::write(work.join("a.txt"), "a").unwrap();
        self.git(&work, &["add", "a.txt"]);
        self.git(&work, &["commit", "-q", "-m", "init"]);
        self.rvow(&work, &["init"]);
        work.canonicalize().unwrap()
    }

    /// One small accepted change naming `file`; returns the change id and task id.
    fn accepted(&self, work: &Path, title: &str, file: &str) -> (String, String) {
        let out = self.rvow(work, &["new", title]);
        let field = |k: &str| out.lines().find_map(|l| l.strip_prefix(&format!("{k}: "))).unwrap().to_string();
        let (id, dir) = (field("created"), PathBuf::from(field("path")));
        std::fs::write(
            dir.join("tasks.md"),
            format!(
                "- TSK-new1 Write {file}\n  verify: grep -q ok {file} && echo 'test result: ok. 1 passed; 0 failed;'\n  files: {file}\n"
            ),
        )
        .unwrap();
        self.rvow(work, &["accept", &id, "--yes"]);
        let brief = self.rvow(work, &["brief", "--human"]);
        let at = brief.find("TSK-").unwrap();
        (id, brief[at..at + 10].to_string())
    }

    fn prove_task(&self, work: &Path, task: &str, file: &str) {
        std::fs::write(work.join(file), "ok\n").unwrap();
        self.rvow(work, &["prove", task]);
    }

    fn archive_and_commit(&self, work: &Path, change: &str) {
        self.rvow(work, &["archive", change]);
        self.git(work, &["add", "-A"]);
        self.git(work, &["commit", "-q", "-m", "archive"]);
    }
}

fn commit_all(fx: &Fx, work: &Path, msg: &str) {
    fx.git(work, &["add", "-A"]);
    fx.git(work, &["commit", "-q", "-m", msg]);
}

#[test]
fn stacked_gate_done_parent_counts() {
    let fx = Fx::new("done-parent");
    let work = fx.repo();
    fx.git(&work, &["checkout", "-q", "-b", "a"]);
    let (done, task) = fx.accepted(&work, "Parent", "a.txt");
    fx.prove_task(&work, &task, "a.txt");
    fx.archive_and_commit(&work, &done);
    fx.git(&work, &["checkout", "-q", "-b", "b"]);
    let (_, active) = fx.accepted(&work, "Child", "b.txt");
    fx.prove_task(&work, &active, "b.txt");
    let (out, code) = fx.gate(&work);
    assert_eq!(code, 0, "the gate fails on the stacked branch: {out}");
    assert!(!out.contains("a.txt"), "the gate names a.txt: {out}");
}

#[test]
fn stacked_gate_active_parent_counts() {
    let fx = Fx::new("active-parent");
    let work = fx.repo();
    fx.git(&work, &["checkout", "-q", "-b", "a"]);
    let (_, task) = fx.accepted(&work, "Parent", "a.txt");
    fx.prove_task(&work, &task, "a.txt");
    commit_all(&fx, &work, "parent work");
    fx.git(&work, &["checkout", "-q", "-b", "b"]);
    let (_, active) = fx.accepted(&work, "Child", "b.txt");
    fx.prove_task(&work, &active, "b.txt");
    let (out, code) = fx.gate(&work);
    assert_eq!(code, 0, "the gate fails on the stacked branch: {out}");
    assert!(!out.contains("a.txt"), "the gate names a.txt: {out}");
}

#[test]
fn stacked_gate_backlog_scope_counts_the_parent() {
    let fx = Fx::new("backlog-parent");
    let work = fx.repo();
    fx.git(&work, &["checkout", "-q", "-b", "a"]);
    let (done, task) = fx.accepted(&work, "Parent", "a.txt");
    fx.prove_task(&work, &task, "a.txt");
    fx.archive_and_commit(&work, &done);
    fx.git(&work, &["checkout", "-q", "-b", "b"]);
    fx.rvow(&work, &["new", "Child"]);
    let (out, _) = fx.gate(&work);
    assert!(!out.contains("a.txt"), "the backlog scope check names the parent's a.txt: {out}");
}

#[test]
fn stacked_gate_sibling_does_not_count() {
    let fx = Fx::new("sibling");
    let work = fx.repo();
    fx.git(&work, &["checkout", "-q", "-b", "c"]);
    let (done, task) = fx.accepted(&work, "Sibling", "c.txt");
    fx.prove_task(&work, &task, "c.txt");
    fx.archive_and_commit(&work, &done);
    fx.git(&work, &["checkout", "-q", "main"]);
    fx.git(&work, &["checkout", "-q", "-b", "b"]);
    let (_, active) = fx.accepted(&work, "Child", "b.txt");
    fx.prove_task(&work, &active, "b.txt");
    std::fs::write(work.join("c.txt"), "changed\n").unwrap();
    let (out, code) = fx.gate(&work);
    assert_ne!(code, 0, "the gate passes but c.txt is outside every scope: {out}");
    assert!(out.contains("c.txt"), "the gate fails without naming c.txt: {out}");
}

#[test]
fn stacked_gate_merged_parent_does_not_count() {
    let fx = Fx::new("merged-parent");
    let work = fx.repo();
    fx.git(&work, &["checkout", "-q", "-b", "a"]);
    let (done, task) = fx.accepted(&work, "Parent", "a.txt");
    fx.prove_task(&work, &task, "a.txt");
    fx.archive_and_commit(&work, &done);
    fx.git(&work, &["checkout", "-q", "main"]);
    fx.git(&work, &["merge", "-q", "--ff-only", "a"]);
    fx.git(&work, &["checkout", "-q", "-b", "b"]);
    let (_, active) = fx.accepted(&work, "Child", "b.txt");
    fx.prove_task(&work, &active, "b.txt");
    std::fs::write(work.join("a.txt"), "changed\n").unwrap();
    let (out, code) = fx.gate(&work);
    assert_ne!(code, 0, "the gate passes but a.txt is in the base: {out}");
    assert!(out.contains("a.txt"), "the gate fails without naming a.txt: {out}");
}
