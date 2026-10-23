//! A worktree lease stays live while its worktree exists and its task is open
//! (REQ-3sdvwa): no time limit, and gone once its folder is gone.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rvow_core::lease::{self, LEASE_FILE, Place};
use rvow_core::store::{self, Project};
use serde_json::{Value, json};

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");
const HOUR: u64 = 3_600_000;
const PASS: &str = "echo 'test result: ok. 1 passed; 0 failed;'";
const SPEC: &str = "## ADD REQ-new1 Two\nrvow SHALL do both.\n### Scenario: s\n- WHEN it runs\n- THEN both are done\n";

struct Fx {
    root: PathBuf,
    repo: PathBuf,
    wt: PathBuf,
    code: String,
    docs: String,
}

impl Drop for Fx {
    fn drop(&mut self) {
        // The worktree may be gone; a second removal is not an error.
        let _ = Command::new("git").args(["worktree", "prune"]).current_dir(&self.repo).output();
        rvow_core::git::testrepo::remove_tree(&self.root);
    }
}

fn field<'a>(out: &'a str, key: &str) -> &'a str {
    out.lines().find_map(|l| l.strip_prefix(&format!("{key}: "))).unwrap_or_else(|| panic!("no {key} in {out}"))
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}

impl Fx {
    /// A repository on `main` with a worktree `../wt` on branch `B` and an
    /// accepted change of two tasks: `src/**` and `docs/**`.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-lease-worktree-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo/src")).unwrap();
        std::fs::create_dir_all(root.join("repo/docs")).unwrap();
        let root = root.canonicalize().unwrap();
        let mut fx = Fx { repo: root.join("repo"), wt: root.join("wt"), root, code: String::new(), docs: String::new() };
        let repo = fx.repo.clone();
        std::fs::write(repo.join("src/a.txt"), "a").unwrap();
        std::fs::write(repo.join("docs/a.md"), "a").unwrap();
        fx.git(&repo, &["init", "-q", "-b", "main"]);
        fx.git(&repo, &["add", "."]);
        fx.git(&repo, &["commit", "-q", "-m", "init"]);
        fx.git(&repo, &["worktree", "add", "-q", "-b", "B", "../wt"]);

        fx.rvow(&repo, &["init"]);
        let out = fx.rvow(&repo, &["new", "--spec", "Two"]);
        let dir = PathBuf::from(field(&out, "path"));
        std::fs::write(
            dir.join("tasks.md"),
            format!(
                "- TSK-new1 Code\n  covers: REQ-new1\n  verify: test -f code.txt && {PASS}\n  files: src/**, code.txt\n\n- TSK-new2 Docs\n  covers: REQ-new1\n  verify: test -f docs.txt && {PASS}\n  files: docs/**, docs.txt\n"
            ),
        )
        .unwrap();
        std::fs::write(dir.join("specs/out.md"), SPEC).unwrap();
        fx.rvow(&repo, &["accept", field(&out, "created"), "--yes"]);
        let located = fx.rvow(&repo, &["path", field(&out, "created")]);
        let tasks = std::fs::read_to_string(PathBuf::from(field(&located, "path")).join("tasks.md")).unwrap();
        let ids: Vec<String> = tasks.lines().filter_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).collect();
        fx.code = ids[0].clone();
        fx.docs = ids[1].clone();
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

    fn project(&self) -> Project {
        store::find(&self.repo, &self.root.join("home")).unwrap().unwrap()
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
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

    /// The deny reason of an edit of `file` from `session` (and `agent` when
    /// it has one), or `None` when the edit is allowed.
    fn edit(&self, dir: &Path, file: &str, session: &str, agent: Option<&str>) -> Option<String> {
        let mut payload = json!({"session_id": session, "tool_name": "Write", "tool_input": {"file_path": dir.join(file)}});
        if let Some(a) = agent {
            payload["agent_id"] = a.into();
        }
        let mut answer = self.event(dir, "pre-tool-use", payload.clone());
        // The first source edit may be denied once for the baseline.
        if answer.as_ref().is_some_and(|v| v.to_string().contains("has no baseline yet")) {
            answer = self.event(dir, "pre-tool-use", payload);
        }
        answer.map(|v| v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().to_owned())
    }

    /// Moves every issue line of the lease store `age` into the past.
    fn age(&self, age: u64) {
        let path = self.project().root.join(LEASE_FILE);
        let text = std::fs::read_to_string(&path).unwrap();
        let mut aged = String::new();
        for line in text.lines() {
            let mut event: Value = serde_json::from_str(line).unwrap();
            if event["event"] == "issue" {
                event["time"] = json!(now() - age);
            }
            aged.push_str(&event.to_string());
            aged.push('\n');
        }
        std::fs::write(&path, aged).unwrap();
    }
}

