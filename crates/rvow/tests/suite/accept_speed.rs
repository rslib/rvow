//! REQ-mreyxw and REQ-wxjrh9: accept stays fast, and the user sees what it runs.
//! `rvow check` warns about a verify that repeats the gate, and `rvow archive`
//! prints each gate check while it runs.

use std::path::{Path, PathBuf};
use std::process::Command;

use rvow_core::change::LEDGER;
use rvow_core::ledger::{self, Kind};
use serde_json::{Map, Value};

struct Repo {
    root: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    project: PathBuf,
}

impl Drop for Repo {
    fn drop(&mut self) {
        rvow_core::git::testrepo::remove_tree(&self.root);
    }
}

fn git_command(repo: &Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(repo)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t");
    c
}

impl Repo {
    /// A git repository with one commit, an rvow project and no change yet.
    fn new(name: &str) -> Repo {
        let root = std::env::temp_dir().join(format!("rvow-accept-speed-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        let root = root.canonicalize().unwrap();
        let mut r = Repo { root: root.clone(), repo: root.join("repo"), home: root.join("home"), project: PathBuf::new() };
        let git = |args: &[&str]| assert!(git_command(&r.repo).args(args).status().unwrap().success(), "git {args:?}");
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(r.repo.join("a.txt"), "a").unwrap();
        git(&["add", "a.txt"]);
        git(&["commit", "-q", "-m", "init"]);
        let out = r.ok(&["init"]);
        r.project = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string());
        r
    }

    fn cmd(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_rvow"));
        c.current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("RVOW_HOME", &self.home)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t");
        c
    }

    /// stdout, stderr and the exit code of one rvow run.
    fn run(&self, args: &[&str]) -> (String, String, i32) {
        let out = self.cmd().args(args).output().unwrap();
        (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap())
    }

    fn ok(&self, args: &[&str]) -> String {
        let (out, err, code) = self.run(args);
        assert_eq!(code, 0, "rvow {args:?}: {out}{err}");
        out
    }

    /// A backlog change of one task with `verify` as its verify command.
    fn plan(&self, name: &str, verify: &str) -> String {
        let out = self.ok(&["new", "--spec", "Out"]);
        let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap().to_string());
        std::fs::write(dir.join("proposal.md"), "branch: main\n\n# Out\n\n## Why\nw\n\n## What changes\nw\n\n## Out of scope\nNone.\n")
            .unwrap();
        std::fs::write(dir.join("tasks.md"), format!("- TSK-new1 {name}\n  covers: REQ-new1\n  verify: {verify}\n  files: a.txt\n"))
            .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN a.txt holds ok\n",
        )
        .unwrap();
        id
    }

    /// The project config file.
    fn config(&self, text: &str) {
        std::fs::write(self.project.join("config.toml"), text).unwrap();
    }

    /// Appends a recorded proof of `cmd` that took `ms`, as an older run would
    /// have, into the ledger of `change`.
    fn record_run(&self, change: &str, cmd: &str, ms: u64) {
        let project = rvow_core::store::find(&self.repo, &self.home).unwrap().unwrap();
        let lock = project.lock().unwrap();
        let (dirs, _) = rvow_core::change::list(&project).unwrap();
        let dir = dirs.iter().find(|d| d.id.to_string() == change).unwrap();
        let mut data = Map::new();
        data.insert("task".into(), Value::String("TSK-old1".into()));
        data.insert("verify".into(), Value::String(cmd.into()));
        data.insert("duration_ms".into(), Value::from(ms));
        data.insert("proved".into(), Value::from(true));
        ledger::append(&dir.path.join(LEDGER), &lock, Kind::Prove, data).unwrap();
    }
}

