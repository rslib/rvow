//! A hook that cannot read the store denies the tools that write, so a broken
//! store does not turn every check off, and still lets the reading tools run.

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
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-gate-failure-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("rh")).unwrap();
        let fx = Fx { root: root.canonicalize().unwrap() };
        let out = fx.cmd("git").args(["init", "-q", "-b", "main"]).output().unwrap();
        assert!(out.status.success());
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
            .env("GIT_CONFIG_NOSYSTEM", "1");
        c
    }

    /// The decision and reason of one tool call from `cwd`, or `None` when it is allowed.
    fn call(&self, cwd: &std::path::Path, tool: &str, input: Value) -> Option<(String, String)> {
        let payload = json!({"cwd": cwd, "tool_name": tool, "tool_input": input});
        let mut child =
            self.cmd(RVOW).args(["hook", "claude", "pre-tool-use"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "a hook exits 0");
        let out = String::from_utf8(out.stdout).unwrap();
        (!out.trim().is_empty()).then(|| {
            let v: Value = serde_json::from_str(&out).unwrap();
            let d = &v["hookSpecificOutput"];
            (d["permissionDecision"].as_str().unwrap().to_string(), d["permissionDecisionReason"].as_str().unwrap().to_string())
        })
    }
}

#[test]
fn failure_a_broken_store_denies_writes_and_allows_reads() {
    let fx = Fx::new("broken");
    std::fs::create_dir_all(fx.repo().join("rvow")).unwrap();
    std::fs::write(fx.repo().join("rvow/config.toml"), "").unwrap();
    let file = fx.repo().join("a.txt");
    let edit = fx.call(&fx.repo(), "Edit", json!({"file_path": file, "old_string": "a", "new_string": "b"}));
    let (decision, reason) = edit.expect("a broken store denies an edit");
    assert_eq!(decision, "deny");
    assert!(reason.contains("project.toml") && reason.contains("rvow install"), "{reason}");
    assert!(fx.call(&fx.repo(), "Bash", json!({"command": "touch b.txt"})).is_some(), "a shell call is denied too");
    assert!(fx.call(&fx.repo(), "mcp__fs__write_file", json!({"path": "c.txt"})).is_some(), "a writing MCP tool is denied too");
    assert_eq!(fx.call(&fx.repo(), "Read", json!({"file_path": file})), None);
    assert_eq!(fx.call(&fx.repo(), "Grep", json!({"pattern": "a"})), None);
}

#[test]
fn failure_a_repository_or_folder_with_no_store_allows_writes() {
    let fx = Fx::new("none");
    let edit = json!({"file_path": fx.repo().join("a.txt"), "old_string": "a", "new_string": "b"});
    assert_eq!(fx.call(&fx.repo(), "Edit", edit.clone()), None, "a repository with no commit");
    std::fs::write(fx.repo().join("a.txt"), "a").unwrap();
    let commit = fx.cmd("git").args(["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "init"]).output();
    assert!(commit.unwrap().status.success());
    assert_eq!(fx.call(&fx.repo(), "Edit", edit), None, "a repository with no store");
    let plain = fx.root.join("home");
    assert_eq!(fx.call(&plain, "Edit", json!({"file_path": plain.join("a.txt")})), None, "a folder outside git");
}
