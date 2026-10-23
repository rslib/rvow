//! The Stop hook reminds about the newest accepted change only; older ones wait for the pre-push gate.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");
const PASS: &str = "echo 'test result: ok. 1 passed; 0 failed;'";

struct Fx {
    root: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        rvow_core::git::testrepo::remove_tree(&self.root);
    }
}

impl Fx {
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-stop-scope-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        let fx = Fx { root: root.canonicalize().unwrap() };
        for args in [vec!["init", "-q", "-b", "main"], vec!["commit", "-q", "--allow-empty", "-m", "init"]] {
            assert!(fx.cmd("git").args(&args).status().unwrap().success(), "git {args:?}");
        }
        fx.rvow(&["init"]);
        fx
    }

    fn cmd(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.current_dir(self.root.join("repo"))
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.root.join("rh"))
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t");
        c
    }

    fn rvow(&self, args: &[&str]) -> String {
        String::from_utf8_lossy(&self.cmd(RVOW).args(args).output().unwrap().stdout).into_owned()
    }

    /// Accepts a change whose one task runs `verify`, and returns the task id.
    fn change(&self, title: &str, verify: &str) -> String {
        let mut ids = self.change_with(title, &[("One", verify, "**")]);
        ids.pop().expect("the change has one task")
    }

    /// Accepts a change with one task per (title, verify) pair, each writing `files`, and returns the ids.
    fn change_with(&self, title: &str, tasks: &[(&str, &str, &str)]) -> Vec<String> {
        let out = self.rvow(&["new", "--spec", title]);
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        let body: String = tasks
            .iter()
            .enumerate()
            .map(|(i, (t, verify, files))| format!("- TSK-new{i} {t}\n  covers: REQ-new1\n  verify: {verify}\n  files: {files}\n"))
            .collect();
        std::fs::write(dir.join("tasks.md"), body).unwrap();
        std::fs::write(
            dir.join("specs/x.md"),
            format!("## ADD REQ-new1 {title}\nrvow SHALL do it.\n### Scenario: s\n- WHEN it runs\n- THEN it is done\n"),
        )
        .unwrap();
        let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        self.rvow(&["accept", &id, "--yes"]);
        let located = self.rvow(&["path", &id]);
        let tasks =
            std::fs::read_to_string(PathBuf::from(located.lines().find_map(|l| l.strip_prefix("path: ")).unwrap().trim()).join("tasks.md"))
                .unwrap();
        tasks.lines().filter_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).collect()
    }

    /// Commits `rel` with `text`, so the file is in the protected set at accept.
    fn commit_file(&self, rel: &str, text: &str) {
        let at = self.root.join("repo").join(rel);
        if let Some(dir) = at.parent() {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(&at, text).unwrap();
        assert!(self.cmd("git").args(["add", rel]).status().unwrap().success());
        assert!(self.cmd("git").args(["commit", "-q", "-m", rel]).status().unwrap().success());
    }

    /// An allowed Write call of session `s1` on `rel`, which puts the file in the session's record.
    fn edit(&self, rel: &str) {
        let input = json!({"file_path": self.root.join("repo").join(rel)});
        let payload = json!({"cwd": self.root.join("repo"), "tool_name": "Write", "tool_input": input, "session_id": "s1"});
        let send = || {
            let mut child =
                self.cmd(RVOW).args(["hook", "claude", "pre-tool-use"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
            child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
            String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
        };
        let mut out = send();
        // The first source edit is denied once for the baseline, and an agent retries it.
        if out.contains("has no baseline yet") {
            out = send();
        }
        assert_eq!(out.trim(), "", "the edit of {rel} was held back");
    }

    fn stop(&self) -> Option<String> {
        let mut child = self.cmd(RVOW).args(["hook", "claude", "stop"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let payload = json!({"cwd": self.root.join("repo"), "session_id": "s1"});
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
        (!out.trim().is_empty()).then(|| serde_json::from_str::<Value>(&out).unwrap()["reason"].as_str().unwrap().to_string())
    }
}

#[test]
fn stop_an_older_change_does_not_hold_the_agent() {
    let fx = Fx::new("older");
    let old = fx.change("Old work", "false");
    fx.rvow(&["prove", &old]);
    std::thread::sleep(std::time::Duration::from_millis(20));
    let new = fx.change("New work", &format!("test -f b.txt && {PASS}"));
    std::fs::write(fx.root.join("repo/b.txt"), "x").unwrap();
    fx.edit("b.txt");
    fx.rvow(&["prove", &new]);
    assert_eq!(fx.stop(), None, "the older change has a failed task, but the newest change is done");
}

#[test]
fn stop_the_newest_change_still_holds_the_agent() {
    let fx = Fx::new("newest");
    let old = fx.change("Old work", &format!("test -f a.txt && {PASS}"));
    std::fs::write(fx.root.join("repo/a.txt"), "x").unwrap();
    fx.rvow(&["prove", &old]);
    std::thread::sleep(std::time::Duration::from_millis(20));
    let new = fx.change("New work", "false");
    fx.edit("a.txt");
    fx.rvow(&["prove", &new]);
    let reason = fx.stop().expect("the newest change has a failed task");
    assert!(reason.contains(&new) && !reason.contains(&old), "{reason}");
}

#[test]
fn stop_scope_three_open_items_come_in_one_block() {
    let fx = Fx::new("three");
    fx.commit_file("tests/t.sh", "exit 1\n");
    let ids = fx.change_with("Both work", &[("Write a", "test -f a.txt", "**"), ("Write b", "test -f b.txt", "**")]);
    std::fs::write(fx.root.join("repo/tests/t.sh"), "exit 0\n").unwrap();
    fx.edit("tests/t.sh");
    fx.edit("a.txt");
    fx.edit("b.txt");
    let reason = fx.stop().expect("three open items hold the stop");
    assert!(reason.starts_with("rvow: 3 open items: "), "{reason}");
    assert!(reason.contains("protected file tests/t.sh changed"), "{reason}");
    assert!(reason.contains(&ids[0]) && reason.contains(&ids[1]), "{reason}");
}

#[test]
fn stop_scope_five_open_items_name_three_and_count_the_rest() {
    let fx = Fx::new("five");
    let files: Vec<String> = ["a", "b", "c", "d", "e"].iter().map(|f| format!("{f}.txt")).collect();
    let tasks: Vec<(&str, &str, &str)> = files.iter().map(|f| ("Write a file", "test -f one.txt", f.as_str())).collect();
    let ids = fx.change_with("Five tasks", &tasks);
    for f in &files {
        fx.edit(f);
    }
    let reason = fx.stop().expect("five open items hold the stop");
    assert!(reason.starts_with("rvow: 5 open items: "), "{reason}");
    for id in &ids[..3] {
        assert!(reason.contains(id.as_str()), "{id} is named: {reason}");
    }
    for id in &ids[3..] {
        assert!(!reason.contains(id.as_str()), "{id} is counted: {reason}");
    }
    assert!(reason.trim_end().ends_with("and 2 more"), "{reason}");
}

#[test]
fn stop_scope_the_same_open_items_again_let_the_stop_pass() {
    let fx = Fx::new("same");
    let ids = fx.change_with("Both work", &[("Write a", "test -f a.txt", "**"), ("Write b", "test -f b.txt", "**")]);
    fx.edit("a.txt");
    fx.edit("b.txt");
    let reason = fx.stop().expect("two open items hold the stop");
    assert!(reason.starts_with("rvow: 2 open items: "), "{reason}");
    assert!(reason.contains(&ids[0]) && reason.contains(&ids[1]), "{reason}");
    assert_eq!(fx.stop(), None, "the same items let the stop pass");
}