/// REQ-mreyxw, scenario: chained commands.
#[test]
fn accept_speed_check_warns_when_a_verify_chains_three_commands() {
    let r = Repo::new("chain");
    let id = r.plan("Suite", "cargo fmt --check && cargo clippy && cargo test");
    let out = r.ok(&["check", &id]);
    assert!(out.contains("full-suite"), "{out}");
    assert!(out.contains("TSK-new1"), "{out}");
    assert!(out.contains("warning"), "{out}");
    assert!(out.contains("chains 3 commands"), "{out}");
    assert!(out.contains("checks"), "{out}");
    assert!(out.contains("Call the rvow_accept tool, or ask the user to run /rvow-accept"), "a warning never blocks: {out}");
}

/// REQ-mreyxw, scenario: a configured check.
#[test]
fn accept_speed_check_warns_when_a_verify_repeats_a_configured_check() {
    let r = Repo::new("configured");
    r.config("checks = [\"cargo test -q\"]\n");
    let id = r.plan("Suite", "cargo test -q");
    let out = r.ok(&["check", &id]);
    assert!(out.contains("full-suite"), "{out}");
    assert!(out.contains("TSK-new1"), "{out}");
    assert!(out.contains("repeats a configured `checks` entry"), "{out}");
}

/// REQ-mreyxw, scenario: a slow earlier run.
#[test]
fn accept_speed_check_warns_when_the_last_run_of_the_same_command_was_slow() {
    let r = Repo::new("slow");
    let old = r.plan("Old", "sleep 2");
    r.record_run(&old, "sleep 2", 90_000);
    let id = r.plan("Slow", "sleep 2");
    let out = r.ok(&["check", &id]);
    assert!(out.contains("full-suite"), "{out}");
    assert!(out.contains("TSK-new1"), "{out}");
    assert!(out.contains("90 s"), "{out}");
}

/// An accepted change with one proved task, ready to archive.
fn proved(name: &str, checks: &str) -> (Repo, String, String) {
    let r = Repo::new(name);
    if !checks.is_empty() {
        r.config(checks);
    }
    let id = r.plan("Write ok", "grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'");
    let out = r.ok(&["accept", &id, "--yes"]);
    assert!(out.contains(&format!("accepted {id} at ")), "{out}");
    let dir = r.ok(&["path", &id]);
    let dir = PathBuf::from(dir.lines().find_map(|l| l.strip_prefix("path: ")).unwrap().to_string());
    let tasks = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    let task = tasks.lines().find_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).unwrap();
    std::fs::write(r.repo.join("out.txt"), "ok").unwrap();
    let git = |args: &[&str]| assert!(git_command(&r.repo).args(args).status().unwrap().success(), "git {args:?}");
    git(&["add", "out.txt"]);
    git(&["commit", "-q", "-m", "ok"]);
    let out = r.ok(&["prove", &task]);
    assert!(out.contains("result: proved"), "{out}");
    (r, id, task)
}

/// REQ-wxjrh9: archive prints each gate check on stderr while it runs.
#[test]
fn accept_speed_archive_prints_the_gate_checks_on_stderr() {
    let (r, id, _task) = proved("archive", "checks = [\"sleep 1\"]\n");
    let (out, err, code) = r.run(&["archive", &id]);
    assert_eq!(code, 0, "stdout: {out}\nstderr: {err}");
    assert!(err.contains("check: sleep 1"), "stderr: {err}");
    assert!(err.contains("check done: sleep 1"), "stderr: {err}");
    assert!(err.contains("ms"), "the end line holds the time: {err}");
    assert!(out.contains("archived:"), "stdout stays the same: {out}");
}

/// REQ-wxjrh9: the checks stream live, so a long check shows before it ends.
#[test]
fn accept_speed_archive_prints_a_line_when_each_check_ends() {
    let (r, id, _task) = proved("archive-end", "checks = [\"sleep 1\"]\n");
    let (out, err, code) = r.run(&["archive", &id]);
    assert_eq!(code, 0, "stdout: {out}\nstderr: {err}");
    let start = err.find("check: sleep 1").unwrap();
    let end = err.find("check done: sleep 1").unwrap();
    assert!(start < end, "the end line follows the start line: {err}");
    let time = err.lines().find(|l| l.starts_with("check done: sleep 1")).unwrap();
    let ms: u64 = time.rsplit('(').next().unwrap().split(" ms)").next().unwrap().trim().parse().unwrap();
    assert!(ms >= 900, "sleep 1 took only {ms} ms: {time}");
}

