//! REQ-new3, REQ-new4 and REQ-new5: accept leaves each baseline pending, and
//! the first prove of a task records it.

use std::path::PathBuf;
use std::process::Command;

struct Repo {
    root: PathBuf,
    repo: PathBuf,
    home: PathBuf,
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Repo {
    /// A git repository with one commit and an rvow project.
    fn new(name: &str) -> Repo {
        let root = std::env::temp_dir().join(format!("rvow-baseline-at-prove-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        let root = root.canonicalize().unwrap();
        let r = Repo { root: root.clone(), repo: root.join("repo"), home: root.join("home") };
        r.git(&["init", "-q", "-b", "main"]);
        std::fs::write(r.repo.join("src.txt"), "old\n").unwrap();
        r.git(&["add", "src.txt"]);
        r.git(&["commit", "-q", "-m", "init"]);
        r.ok(&["init"]);
        r
    }

    fn git(&self, args: &[&str]) {
        let ok = Command::new("git")
            .current_dir(&self.repo)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t")
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    /// stdout and the exit code of one rvow run.
    fn rvow(&self, args: &[&str]) -> (String, i32) {
        let out = Command::new(env!("CARGO_BIN_EXE_rvow"))
            .current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("RVOW_HOME", &self.home)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .output()
            .unwrap();
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        (text, out.status.code().unwrap())
    }

    fn ok(&self, args: &[&str]) -> String {
        let (out, code) = self.rvow(args);
        assert_eq!(code, 0, "rvow {args:?}: {out}");
        out
    }

    /// Accepts a change of one task and returns the task id. The task's
    /// scope is `src.txt` and the protected test file `tests/check.rs`.
    fn accepted(&self, verify: &str, extra: &str) -> String {
        let out = self.ok(&["new", "Out"]);
        let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        std::fs::write(dir.join("proposal.md"), "branch: main\n\n# Out\n\n## Why\nw\n\n## What changes\nw\n\n## Out of scope\nNone.\n")
            .unwrap();
        std::fs::write(dir.join("tasks.md"), format!("- TSK-new1 Write ok\n  verify: {verify}\n  files: src.txt, tests/check.rs\n{extra}"))
            .unwrap();
        self.ok(&["accept", &id, "--yes"]);
        let tasks = std::fs::read_to_string(
            self.ok(&["path", &id]).lines().find_map(|l| l.strip_prefix("path: ")).map(PathBuf::from).unwrap().join("tasks.md"),
        )
        .unwrap();
        tasks.lines().find_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).unwrap()
    }

    fn status(&self) -> String {
        self.ok(&["--human", "status"])
    }
}

const CHECK: &str = "grep -q ok src.txt && echo 'test result: ok. 1 passed; 0 failed;'";

#[test]
fn baseline_at_first_prove_records_a_red() {
    let r = Repo::new("red");
    let task = r.accepted(CHECK, "");
    std::fs::create_dir_all(r.repo.join("tests")).unwrap();
    std::fs::write(r.repo.join("tests/check.rs"), "// the new test\n").unwrap();
    let (out, code) = r.rvow(&["prove", &task]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("baseline: recorded"), "{out}");
    assert!(!out.contains("result: proved"), "{out}");
    assert!(!r.status().contains("baseline pending"), "{}", r.status());
    // The work, then the proof.
    std::fs::write(r.repo.join("src.txt"), "ok\n").unwrap();
    let out = r.ok(&["prove", &task]);
    assert!(out.contains("result: proved"), "{out}");
}

#[test]
fn baseline_at_first_prove_later_failure_is_a_failed_proof() {
    let r = Repo::new("later");
    let task = r.accepted(CHECK, "");
    r.ok(&["prove", &task]);
    let (out, code) = r.rvow(&["prove", &task]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("result: \"failed: exit 1\""), "{out}");
    assert!(!out.contains("baseline: recorded"), "{out}");
}

#[test]
fn baseline_at_first_prove_refuses_a_pass() {
    let r = Repo::new("pass");
    let task = r.accepted("echo 'test result: ok. 2 passed; 0 failed;'", "");
    let (out, code) = r.rvow(&["prove", &task]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("code: baseline") && out.contains(&task), "{out}");
    assert!(r.status().contains("baseline pending"), "nothing is recorded: {}", r.status());
}

#[test]
fn baseline_at_first_prove_records_a_passing_guard() {
    let r = Repo::new("guard");
    let task = r.accepted("echo 'test result: ok. 2 passed; 0 failed;'", "  baseline: pass\n");
    let out = r.ok(&["prove", &task]);
    assert!(out.contains("result: proved"), "{out}");
    assert!(!r.status().contains("baseline pending"), "{}", r.status());
}

#[test]
fn baseline_at_first_prove_refuses_a_failing_guard() {
    let r = Repo::new("guard-fails");
    let task = r.accepted("false", "  baseline: pass\n");
    let (out, code) = r.rvow(&["prove", &task]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("code: baseline"), "{out}");
    assert!(r.status().contains("baseline pending"), "{}", r.status());
}

#[test]
fn baseline_on_the_base_commit_runs_when_the_work_came_first() {
    let r = Repo::new("work-first");
    let task = r.accepted(CHECK, "");
    std::fs::write(r.repo.join("src.txt"), "ok\n").unwrap();
    let out = r.ok(&["prove", &task]);
    assert!(out.contains("result: proved"), "{out}");
    assert!(!r.status().contains("baseline pending"), "{}", r.status());
}

#[test]
fn baseline_on_the_base_commit_refuses_a_check_that_passed_there() {
    let r = Repo::new("work-first-pass");
    let task = r.accepted("echo 'test result: ok. 3 passed; 0 failed;'", "");
    std::fs::write(r.repo.join("src.txt"), "ok\n").unwrap();
    let (out, code) = r.rvow(&["prove", &task]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("code: baseline") && out.contains("base commit"), "{out}");
    assert!(r.status().contains("baseline pending"), "{}", r.status());
}

/// Accepts a change of two tasks with the same scope and returns their ids.
fn two_tasks(r: &Repo, second_verify: &str) -> (String, String) {
    let out = r.ok(&["new", "Two"]);
    let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
    let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
    std::fs::write(dir.join("proposal.md"), "branch: main\n\n# Two\n\n## Why\nw\n\n## What changes\nw\n\n## Out of scope\nNone.\n")
        .unwrap();
    std::fs::write(
        dir.join("tasks.md"),
        format!("- TSK-new1 First\n  verify: {CHECK}\n  files: src.txt\n- TSK-new2 Second\n  verify: {second_verify}\n  files: src.txt\n"),
    )
    .unwrap();
    r.ok(&["accept", &id, "--yes"]);
    let tasks = std::fs::read_to_string(
        PathBuf::from(r.ok(&["path", &id]).lines().find_map(|l| l.strip_prefix("path: ")).unwrap()).join("tasks.md"),
    )
    .unwrap();
    let ids: Vec<String> = tasks.lines().filter_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).collect();
    (ids[0].clone(), ids[1].clone())
}

#[test]
fn baseline_on_the_base_commit_when_a_proved_task_holds_the_work() {
    // The work of both tasks came before the first proof, so the proved first
    // task's scope holds the second task's work too: the second task's
    // passing first prove checks the base commit instead of refusing.
    let r = Repo::new("hidden-work");
    let (first, second) = two_tasks(&r, "grep -q ok src.txt && echo 'test result: ok. 2 passed; 0 failed;'");
    std::fs::write(r.repo.join("src.txt"), "ok\n").unwrap();
    assert!(r.ok(&["prove", &first]).contains("result: proved"));
    let out = r.ok(&["prove", &second]);
    assert!(out.contains("result: proved"), "{out}");
    assert!(!r.status().contains("baseline pending"), "{}", r.status());
}

#[test]
fn baseline_at_first_prove_red_keeps_the_task_open_after_the_work_starts() {
    // A red baseline is no proof attempt: once the work changes the scope the
    // task is still missing, not stale, so the stop hook still holds it.
    let r = Repo::new("red-open");
    let task = r.accepted(CHECK, "");
    r.ok(&["prove", &task]);
    std::fs::write(r.repo.join("src.txt"), "half\n").unwrap();
    let out = r.status();
    assert!(out.contains("missing") && !out.contains("stale"), "{out}");
}

/// Accepts a change of one task on `main` and returns the task id.
fn one_task(r: &Repo, title: &str, verify: &str, files: &str) -> String {
    let out = r.ok(&["new", title]);
    let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
    let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
    std::fs::write(
        dir.join("proposal.md"),
        format!("branch: main\n\n# {title}\n\n## Why\nw\n\n## What changes\nw\n\n## Out of scope\nNone.\n"),
    )
    .unwrap();
    std::fs::write(dir.join("tasks.md"), format!("- TSK-new1 {title}\n  verify: {verify}\n  files: {files}\n")).unwrap();
    r.ok(&["accept", &id, "--yes"]);
    let tasks = std::fs::read_to_string(
        PathBuf::from(r.ok(&["path", &id]).lines().find_map(|l| l.strip_prefix("path: ")).unwrap()).join("tasks.md"),
    )
    .unwrap();
    tasks.lines().find_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).unwrap()
}

#[test]
fn baseline_shared_scope_work_of_another_changes_proved_task_is_not_this_tasks() {
    // Change A proved its work in src.txt. Change B shares the file, so the
    // first prove of B runs on the live tree, where its check fails, and not on
    // the base commit, where it passes.
    let r = Repo::new("shared-scope");
    let a = one_task(&r, "First", CHECK, "src.txt");
    r.ok(&["prove", &a]);
    std::fs::write(r.repo.join("src.txt"), "ok\n").unwrap();
    assert!(r.ok(&["prove", &a]).contains("result: proved"));
    let b = one_task(&r, "Second", "grep -q old src.txt && echo 'test result: ok. 1 passed; 0 failed;'", "src.txt");
    let (out, code) = r.rvow(&["prove", &b]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("baseline: recorded"), "{out}");
}
