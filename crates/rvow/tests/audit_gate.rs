use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rvow_core::{archive, change, gate, proof, store, vow};

struct Fixture {
    dir: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    project: store::Project,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Fixture {
    fn new(name: &str, config: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rvow-audit-gate-{}-{name}", std::process::id()));
        let repo = dir.join("repo");
        let home = dir.join("home");
        std::fs::create_dir_all(&repo).unwrap();
        let command = |args: &[&str]| {
            let output = git_command(&repo).args(args).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        };
        command(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("initial.txt"), "initial").unwrap();
        command(&["add", "initial.txt"]);
        command(&["commit", "-q", "-m", "initial"]);
        let project = store::init(&repo, &home, store::Mode::Home).unwrap();
        std::fs::write(project.root.join("config.toml"), config).unwrap();
        Self { dir, repo, home, project }
    }

    fn git(&self, args: &[&str]) {
        let output = git_command(&self.repo).args(args).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }

    fn accept(&self, files: &str) -> (String, String) {
        let branch = rvow_core::git::branch(&self.repo).unwrap().unwrap();
        let plan = change::new(&self.project, "Write output", &branch, true).unwrap();
        std::fs::write(
            plan.path.join("tasks.md"),
            format!(
                "- TSK-new1 Write output\n  verify: grep -qx ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: {files}\n"
            ),
        )
        .unwrap();
        let prepared = vow::prepare(&self.project, &self.home, plan, &self.repo, &mut std::io::sink()).unwrap();
        let accepted = vow::commit(&self.project, prepared).unwrap();
        let task = accepted.minted.iter().find(|(old, _)| old == "TSK-new1").unwrap().1.to_string();
        (accepted.id.to_string(), task)
    }

    fn prove(&self, task: &str) {
        std::fs::write(self.repo.join("out.txt"), "ok\n").unwrap();
        assert!(proof::prove(&self.project, &self.home, &self.repo, task, &mut std::io::sink()).unwrap().proved);
    }

    fn archive(&self, change: &str) -> rvow_core::error::Result<archive::Archived> {
        archive::archive(&self.project, &self.home, &self.repo, change)
    }

    fn gate(&self, rerun: bool) -> gate::Report {
        gate::gate(&self.project, &self.home, &self.repo, gate::Options { strict: true, rerun, out: None }, &mut std::io::sink()).unwrap()
    }

    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_rvow"))
            .args(args)
            .current_dir(&self.repo)
            .env("HOME", &self.dir)
            .env("RVOW_HOME", &self.home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap()
    }
}

fn git_command(repo: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(repo)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid");
    command
}

#[test]
fn audit_gate_archived_regressions_fail_without_an_active_change() {
    let fixture = Fixture::new("archived", "");
    let (change, task) = fixture.accept("out.txt");
    fixture.prove(&task);
    fixture.git(&["add", "out.txt"]);
    fixture.git(&["commit", "-q", "-m", "output"]);
    fixture.archive(&change).unwrap();
    fixture.git(&["checkout", "-q", "-b", "regression"]);
    std::fs::write(fixture.repo.join("out.txt"), "bad\n").unwrap();
    fixture.git(&["add", "out.txt"]);
    fixture.git(&["commit", "-q", "-m", "regression"]);
    let report = fixture.gate(true);
    assert!(!report.passed(), "{report:?}");
    assert_eq!(report.changes, 0);
    assert!(report.archived.iter().any(|failure| failure.contains(&task)), "{report:?}");
    for mode in ["--toon", "--human"] {
        let output = fixture.cli(&["gate", "--strict", "--rerun", mode]);
        assert_eq!(output.status.code(), Some(1), "{}", String::from_utf8_lossy(&output.stdout));
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains(&task) && text.contains("archived"), "{text}");
    }
    let trees = git_command(&fixture.repo).args(["worktree", "list", "--porcelain"]).output().unwrap();
    assert_eq!(String::from_utf8(trees.stdout).unwrap().matches("worktree ").count(), 1);
}

#[test]
fn audit_gate_configured_checks_run_without_an_active_change() {
    let fixture = Fixture::new("empty-check", "checks = [\"false\"]\n");
    for rerun in [false, true] {
        let report = fixture.gate(rerun);
        assert!(!report.passed(), "{report:?}");
        assert!(report.failures.iter().any(|failure| failure.contains("check failed: false")), "{report:?}");
    }
}

#[test]
fn audit_gate_archive_refuses_repository_check_failures() {
    let fixture = Fixture::new("archive-check", "checks = [\"test ! -e fail-check\"]\n");
    let (change, task) = fixture.accept("out.txt, fail-check");
    std::fs::write(fixture.repo.join("fail-check"), "fail").unwrap();
    fixture.prove(&task);
    let result = fixture.archive(&change);
    assert!(result.is_err(), "archive bypassed a failing configured check");
    let error = result.unwrap_err();
    assert_eq!(error.code, "gate");
    assert!(error.message.contains("check failed"), "{error}");
    assert_eq!(change::resolve(&fixture.project, &change).unwrap().stage, change::Stage::Active);
}

#[test]
fn audit_gate_archive_refuses_out_of_scope_edits() {
    let fixture = Fixture::new("archive-scope", "");
    let (change, task) = fixture.accept("out.txt");
    fixture.prove(&task);
    std::fs::write(fixture.repo.join("outside.txt"), "outside").unwrap();
    let result = fixture.archive(&change);
    assert!(result.is_err(), "archive bypassed task scope");
    let error = result.unwrap_err();
    assert_eq!(error.code, "gate");
    assert!(error.message.contains("outside.txt"), "{error}");
}

#[test]
fn audit_gate_archive_refuses_detached_or_different_branches() {
    for detached in [false, true] {
        let fixture = Fixture::new(if detached { "archive-detached" } else { "archive-branch" }, "");
        let (change, task) = fixture.accept("out.txt");
        fixture.prove(&task);
        if detached {
            fixture.git(&["checkout", "-q", "--detach"]);
        } else {
            fixture.git(&["checkout", "-q", "-b", "other"]);
        }
        let result = fixture.archive(&change);
        assert!(result.is_err(), "archive bypassed its accepted branch");
        assert_eq!(result.unwrap_err().code, "gate");
    }
}

#[test]
fn audit_gate_archive_still_allows_other_unfinished_changes() {
    let fixture = Fixture::new("archive-selected", "");
    let (first, task) = fixture.accept("out.txt");
    let (_second, _missing_task) = fixture.accept("out.txt");
    fixture.prove(&task);
    fixture.archive(&first).unwrap();
}
