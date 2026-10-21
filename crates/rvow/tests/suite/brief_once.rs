//! REQ-b7zczh, REQ-m9nj3v, REQ-xexj0f, REQ-4gpzr7 and REQ-xqw93b: the
//! session-start hook sends the same brief once per session, nothing when only
//! the counts changed, only the tail when the next task moved, and the full
//! brief again after a compact or a clear.

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
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Fx {
    /// A repository with one accepted change of two missing tasks, TSK-0ne111 then TSK-tw0222.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-brief-once-{}-{name}", std::process::id()));
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
        assert!(self.command().arg("--version").output().is_ok());
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

    /// Runs the session-start hook with `extra` merged into the input and returns the context, or `None` for no output.
    fn start(&self, extra: Value) -> Option<String> {
        let mut input = json!({"cwd": self.repo()});
        for (k, v) in extra.as_object().unwrap() {
            input[k] = v.clone();
        }
        let mut child = self
            .command()
            .args(["hook", "claude", "session-start"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "hook session-start failed");
        let text = String::from_utf8(out.stdout).unwrap();
        if text.trim().is_empty() {
            return None;
        }
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["hookSpecificOutput"]["hookEventName"], "SessionStart");
        Some(value["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_string())
    }
}

fn session(id: &str) -> Value {
    json!({"session_id": id, "source": "startup"})
}

#[test]
fn brief_once_same_brief_again_prints_no_context() {
    let fx = Fx::new("same");
    let first = fx.start(session("s1")).expect("the first run sends the brief");
    assert!(first.contains("next: TSK-0ne111"), "{first}");
    assert_eq!(fx.start(session("s1")), None, "the same brief is sent once");
}

#[test]
fn brief_once_other_session_gets_the_full_brief() {
    let fx = Fx::new("other");
    fx.start(session("s1")).unwrap();
    let other = fx.start(session("s2")).expect("a new session gets the brief");
    assert!(other.contains("next: TSK-0ne111"), "{other}");
}

#[test]
fn brief_once_no_session_id_prints_the_full_brief_every_time() {
    let fx = Fx::new("nosession");
    for _ in 0..2 {
        let text = fx.start(json!({})).expect("no session id sends the brief");
        assert!(text.contains("next: TSK-0ne111"), "{text}");
    }
}

#[test]
fn brief_once_only_the_counts_changed_sends_nothing() {
    let fx = Fx::new("counts");
    let first = fx.start(session("s1")).unwrap();
    assert!(first.contains("tasks: 2 missing"), "{first}");
    std::fs::write(fx.repo().join("two.txt"), "ok").unwrap();
    fx.rvow(&["prove", "TSK-tw0222"]);
    assert_eq!(fx.start(session("s1")), None, "a count change sends no context");
    let other = fx.start(session("s2")).expect("a new session gets the brief");
    assert!(other.contains("tasks: 1 proved, 1 missing"), "{other}");
    assert!(other.contains("next: TSK-0ne111"), "{other}");
}

#[test]
fn brief_once_a_moved_next_task_sends_only_the_tail() {
    let fx = Fx::new("next");
    let first = fx.start(session("s1")).unwrap();
    assert!(first.contains("next: TSK-0ne111"), "{first}");
    std::fs::write(fx.repo().join("one.txt"), "ok").unwrap();
    fx.rvow(&["prove", "TSK-0ne111"]);
    let text = fx.start(session("s1")).expect("the next task moved");
    assert!(text.starts_with("next: TSK-tw0222"), "{text}");
    assert!(!text.lines().any(|l| l.starts_with("tasks:")), "{text}");
    assert!(!text.lines().any(|l| l.starts_with("CHG-")), "{text}");
}

#[test]
fn brief_once_a_crlf_note_keeps_the_tail_at_next() {
    let fx = Fx::new("crlf");
    fx.rvow(&["note", "decision", "first \u{e9}\r\nnext: not the task\r\nthird"]);
    let first = fx.start(session("s1")).unwrap();
    assert!(first.contains("next: TSK-0ne111"), "{first}");
    std::fs::write(fx.repo().join("one.txt"), "ok").unwrap();
    fx.rvow(&["prove", "TSK-0ne111"]);
    let text = fx.start(session("s1")).expect("the next task moved");
    assert!(text.starts_with("next: TSK-tw0222"), "{text}");
    assert!(!text.contains("decision:"), "{text}");
}

#[test]
fn brief_once_a_decision_sends_the_full_brief() {
    let fx = Fx::new("decision");
    let first = fx.start(session("s1")).unwrap();
    assert!(!first.contains("decision:"), "{first}");
    fx.rvow(&["note", "decision", "write two first"]);
    let text = fx.start(session("s1")).expect("a decision changed the brief");
    assert!(text.contains("decision: write two first"), "{text}");
    assert!(text.contains("tasks: 2 missing"), "{text}");
    assert!(text.contains("next: TSK-0ne111"), "{text}");
}

#[test]
fn brief_once_compact_sends_the_full_brief_again() {
    let fx = Fx::new("compact");
    fx.start(session("s1")).unwrap();
    let text = fx.start(json!({"session_id": "s1", "source": "compact"})).expect("a compact forgets the record");
    assert!(text.contains("next: TSK-0ne111"), "{text}");
}

#[test]
fn brief_once_clear_sends_the_full_brief_again() {
    let fx = Fx::new("clear");
    fx.start(session("s1")).unwrap();
    let text = fx.start(json!({"session_id": "s1", "source": "clear"})).expect("a clear forgets the record");
    assert!(text.contains("next: TSK-0ne111"), "{text}");
    assert_eq!(fx.start(session("s1")), None, "the record is kept again after the clear");
}

#[test]
fn brief_once_a_stale_skill_warning_goes_once_per_session() {
    let fx = Fx::new("stale-skill");
    rvow_core::install::skill(&fx.root.join("claude"), "---\nname: rvow\ndescription: a\n---\nold text\n").unwrap();
    let first = fx.start(session("s1")).expect("the first start warns");
    assert!(first.contains("run rvow install") && first.contains("next: TSK-0ne111"), "{first}");
    assert_eq!(fx.start(session("s1")), None, "the warning is not sent with each prompt");
    let outside = json!({"cwd": fx.root.join("home"), "session_id": "s2", "source": "startup"});
    assert!(fx.start(outside.clone()).expect("outside a project the warning goes too").contains("run rvow install"));
    assert_eq!(fx.start(outside), None, "outside a project the warning also goes once");
}