/// REQ-mreyxw, scenario: a targeted test.
#[test]
fn accept_speed_check_leaves_a_targeted_verify_alone() {
    let r = Repo::new("targeted");
    r.config("checks = [\"cargo test -q\"]\n");
    let old = r.plan("Old", "cargo test -q --test cli one_case");
    r.record_run(&old, "cargo test -q --test cli one_case", 500);
    let id = r.plan("Target", "cargo test -q --test cli one_case");
    let out = r.ok(&["check", &id]);
    assert!(!out.contains("full-suite"), "{out}");
    assert!(out.contains("Call the rvow_accept tool, or ask the user to run /rvow-accept"), "{out}");
}

/// REQ-mreyxw, scenario: chained commands.
#[test]
fn full_suite_warns_when_a_verify_chains_three_programs() {
    let r = Repo::new("fs-chain");
    let id = r.plan("Suite", "cargo fmt --check && cargo clippy && cargo test");
    let out = r.ok(&["check", &id]);
    assert!(out.contains("full-suite"), "{out}");
    assert!(out.contains("chains 3 commands"), "{out}");
}

/// REQ-mreyxw, scenario: file checks before one test.
#[test]
fn full_suite_ignores_file_checks_before_one_test() {
    let r = Repo::new("fs-file-checks");
    let id = r.plan("Docs", "grep -q x a.md && grep -q y b.md && cargo test --test main docs::");
    let out = r.ok(&["check", &id]);
    assert!(!out.contains("full-suite"), "{out}");
}

/// REQ-mreyxw: a leading `!` and env assignments do not hide the program.
#[test]
fn full_suite_counts_only_programs_after_a_bang_or_env_assignment() {
    let r = Repo::new("fs-prefix");
    let id = r.plan("Prefix", "! grep -q x a.md && X=1 test -f a.txt && FOO=bar cargo test --test main one");
    let out = r.ok(&["check", &id]);
    assert!(!out.contains("full-suite"), "{out}");
    let id = r.plan("Count", "X=1 cargo fmt && ! cargo clippy && grep -q x a.md && cargo test");
    let out = r.ok(&["check", &id]);
    assert!(out.contains("chains 3 commands"), "{out}");
}

/// Accept runs no verify command and records no baseline: it is the code at accept.
#[test]
fn accept_runs_no_verify_command_and_records_no_baseline() {
    let r = Repo::new("no-run");
    let id = r.plan("One", "true");
    let dir = r.ok(&["path", &id]);
    let dir = PathBuf::from(dir.lines().find_map(|l| l.strip_prefix("path: ")).unwrap().to_string());
    let mut tasks = String::new();
    for (n, extra) in [(1, ""), (2, ""), (3, "  baseline: pass\n")] {
        tasks.push_str(&format!("- TSK-new{n} Task {n}\n  covers: REQ-new1\n  verify: touch m{n} && echo 'test result: ok. 1 passed; 0 failed;'\n  files: a.txt\n{extra}"));
    }
    std::fs::write(dir.join("tasks.md"), tasks).unwrap();
    let (out, err, code) = r.run(&["accept", &id, "--yes"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains(&format!("accepted {id} at ")), "{out}");
    assert!(!format!("{out}{err}").contains("baseline"), "accept says nothing of a baseline: {out}{err}");
    for n in 1..=3 {
        assert!(!r.repo.join(format!("m{n}")).exists(), "accept ran the verify of task {n}");
    }
    let out = r.ok(&["--human", "status"]);
    assert!(!out.contains("baseline"), "{out}");
}
