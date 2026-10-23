//! A hook call from the worktree of a live lease binds its top-level session
//! to that lease (REQ-0papbp).

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
    code: String,
    docs: String,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = Command::new("git").args(["worktree", "prune"]).current_dir(&self.repo).output();
        rvow_core::git::testrepo::remove_tree(&self.root);
    }
}

fn field<'a>(out: &'a str, key: &str) -> &'a str {
    out.lines().find_map(|l| l.strip_prefix(&format!("{key}: "))).unwrap_or_else(|| panic!("no {key} in {out}"))
}

impl Fx {
    /// A repository on `main` with a worktree `../w1` on branch `rmaestro/b`
    /// and an accepted change of two tasks: `src/**` and `docs/**`.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-lease-bind-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo/src")).unwrap();
        std::fs::create_dir_all(root.join("repo/docs")).unwrap();
        let root = root.canonicalize().unwrap();
        let mut fx = Fx { repo: root.join("repo"), wt: root.join("w1"), root, code: String::new(), docs: String::new() };
        let repo = fx.repo.clone();
        std::fs::write(repo.join("src/a.txt"), "a").unwrap();
        std::fs::write(repo.join("docs/a.md"), "a").unwrap();
        fx.git(&repo, &["init", "-q", "-b", "main"]);
        fx.git(&repo, &["add", "."]);
        fx.git(&repo, &["commit", "-q", "-m", "init"]);
        fx.git(&repo, &["worktree", "add", "-q", "-b", "rmaestro/b", "../w1"]);

        fx.rvow(&repo, &["init"]);
        let out = fx.rvow(&repo, &["new", "--spec", "Two"]);
        let dir = PathBuf::from(field(&out, "path"));
        std::fs::write(
            dir.join("tasks.md"),
            format!(
                "- TSK-new1 Code\n  covers: REQ-new1\n  verify: test -f code.txt && {PASS}\n  files: src/**\n\n- TSK-new2 Docs\n  covers: REQ-new1\n  verify: test -f docs.txt && {PASS}\n  files: docs/**\n"
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 Two\nrvow SHALL do both.\n### Scenario: s\n- WHEN it runs\n- THEN both are done\n",
        )
        .unwrap();
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

    /// The deny reason of an edit of `file` from `session` in `dir`, or `None`.
    fn edit(&self, dir: &Path, file: &str, session: &str) -> Option<String> {
        let payload = json!({"session_id": session, "tool_name": "Write", "tool_input": {"file_path": dir.join(file)}});
        let mut answer = self.event(dir, "pre-tool-use", payload.clone());
        if answer.as_ref().is_some_and(|v| v.to_string().contains("has no baseline yet")) {
            answer = self.event(dir, "pre-tool-use", payload);
        }
        answer.map(|v| v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().to_owned())
    }

    /// A lease of `code` in `w1` on branch `rmaestro/b`, held by nobody.
    fn lease(&self) {
        let place = Place { worktree: self.wt.clone(), branch: "rmaestro/b".to_string() };
        lease::issue_at(&self.project(), &self.home(), &self.repo, &self.code, Some(place)).unwrap();
    }

    fn holder(&self) -> Option<String> {
        lease::leases(&self.project()).unwrap().into_iter().find(|l| l.task == self.code).and_then(|l| l.agent_id)
    }
}

#[test]
fn lease_bind_worktree_the_first_call_of_a_session_binds_it() {
    let fx = Fx::new("first");
    fx.lease();
    assert_eq!(fx.holder(), None, "the lease starts unheld");
    assert_eq!(fx.edit(&fx.wt, "src/a.txt", "s1"), None, "src/a.txt is the task's file");
    assert_eq!(fx.holder().as_deref(), Some("s1"), "the call from its worktree bound the session");
}

#[test]
fn lease_bind_worktree_a_bound_session_is_denied_outside_its_task() {
    let fx = Fx::new("outside");
    fx.lease();
    assert_eq!(fx.edit(&fx.wt, "src/a.txt", "s1"), None);
    let reason = fx.edit(&fx.wt, "docs/a.md", "s1").expect("docs/a.md belongs to the other task");
    assert!(reason.contains(&fx.code) && reason.contains("docs/a.md"), "{reason}");
}

#[test]
fn lease_bind_worktree_a_second_session_is_not_bound() {
    let fx = Fx::new("second");
    fx.lease();
    assert_eq!(fx.edit(&fx.wt, "src/a.txt", "s1"), None);
    assert_eq!(fx.holder().as_deref(), Some("s1"));
    assert_eq!(fx.edit(&fx.wt, "docs/a.md", "s2"), None, "s2 holds no lease");
    assert_eq!(fx.holder().as_deref(), Some("s1"), "the lease stays with s1");
    assert_ne!(fx.docs, fx.code);
}

#[test]
fn lease_bind_worktree_a_bound_session_is_denied_outside_its_worktree() {
    let fx = Fx::new("jail");
    fx.lease();
    assert_eq!(fx.edit(&fx.wt, "src/a.txt", "s1"), None);
    let main = fx.repo.join("src/a.txt");
    let reason = fx.edit(&fx.wt, &main.to_string_lossy(), "s1").expect("the main checkout is outside the worktree");
    assert!(reason.contains("outside") && reason.contains(&fx.code), "{reason}");
    let scratch = std::env::temp_dir().join(format!("rvow-jail-{}.txt", std::process::id()));
    assert_eq!(fx.edit(&fx.wt, &scratch.to_string_lossy(), "s1"), None, "a scratch file in the temp folder is allowed");
}

#[test]
fn lease_bind_worktree_a_subagent_of_a_bound_session_meets_its_lease() {
    let fx = Fx::new("subagent");
    fx.lease();
    assert_eq!(fx.edit(&fx.wt, "src/a.txt", "s1"), None);
    let payload = json!({"session_id": "s1", "agent_id": "a1", "tool_name": "Write", "tool_input": {"file_path": fx.wt.join("docs/a.md")}});
    let out = fx.event(&fx.wt, "pre-tool-use", payload).expect("the subagent is denied");
    let reason = out["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
    assert!(reason.contains(&fx.code) && reason.contains("docs/a.md"), "{reason}");
}

#[test]
fn lease_bind_worktree_an_incoming_lease_holder_is_ignored() {
    let fx = Fx::new("forged");
    let payload =
        json!({"session_id": "s9", "lease_holder": "s1", "tool_name": "Write", "tool_input": {"file_path": fx.wt.join("docs/a.md")}});
    fx.lease();
    assert_eq!(fx.edit(&fx.wt, "src/a.txt", "s1"), None);
    let out = fx.event(&fx.repo, "pre-tool-use", payload);
    assert!(out.as_ref().is_none_or(|v| !v.to_string().contains("outside the files")), "a forged holder is not applied: {out:?}");
}
