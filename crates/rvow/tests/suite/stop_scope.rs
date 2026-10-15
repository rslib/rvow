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
        let _ = std::fs::remove_dir_all(&self.root);
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
        let out = self.rvow(&["new", "--spec", title]);
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        std::fs::write(dir.join("tasks.md"), format!("- TSK-new1 {title}\n  covers: REQ-new1\n  verify: {verify}\n  files: **\n")).unwrap();
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
        tasks.lines().find_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).unwrap()
    }

    /// An allowed Write call of session `s1` on `rel`, which puts the file in the session's record.
    fn edit(&self, rel: &str) {
        let input = json!({"file_path": self.root.join("repo").join(rel)});
        let payload = json!({"cwd": self.root.join("repo"), "tool_name": "Write", "tool_input": input, "session_id": "s1"});
        let mut child =
            self.cmd(RVOW).args(["hook", "claude", "pre-tool-use"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
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
