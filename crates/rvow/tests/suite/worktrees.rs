//! Plan actions run in the worktree of the change branch (CHG-wvxkxy). Each
//! test drives the built binary in a throwaway repository with a real
//! `git worktree add`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rvow_core::lease;
use rvow_core::store;
use rvow_hook::hook::{self, Rule};
use serde_json::{Value, json};

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");
const TASKS: &str = "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n";
const SPEC: &str = "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n";

struct Fx {
    root: PathBuf,
    repo: PathBuf,
    wt: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        rvow_core::git::testrepo::remove_tree(&self.root);
    }
}

fn field<'a>(out: &'a str, key: &str) -> &'a str {
    out.lines().find_map(|l| l.strip_prefix(&format!("{key}: "))).unwrap_or_else(|| panic!("no {key} in {out}"))
}

impl Fx {
    /// A repository on `main` with a worktree `../wt` on branch `B`.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-worktrees-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        let root = root.canonicalize().unwrap();
        let fx = Fx { repo: root.join("repo"), wt: root.join("wt"), root };
        fx.git(&fx.repo, &["init", "-q", "-b", "main"]);
        std::fs::write(fx.repo.join("a.txt"), "a").unwrap();
        fx.git(&fx.repo, &["add", "a.txt"]);
        fx.git(&fx.repo, &["commit", "-q", "-m", "init"]);
        fx.git(&fx.repo, &["worktree", "add", "-q", "-b", "B", "../wt"]);
        fx
    }

    fn env(&self, c: &mut Command, dir: &Path) {
        c.current_dir(dir)
            .env("HOME", &self.root)
            .env("RVOW_HOME", self.root.join("home"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t");
    }

    fn git(&self, dir: &Path, args: &[&str]) {
        let mut c = Command::new("git");
        self.env(&mut c, dir);
        let out = c.args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// Runs rvow in `dir`: stdout and exit code.
    fn run(&self, dir: &Path, args: &[&str]) -> (String, i32) {
        let mut c = Command::new(RVOW);
        self.env(&mut c, dir);
        let out = c.args(args).output().unwrap();
        (String::from_utf8(out.stdout).unwrap(), out.status.code().unwrap())
    }

    fn ok(&self, dir: &Path, args: &[&str]) -> String {
        let (out, code) = self.run(dir, args);
        assert_eq!(code, 0, "rvow {args:?}: {out}");
        out
    }

    /// A backlog change of the branch `dir` is on, with one task.
    fn plan(&self, dir: &Path) -> (String, PathBuf) {
        let out = self.ok(dir, &["new", "--spec", "Out"]);
        let id = field(&out, "created").to_owned();
        let path = PathBuf::from(field(&out, "path"));
        std::fs::write(path.join("tasks.md"), TASKS).unwrap();
        std::fs::write(path.join("specs/out.md"), SPEC).unwrap();
        (id, path)
    }

    /// A change of branch B in `../wt`, planned from `../wt` and accepted from `from`.
    fn accepted_from(&self, from: &Path) -> (String, PathBuf) {
        self.ok(&self.repo, &["init"]);
        let (id, _) = self.plan(&self.wt);
        self.ok(from, &["accept", &id, "--yes"]);
        let dir = PathBuf::from(field(&self.ok(&self.wt, &["path", &id]), "path"));
        (id, dir)
    }
}

fn task_of(dir: &Path) -> String {
    let tasks = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
    tasks.lines().find_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).unwrap()
}

#[test]
fn worktree_plan_accept_runs_in_the_branch_worktree() {
    let fx = Fx::new("accept");
    fx.ok(&fx.repo, &["init"]);
    let (id, _) = fx.plan(&fx.wt);
    let out = fx.ok(&fx.repo, &["accept", &id, "--yes"]);
    assert!(out.contains(&format!("accepted {id} at ")), "{out}");
}

#[test]
fn worktree_plan_accept_names_both_fixes_when_no_worktree_has_the_branch() {
    let fx = Fx::new("nobranch");
    fx.ok(&fx.repo, &["init"]);
    fx.git(&fx.repo, &["checkout", "-q", "-b", "C"]);
    let (id, _) = fx.plan(&fx.repo);
    fx.git(&fx.repo, &["checkout", "-q", "main"]);
    let (out, code) = fx.run(&fx.repo, &["accept", &id, "--yes"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("\"C\""), "{out}");
    assert!(out.contains("git checkout C"), "{out}");
    assert!(out.contains("git worktree add <path> C"), "{out}");
}

#[test]
fn worktree_plan_archive_runs_in_the_branch_worktree() {
    let fx = Fx::new("archive");
    let (id, dir) = fx.accepted_from(&fx.wt);
    std::fs::write(fx.wt.join("out.txt"), "ok").unwrap();
    fx.git(&fx.wt, &["add", "out.txt"]);
    fx.git(&fx.wt, &["commit", "-q", "-m", "ok"]);
    let out = fx.ok(&fx.wt, &["prove", &task_of(&dir)]);
    assert!(out.contains("result: proved"), "{out}");
    let out = fx.ok(&fx.repo, &["archive", &id]);
    assert!(out.contains(&format!("archived: {id}")), "{out}");
}

#[test]
fn worktree_plan_confirm_decides_an_amendment_made_from_the_main_checkout() {
    let fx = Fx::new("confirm");
    let (id, dir) = fx.accepted_from(&fx.wt);
    std::fs::write(dir.join("tasks.md"), TASKS.replace("Write ok", "Write more")).unwrap();
    let amended = fx.ok(&fx.repo, &["amend", "--reason", "retitle", "--change", &id]);
    let amd = amended
        .split_whitespace()
        .find(|w| w.starts_with("AMD-"))
        .unwrap()
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '-')
        .to_owned();
    let out = fx.ok(&fx.repo, &["confirm", &amd, "--accept"]);
    assert!(out.contains(&format!("{amd}: accepted")), "{out}");
}

#[test]
fn worktree_check_reads_the_files_of_the_change_worktree() {
    let fx = Fx::new("check");
    fx.ok(&fx.repo, &["init"]);
    let (id, dir) = fx.plan(&fx.wt);
    std::fs::write(fx.wt.join("prove.sh"), "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::write(dir.join("tasks.md"), TASKS.replace("grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'", "./prove.sh"))
        .unwrap();
    let (from_wt, code_wt) = fx.run(&fx.wt, &["check", &id]);
    let (from_main, code_main) = fx.run(&fx.repo, &["check", &id]);
    assert!(!from_wt.contains("missing-program"), "{from_wt}");
    assert!(!from_main.contains("missing-program"), "{from_main}");
    assert_eq!(from_main, from_wt, "the main checkout must read the worktree of branch B");
    assert_eq!(code_main, code_wt);
}

#[test]
fn worktree_check_clarify_lists_the_questions_of_the_change_worktree() {
    let fx = Fx::new("check-clarify");
    fx.ok(&fx.repo, &["init"]);
    let (id, dir) = fx.plan(&fx.wt);
    let proposal = std::fs::read_to_string(dir.join("proposal.md")).unwrap();
    std::fs::write(dir.join("proposal.md"), format!("{proposal}\n[NEEDS CLARIFICATION: Which shell?]\n")).unwrap();
    let out = fx.ok(&fx.repo, &["clarify", &id]);
    assert!(out.contains("Which shell?"), "{out}");
}

#[test]
fn worktree_plan_clarify_lists_the_questions_of_a_worktree_change() {
    let fx = Fx::new("clarify");
    fx.ok(&fx.repo, &["init"]);
    let (id, dir) = fx.plan(&fx.wt);
    let proposal = std::fs::read_to_string(dir.join("proposal.md")).unwrap();
    std::fs::write(dir.join("proposal.md"), format!("{proposal}\n[NEEDS CLARIFICATION: Which shell?]\n")).unwrap();
    let out = fx.ok(&fx.repo, &["clarify", &id]);
    assert!(out.contains("Which shell?"), "{out}");
}

impl Fx {
    /// Accepts a change of the branch `dir` is on, with one task per `(file, title)`, and returns the task ids.
    fn change(&self, dir: &Path, tasks: &[(&str, &str)]) -> Vec<String> {
        let out = self.ok(dir, &["new", "--spec", tasks[0].1]);
        let id = field(&out, "created").to_owned();
        let path = PathBuf::from(field(&out, "path"));
        let text: String = tasks
            .iter()
            .enumerate()
            .map(|(i, (file, title))| format!("- TSK-new{i} {title}\n  covers: REQ-new1\n  verify: false\n  files: {file}\n"))
            .collect();
        std::fs::write(path.join("tasks.md"), text).unwrap();
        std::fs::write(path.join("specs/out.md"), SPEC).unwrap();
        self.ok(dir, &["accept", &id, "--yes"]);
        let located = PathBuf::from(field(&self.ok(dir, &["path", &id]), "path"));
        std::fs::read_to_string(located.join("tasks.md"))
            .unwrap()
            .lines()
            .filter_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from))
            .collect()
    }

    /// Change A in the main checkout owns `a.txt`; change B in `../wt` owns `b.txt` and `c.txt`.
    fn two_changes(&self) -> (Vec<String>, Vec<String>) {
        self.ok(&self.repo, &["init"]);
        let a = self.change(&self.repo, &[("a.txt", "Task of A")]);
        let b = self.change(&self.wt, &[("b.txt", "Task of B one"), ("c.txt", "Task of B two")]);
        (a, b)
    }

    fn hook(&self, dir: &Path, event: &str, mut payload: Value) -> Option<Value> {
        payload["cwd"] = json!(dir);
        let mut c = Command::new(RVOW);
        self.env(&mut c, dir);
        let mut child = c.args(["hook", "claude", event]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
        (!out.trim().is_empty()).then(|| serde_json::from_str(&out).unwrap())
    }

    /// The deny reason of an edit of `file` from a session in `dir`, or `None` when it is allowed.
    fn edit(&self, dir: &Path, file: &str, agent: Option<&str>) -> Option<String> {
        let mut payload = json!({"tool_name": "Write", "tool_input": {"file_path": dir.join(file)}, "session_id": "s1"});
        if let Some(a) = agent {
            payload["agent_id"] = a.into();
        }
        let mut answer = self.hook(dir, "pre-tool-use", payload.clone());
        // The first source edit is denied once for the baseline, and an agent retries it.
        if answer.as_ref().is_some_and(|v| v.to_string().contains("has no baseline yet")) {
            answer = self.hook(dir, "pre-tool-use", payload);
        }
        answer.map(|v| v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().to_owned())
    }
}

#[test]
fn worktree_guard_an_edit_is_judged_by_the_change_of_the_worktree() {
    let fx = Fx::new("guard-edit");
    fx.two_changes();
    let project = store::find(&fx.wt, &fx.root.join("home")).unwrap().unwrap();
    let judged = |dir: &Path, file: &str| {
        let call = json!({"cwd": dir, "tool_name": "Write", "tool_input": {"file_path": dir.join(file)}});
        hook::pre_tool_use_denial(&project, &call).unwrap()
    };
    assert!(judged(&fx.wt, "b.txt").is_none(), "b.txt belongs to a task of B");
    let denied = judged(&fx.wt, "a.txt").expect("a.txt belongs to a task of A only");
    assert_eq!(denied.rule, Rule::OutsideScope, "{}", denied.reason);
    assert!(judged(&fx.repo, "a.txt").is_none(), "the main checkout is still judged by A");
    assert!(judged(&fx.repo, "b.txt").is_some(), "b.txt is outside the tasks of A");
}

#[test]
fn worktree_guard_a_stop_names_the_task_of_the_worktree_change() {
    let fx = Fx::new("guard-stop");
    let (a, b) = fx.two_changes();
    assert_eq!(fx.edit(&fx.wt, "b.txt", None), None);
    std::fs::write(fx.wt.join("b.txt"), "x").unwrap();
    let held = fx.hook(&fx.wt, "stop", json!({"session_id": "s1"})).expect("a task of B is unproved");
    let reason = held["reason"].as_str().unwrap();
    assert!(reason.contains(&b[0]), "{reason}");
    assert!(!reason.contains(&a[0]), "{reason}");
}

#[test]
fn worktree_guard_a_lease_delegated_in_the_worktree_judges_by_its_task() {
    let fx = Fx::new("guard-lease");
    let (_a, b) = fx.two_changes();
    fx.ok(&fx.wt, &["delegate", &b[0]]);
    let project = store::find(&fx.wt, &fx.root.join("home")).unwrap().unwrap();
    assert!(lease::bind(&project, &b[0], "a1").unwrap());
    assert_eq!(fx.edit(&fx.wt, "b.txt", Some("a1")), None, "inside the leased task");
    let reason = fx.edit(&fx.wt, "c.txt", Some("a1")).expect("c.txt belongs to the other task of B");
    assert!(reason.contains(&b[0]) && reason.contains("b.txt"), "{reason}");
}
