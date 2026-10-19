//! An accept, confirm or yolo switch made during a session counts only when
//! the harness saw the user make it. The harness says what it saw in the
//! `user_made` field of each hook call; a harness that sends no such field
//! keeps the old behavior.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");

struct Fx {
    root: PathBuf,
    id: String,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}

impl Fx {
    /// A repository with a plan of one task on `src/a.rs`, not accepted yet.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-gate-unseen-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo/src")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let mut fx = Fx { root: root.canonicalize().unwrap(), id: String::new() };
        std::fs::write(fx.repo().join("src/a.rs"), "x").unwrap();
        fx.run("git", &["init", "-q", "-b", "main"]);
        fx.run("git", &["add", "."]);
        fx.run("git", &["commit", "-q", "-m", "init"]);
        fx.run(RVOW, &["init"]);
        let out = fx.run(RVOW, &["new", "--spec", "Out"]);
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        fx.id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        std::fs::write(
            dir.join("tasks.md"),
            "- TSK-new1 Write a\n  covers: REQ-new1\n  verify: grep -q y src/a.rs && echo 'test result: ok. 1 passed; 0 failed;'\n  files: src/a.rs\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 Out\nrvow SHALL write y.\n### Scenario: s\n- WHEN it runs\n- THEN a holds y\n",
        )
        .unwrap();
        fx
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
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

    fn run(&self, program: &str, args: &[&str]) -> String {
        let out = self.cmd(program).args(args).output().unwrap();
        assert!(out.status.success(), "{program} {args:?}: {}", String::from_utf8_lossy(&out.stdout));
        String::from_utf8(out.stdout).unwrap()
    }

    /// A copy of the rvow binary under another name, as an agent could make.
    fn copy(&self) -> String {
        let copy = self.root.join("x");
        std::fs::copy(RVOW, &copy).unwrap();
        copy.display().to_string()
    }

    /// The decision and reason of one tool call, or `None` when it is allowed.
    fn call(&self, tool: &str, input: Value, user_made: Option<Value>) -> Option<(String, String)> {
        let mut payload = json!({"cwd": self.repo(), "tool_name": tool, "tool_input": input});
        if let Some(seen) = user_made {
            payload["user_made"] = seen;
        }
        let mut child =
            self.cmd(RVOW).args(["hook", "claude", "pre-tool-use"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
        (!out.trim().is_empty()).then(|| {
            let v: Value = serde_json::from_str(&out).unwrap();
            let d = &v["hookSpecificOutput"];
            (d["permissionDecision"].as_str().unwrap().to_string(), d["permissionDecisionReason"].as_str().unwrap().to_string())
        })
    }

    fn edit(&self, user_made: Option<Value>) -> Option<(String, String)> {
        self.call("Edit", json!({"file_path": self.repo().join("src/a.rs"), "old_string": "x", "new_string": "y"}), user_made)
    }
}

#[test]
fn unseen_accept_from_a_copied_binary_keeps_edits_denied() {
    let fx = Fx::new("copy");
    let since = now();
    fx.run(&fx.copy(), &["accept", &fx.id, "--yes"]);
    let (decision, reason) = fx.edit(Some(json!({"since": since, "changes": []}))).expect("an unseen accept denies the edit");
    assert_eq!(decision, "deny");
    assert!(reason.contains(&format!("/rvow-accept {}", fx.id)), "{reason}");
    let shell = fx.call("Bash", json!({"command": "echo y > src/a.rs"}), Some(json!({"since": since, "changes": []})));
    assert!(shell.is_some(), "a shell write is denied too");
    let read = fx.call("Read", json!({"file_path": fx.repo().join("src/a.rs")}), Some(json!({"since": since, "changes": []})));
    assert_eq!(read, None, "a read goes on");
    assert_eq!(fx.edit(Some(json!({"since": since, "changes": [fx.id]}))), None, "the user accepted it");
    assert_eq!(fx.edit(None), None, "a harness that sends no record keeps the old behavior");
}

#[test]
fn unseen_accept_from_before_the_session_or_under_a_user_yolo_counts() {
    let fx = Fx::new("before");
    let before = now();
    fx.run(RVOW, &["accept", &fx.id, "--yes"]);
    assert_eq!(fx.edit(Some(json!({"since": now() + 1, "changes": []}))), None, "accepted before the session started");
    let yolo = json!([{"tier": "auto", "at": before, "until": before + 3_600_000}]);
    assert_eq!(fx.edit(Some(json!({"since": before, "changes": [], "yolo": yolo}))), None, "accepted under a yolo auto the user turned on");
    let after = now();
    let run = json!({"since": before, "changes": [], "runs": [{"at": before, "until": after}]});
    assert_eq!(fx.edit(Some(run)), None, "accepted while the harness ran an accept for the user");
    let late = json!({"since": before, "changes": [], "runs": [{"at": after + 1, "until": after + 2}]});
    assert!(fx.edit(Some(late)).is_some(), "a run that does not span the accept does not count");
    let audit = json!([{"tier": "audit", "at": before, "until": before + 3_600_000}]);
    assert!(fx.edit(Some(json!({"since": before, "changes": [], "yolo": audit}))).is_some(), "an audit tier does not accept for the user");
}

#[test]
fn unseen_yolo_from_a_copied_binary_is_off() {
    let fx = Fx::new("yolo");
    fx.run(RVOW, &["accept", &fx.id, "--yes"]);
    let since = now();
    fx.run(&fx.copy(), &["yolo", "auto", "--for", "1h"]);
    let confirm = json!({"command": "rvow confirm --accept --all"});
    let seen = |yolo: Value| Some(json!({"since": since, "changes": [], "yolo": yolo}));
    let denied = fx.call("Bash", confirm.clone(), seen(json!([])));
    assert!(denied.is_some_and(|(d, r)| d == "deny" && r.contains("rvow confirm")), "a yolo the user did not turn on is off");
    let at = now();
    assert_eq!(fx.call("Bash", confirm.clone(), seen(json!([{"tier": "auto", "at": since, "until": at + 3_600_000}]))), None);
    let short = seen(json!([{"tier": "auto", "at": since, "until": at + 60_000}]));
    assert!(fx.call("Bash", confirm.clone(), short).is_some(), "a yolo that runs past the user's end is off");
    assert_eq!(fx.call("Bash", confirm, None), None, "a harness that sends no record keeps the old behavior");
}
