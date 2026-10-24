//! REQ-m3hqr1: the session-start hook names the leased task as the next one of a lease holder.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");

struct Fx {
    root: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        rvow_core::git::testrepo::remove_tree(&self.root);
    }
}

impl Fx {
    /// A repository with one accepted change of two missing tasks, TSK-0ne111 then TSK-tw0222.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-lease-brief-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let fx = Fx { root: root.canonicalize().unwrap() };
        fx.git(&["init", "-q", "-b", "main"]);
        std::fs::write(fx.repo().join("a.txt"), "a").unwrap();
        fx.git(&["add", "a.txt"]);
        fx.git(&["commit", "-q", "-m", "init"]);
        fx.rvow(&["init"]);
        let out = fx.rvow(&["new", "--spec", "Out"]);
        let field = |k: &str| out.lines().find_map(|l| l.strip_prefix(&format!("{k}: "))).unwrap().to_string();
        let (id, dir) = (field("created"), PathBuf::from(field("path")));
        std::fs::write(
            dir.join("tasks.md"),
            "- TSK-0ne111 Write one\n  covers: REQ-new1\n  verify: test -f one.txt\n  files: one.txt\n\
             - TSK-tw0222 Write two\n  covers: REQ-new1\n  verify: test -f two.txt\n  files: two.txt\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN the files hold ok\n",
        )
        .unwrap();
        fx.rvow(&["accept", &id, "--yes"]);
        fx
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    fn command(&self) -> Command {
        let mut c = Command::new(RVOW);
        c.current_dir(self.repo())
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.root.join("rh"))
            .env("CLAUDE_CONFIG_DIR", self.root.join("claude"))
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("PI_CODING_AGENT_DIR")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t");
        c
    }

    fn git(&self, args: &[&str]) {
        let ok = Command::new("git")
            .args(args)
            .current_dir(self.repo())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    fn rvow(&self, args: &[&str]) -> String {
        let out = self.command().args(args).output().unwrap();
        assert!(out.status.success(), "rvow {args:?}: {}", String::from_utf8_lossy(&out.stdout));
        String::from_utf8(out.stdout).unwrap()
    }

    /// Runs a hook event with `payload` and returns its JSON, or `None` when it printed nothing.
    fn event(&self, event: &str, mut payload: Value) -> Option<Value> {
        payload["cwd"] = self.repo().to_str().unwrap().into();
        let mut child = self
            .command()
            .args(["hook", "claude", event])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "hook {event} failed");
        let text = String::from_utf8(out.stdout).unwrap();
        (!text.trim().is_empty()).then(|| serde_json::from_str(&text).unwrap())
    }

    /// Binds `agent` to the lease of `task` with the lease-bind hook event, as the mod does.
    fn bind(&self, agent: &str, task: &str) {
        let brief = self.rvow(&["delegate", task]);
        let out = self.event("lease-bind", json!({"agent_id": agent, "prompt": format!("{brief}\nStart now.")})).unwrap();
        assert_eq!(out["bound"], true, "{out}");
    }

    /// Runs the session-start hook with `extra` merged into the input and returns the context, or `None` for no output.
    fn start(&self, extra: Value) -> Option<String> {
        let mut input = json!({"cwd": self.repo()});
        for (k, v) in extra.as_object().unwrap() {
            input[k] = v.clone();
        }
        let out = self.event("session-start", input)?;
        assert_eq!(out["hookSpecificOutput"]["hookEventName"], "SessionStart");
        Some(out["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_string())
    }
}

fn session(id: &str, agent: &str) -> Value {
    json!({"session_id": id, "source": "startup", "agent_id": agent})
}

#[test]
fn lease_brief_a_leased_session_gets_its_own_task_as_next() {
    let fx = Fx::new("own-task");
    fx.bind("p1", "TSK-tw0222");
    let text = fx.start(session("s1", "p1")).expect("a session gets the brief");
    assert!(text.contains("next: TSK-tw0222 Write two"), "{text}");
    assert!(!text.contains("next: TSK-0ne111"), "{text}");
    let loop_line = text.lines().find(|l| l.starts_with("loop:")).expect("the loop line stays");
    assert!(loop_line.contains("vow prove TSK-tw0222"), "{text}");
}

#[test]
fn lease_brief_another_session_proving_another_task_sends_nothing() {
    let fx = Fx::new("another");
    fx.bind("p1", "TSK-tw0222");
    let first = fx.start(session("s1", "p1")).expect("a session gets the brief");
    assert!(first.contains("next: TSK-tw0222"), "{first}");
    std::fs::write(fx.repo().join("one.txt"), "ok").unwrap();
    fx.rvow(&["prove", "TSK-0ne111"]);
    assert_eq!(fx.start(session("s1", "p1")), None, "the leased next task did not move");
}

#[test]
fn lease_brief_delegate_help_says_not_to_repeat_the_task() {
    let fx = Fx::new("repeat-task");
    let out = fx.rvow(&["delegate", "TSK-0ne111"]);
    assert!(out.contains("do not repeat them"), "{out}");
}

#[test]
fn lease_brief_an_ended_lease_names_the_first_open_task() {
    let fx = Fx::new("ended");
    fx.bind("p1", "TSK-tw0222");
    std::fs::write(fx.repo().join("two.txt"), "ok").unwrap();
    fx.rvow(&["prove", "TSK-tw0222"]);
    let text = fx.start(session("s2", "p1")).expect("a new session gets the brief");
    assert!(text.contains("next: TSK-0ne111 Write one"), "{text}");
    assert!(!text.contains("next: TSK-tw0222"), "{text}");
}
