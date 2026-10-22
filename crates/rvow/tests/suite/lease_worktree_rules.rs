//! A session in the worktree of a live lease is judged by the change of that
//! lease, whatever its branch is called (REQ-96cyzx).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rvow_core::lease::{self, Place};
use rvow_core::store::{self, Project};
use serde_json::{Value, json};

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");
const PASS: &str = "echo 'test result: ok. 1 passed; 0 failed;'";

struct Fx {
    root: PathBuf,
    repo: PathBuf,
    wt: PathBuf,
    change: String,
    task: String,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = Command::new("git").args(["worktree", "prune"]).current_dir(&self.repo).output();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn field<'a>(out: &'a str, key: &str) -> &'a str {
    out.lines().find_map(|l| l.strip_prefix(&format!("{key}: "))).unwrap_or_else(|| panic!("no {key} in {out}"))
}

impl Fx {
    /// A repository on `main` with a worktree `../w1` on branch `rmaestro/b`
    /// and an accepted one-task change on `main`.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-lease-rules-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo/src")).unwrap();
        std::fs::create_dir_all(root.join("repo/docs")).unwrap();
        let root = root.canonicalize().unwrap();
        let mut fx = Fx { repo: root.join("repo"), wt: root.join("w1"), root, change: String::new(), task: String::new() };
        let repo = fx.repo.clone();
        std::fs::write(repo.join("src/a.txt"), "a").unwrap();
        std::fs::write(repo.join("docs/a.md"), "a").unwrap();
        fx.git(&repo, &["init", "-q", "-b", "main"]);
        fx.git(&repo, &["add", "."]);
        fx.git(&repo, &["commit", "-q", "-m", "init"]);
        fx.git(&repo, &["worktree", "add", "-q", "-b", "rmaestro/b", "../w1"]);

        fx.rvow(&repo, &["init"]);
        let out = fx.rvow(&repo, &["new", "--spec", "Out"]);
        let dir = PathBuf::from(field(&out, "path"));
        std::fs::write(
            dir.join("tasks.md"),
            format!("- TSK-new1 Code\n  covers: REQ-new1\n  verify: test -f code.txt && {PASS}\n  files: src/**\n"),
        )
        .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 Out\nrvow SHALL write code.\n### Scenario: s\n- WHEN it runs\n- THEN code.txt holds ok\n",
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
        c.current_dir(dir)
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.root.join("home"))
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env_remove("RVOW_GIT")
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

    fn git(&self, dir: &Path, args: &[&str]) {
        let mut c = Command::new("git");
        self.env(&mut c, dir);
        let out = c.args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn rvow(&self, dir: &Path, args: &[&str]) -> String {
        let mut c = Command::new(RVOW);
        self.env(&mut c, dir);
        let out = c.args(args).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn project(&self) -> Project {
        store::find(&self.repo, &self.home()).unwrap().unwrap()
    }

    /// The JSON one hook call printed, or `None` when it printed nothing.
    fn event(&self, dir: &Path, name: &str, mut payload: Value) -> Option<Value> {
        payload["cwd"] = json!(dir);
        let mut c = Command::new(RVOW);
        self.env(&mut c, dir);
        let mut child = c.args(["hook", "claude", name]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
        (!out.trim().is_empty()).then(|| serde_json::from_str(&out).unwrap())
    }

    /// The brief the session-start hook prints for `session` in `dir`.
    fn brief(&self, dir: &Path, session: &str) -> String {
        let out = self.event(dir, "session-start", json!({"session_id": session})).expect("a session start prints a brief");
        out["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_owned()
    }

    /// The deny reason of an edit of `file` from `session`, or `None`.
    fn edit(&self, dir: &Path, file: &str, session: &str) -> Option<String> {
        let payload = json!({"session_id": session, "tool_name": "Write", "tool_input": {"file_path": dir.join(file)}});
        let mut answer = self.event(dir, "pre-tool-use", payload.clone());
        if answer.as_ref().is_some_and(|v| v.to_string().contains("has no baseline yet")) {
            answer = self.event(dir, "pre-tool-use", payload);
        }
        answer.map(|v| v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().to_owned())
    }

    /// A lease of the task, built in `w1` on branch `rmaestro/b`.
    fn lease(&self) -> lease::Lease {
        let place = Place { worktree: self.wt.clone(), branch: "rmaestro/b".to_string() };
        lease::issue_at(&self.project(), &self.home(), &self.repo, &self.task, Some(place)).unwrap().lease
    }
}

#[test]
fn lease_worktree_rules_the_brief_of_a_leased_worktree_names_the_change() {
    let fx = Fx::new("brief");
    fx.lease();
    let text = fx.brief(&fx.wt, "s1");
    assert!(text.contains(&fx.change), "{text}");
    assert!(text.contains("next:"), "{text}");
}

#[test]
fn lease_worktree_rules_a_stop_in_a_leased_worktree_names_the_leased_task() {
    let fx = Fx::new("stop");
    fx.lease();
    assert!(lease::bind(&fx.project(), &fx.task, "s1").unwrap());
    assert_eq!(fx.edit(&fx.wt, "src/a.txt", "s1"), None, "src/a.txt is the task's file");
    std::fs::write(fx.wt.join("src/a.txt"), "b").unwrap();
    let held = fx.event(&fx.wt, "stop", json!({"session_id": "s1"})).expect("the task has no proof");
    let reason = held["reason"].as_str().unwrap();
    assert!(reason.contains(&fx.task) && reason.contains("rvow prove"), "{reason}");
}

#[test]
fn lease_worktree_rules_a_worktree_without_a_lease_has_no_change() {
    let fx = Fx::new("none");
    let text = fx.brief(&fx.wt, "s1");
    assert!(text.contains("no plan"), "{text}");
    assert!(!text.contains(&fx.change), "{text}");
}
