//! A hook waits a bounded time for the project lock: a writing call is denied
//! as busy after 5 s, a read goes on, and a short hold only waits.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rvow_core::store;
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
    /// A repository with an rvow store and no change.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-gate-lock-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let fx = Fx { root: root.canonicalize().unwrap() };
        std::fs::write(fx.repo().join("a.txt"), "a").unwrap();
        for args in [&["init", "-q", "-b", "main"][..], &["add", "a.txt"], &["commit", "-q", "-m", "init"]] {
            assert!(fx.cmd("git").args(args).status().unwrap().success(), "git {args:?}");
        }
        assert!(fx.cmd(RVOW).arg("init").output().unwrap().status.success());
        fx
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    fn project(&self) -> store::Project {
        store::find(&self.repo(), &self.root.join("rh")).unwrap().unwrap()
    }

    fn cmd(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.current_dir(self.repo())
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.root.join("rh"))
            .env_remove("CLAUDE_CONFIG_DIR")
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t");
        c
    }

    /// The deny reason of one tool call, or `None` when it is allowed.
    fn call(&self, tool: &str, input: Value) -> Option<String> {
        let payload = json!({"cwd": self.repo(), "tool_name": tool, "tool_input": input, "session_id": "s1", "tool_use_id": "t1"});
        let mut child =
            self.cmd(RVOW).args(["hook", "claude", "pre-tool-use"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
        (!out.trim().is_empty()).then(|| {
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny", "{out}");
            v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().to_string()
        })
    }

    fn edit(&self) -> Value {
        json!({"file_path": self.repo().join("a.txt"), "old_string": "a", "new_string": "b"})
    }
}

#[test]
fn lock_held_too_long_denies_a_write_as_busy_and_allows_a_read() {
    let fx = Fx::new("held");
    let project = fx.project();
    let _held = project.lock().unwrap();
    let start = Instant::now();
    let reason = fx.call("Edit", fx.edit());
    let took = start.elapsed();
    assert!(reason.as_deref().is_some_and(|r| r.contains("busy")), "{reason:?}");
    assert!(took >= Duration::from_secs(4) && took < Duration::from_secs(8), "waited {took:?}");
    let start = Instant::now();
    assert_eq!(fx.call("Read", json!({"file_path": fx.repo().join("a.txt")})), None);
    assert!(start.elapsed() < Duration::from_secs(4), "a read does not wait for the lock");
}

#[test]
fn lock_held_briefly_only_waits() {
    let fx = Fx::new("brief");
    let project = fx.project();
    let held = project.lock().unwrap();
    let free = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(1));
        drop(held);
    });
    assert_eq!(fx.call("Edit", fx.edit()), None);
    free.join().unwrap();
}
