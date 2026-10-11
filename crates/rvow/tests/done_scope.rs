//! A done change counts toward scope only while its work is not in the base.
//! Each test drives the built binary in a throwaway repository.

use std::path::{Path, PathBuf};
use std::process::Command;

use rvow_core::{change, ledger, store};

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");

struct Fx {
    root: PathBuf,
    home: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Fx {
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-donescope-{name}-{}", std::process::id()));
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

    /// The archive event the old binary wrote: no `head`, so the gate must
    /// fall back to the vow base. Appended with the product API, never edited.
    fn legacy_archive(&self, work: &Path, change: &str) {
        let project = store::find(work, &self.home).unwrap().unwrap();
        let dir = change::resolve(&project, change).unwrap();
        let lock = project.lock().unwrap();
        let mut data = serde_json::Map::new();
        data.insert("merged".into(), serde_json::json!({}));
        ledger::append(&dir.path.join(change::LEDGER), &lock, ledger::Kind::Archive, data).unwrap();
        let name = dir.path.file_name().unwrap().to_str().unwrap().to_string();
        let dest = project.root.join("archive/done").join(format!("2020-01-02-{name}"));
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::rename(&dir.path, &dest).unwrap();
    }
}

#[test]
fn done_scope_old_done_does_not_widen() {
    let fx = Fx::new("old-done");
    let work = fx.repo();
    let (done, task) = fx.accepted(&work, "Old", "old.txt");
    fx.prove_task(&work, &task, "old.txt");
    fx.archive_and_commit(&work, &done);
    let (_, active) = fx.accepted(&work, "New", "out.txt");
    fx.prove_task(&work, &active, "out.txt");
    std::fs::write(work.join("old.txt"), "changed\n").unwrap();
    let (out, code) = fx.gate(&work);
    assert_ne!(code, 0, "the gate passes but old.txt is outside every scope: {out}");
    assert!(out.contains("old.txt"), "the gate fails without naming old.txt: {out}");
}

#[test]
fn done_scope_feature_branch_still_counts() {
    let fx = Fx::new("feature-branch");
    let work = fx.repo();
    fx.git(&work, &["checkout", "-q", "-b", "job"]);
    let (done, task) = fx.accepted(&work, "Old", "a.txt");
    fx.prove_task(&work, &task, "a.txt");
    fx.archive_and_commit(&work, &done);
    let (_, active) = fx.accepted(&work, "New", "b.txt");
    fx.prove_task(&work, &active, "b.txt");
    std::fs::write(work.join("a.txt"), "changed\n").unwrap();
    let (out, code) = fx.gate(&work);
    assert_eq!(code, 0, "the gate fails on the feature branch: {out}");
    assert!(!out.contains("a.txt"), "the gate names a.txt: {out}");
}

#[test]
fn done_scope_uncommitted_work_still_counts() {
    let fx = Fx::new("uncommitted");
    let work = fx.repo();
    let (done, task) = fx.accepted(&work, "Old", "old.txt");
    fx.prove_task(&work, &task, "old.txt");
    fx.rvow(&work, &["archive", &done]);
    let (_, active) = fx.accepted(&work, "New", "out.txt");
    fx.prove_task(&work, &active, "out.txt");
    let (out, code) = fx.gate(&work);
    assert_eq!(code, 0, "the gate fails before any new commit: {out}");
    assert!(!out.contains("old.txt"), "the gate names old.txt: {out}");
}

#[test]
fn done_scope_legacy_uses_vow_base() {
    let fx = Fx::new("legacy");
    let work = fx.repo();
    let (done, task) = fx.accepted(&work, "Old", "old.txt");
    fx.prove_task(&work, &task, "old.txt");
    fx.legacy_archive(&work, &done);
    fx.git(&work, &["add", "-A"]);
    fx.git(&work, &["commit", "-q", "-m", "archive"]);
    let (_, active) = fx.accepted(&work, "New", "out.txt");
    fx.prove_task(&work, &active, "out.txt");
    std::fs::write(work.join("old.txt"), "changed\n").unwrap();
    let (out, code) = fx.gate(&work);
    assert_ne!(code, 0, "the gate passes but the legacy done change is in the base: {out}");
    assert!(out.contains("old.txt"), "the gate fails without naming old.txt: {out}");
}

#[test]
fn done_scope_backlog_names_waiting_plan() {
    let fx = Fx::new("backlog");
    let work = fx.repo();
    let (done, task) = fx.accepted(&work, "Old", "old.txt");
    fx.prove_task(&work, &task, "old.txt");
    fx.archive_and_commit(&work, &done);
    let out = fx.rvow(&work, &["new", "Later"]);
    let waiting = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
    std::fs::write(work.join("old.txt"), "changed\n").unwrap();
    let (out, code) = fx.gate(&work);
    assert_ne!(code, 0, "the gate passes but old.txt is outside every scope: {out}");
    assert!(out.contains("old.txt"), "the gate fails without naming old.txt: {out}");
    assert!(out.contains(&waiting), "the gate fails without naming the waiting plan: {out}");
}