#[test]
fn lease_worktree_an_old_worktree_lease_still_scopes_its_agent() {
    let fx = Fx::new("old");
    let (project, home) = (fx.project(), fx.home());
    let place = Place { worktree: fx.wt.clone(), branch: "B".to_string() };
    let issued = lease::issue_at(&project, &home, &fx.repo, &fx.code, Some(place)).unwrap();
    assert_eq!(issued.lease.worktree.as_deref(), Some(fx.wt.as_path()));
    fx.age(5 * HOUR);

    let bound =
        fx.event(&fx.repo, "lease-bind", json!({"agent_id": "a1", "session_id": "s1", "prompt": format!("lease: {}", fx.code)})).unwrap();
    assert_eq!(bound["bound"], true, "{bound}");
    let lease = lease::leases(&project).unwrap().into_iter().find(|l| l.task == fx.code).unwrap();
    assert!(lease.issued < now() - 4 * HOUR, "the lease is five hours old");
    assert!(lease.live(now()), "a lease with a worktree has no time limit");
    assert_eq!(lease::holder(&project, &home, &fx.repo, "a1", now()).unwrap().map(|l| l.task), Some(fx.code.clone()));

    let reason = fx.edit(&fx.wt, "docs/a.md", "s1", Some("a1")).expect("docs/a.md is outside the leased task");
    assert!(reason.contains(&fx.code) && reason.contains("docs/a.md"), "{reason}");
    assert_eq!(fx.edit(&fx.wt, "src/a.txt", "s1", Some("a1")), None, "src/a.txt is the leased task's");
}

#[test]
fn lease_worktree_a_removed_worktree_hands_the_task_back_to_the_main_session() {
    let fx = Fx::new("gone");
    let (project, home) = (fx.project(), fx.home());
    let place = Place { worktree: fx.wt.clone(), branch: "B".to_string() };
    lease::issue_at(&project, &home, &fx.repo, &fx.code, Some(place)).unwrap();
    assert!(lease::bind(&project, &fx.code, "s1").unwrap());

    fx.git(&fx.repo, &["worktree", "remove", "--force", &fx.wt.to_string_lossy()]);
    assert!(!fx.wt.is_dir(), "the worktree folder is gone");
    let lease = lease::leases(&project).unwrap().into_iter().find(|l| l.task == fx.code).unwrap();
    assert!(!lease.live(now()), "a lease whose worktree is gone is not live");

    assert_eq!(fx.edit(&fx.repo, "src/a.txt", "m1", None), None, "the main session edits the task's file");
    let held = fx.event(&fx.repo, "stop", json!({"session_id": "m1"})).expect("the task has no proof");
    let reason = held["reason"].as_str().unwrap();
    assert!(reason.contains(&fx.code) && reason.contains("rvow prove"), "{reason}");
}

#[test]
fn lease_worktree_a_lease_without_a_worktree_ends_after_four_hours() {
    let fx = Fx::new("ttl");
    let (project, home) = (fx.project(), fx.home());
    lease::issue(&project, &home, &fx.repo, &fx.code).unwrap();
    assert!(lease::bind(&project, &fx.code, "a1").unwrap());
    assert!(lease::holder(&project, &home, &fx.repo, "a1", now()).unwrap().is_some(), "fresh");
    fx.age(5 * HOUR);
    let lease = lease::leases(&project).unwrap().into_iter().find(|l| l.task == fx.code).unwrap();
    assert!(lease.worktree.is_none() && !lease.live(now()), "a lease with no worktree ends after four hours");
    assert!(lease::holder(&project, &home, &fx.repo, "a1", now()).unwrap().is_none());
}
