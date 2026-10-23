//! Tool calls that once got past the hook: MCP arguments that skip the git
//! hooks, and the Monitor tool, which runs a shell command.

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
    /// A repository with an rvow store and no change.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-gate-tools-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let fx = Fx { root: root.canonicalize().unwrap() };
        fx.run("git", &["init", "-q", "-b", "main"]);
        std::fs::write(fx.repo().join("a.txt"), "a").unwrap();
        fx.run("git", &["add", "a.txt"]);
        fx.run("git", &["commit", "-q", "-m", "init"]);
        fx.run(RVOW, &["init"]);
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

    fn run(&self, program: &str, args: &[&str]) {
        let out = self.cmd(program).args(args).output().unwrap();
        assert!(out.status.success(), "{program} {args:?}: {}", String::from_utf8_lossy(&out.stdout));
    }

    /// The deny reason of one tool call, or `None` when it is allowed.
    fn call(&self, tool: &str, input: Value) -> Option<String> {
        let payload = json!({"cwd": self.repo(), "tool_name": tool, "tool_input": input});
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
}

#[test]
fn mcp_arguments_that_skip_the_git_hooks_are_denied() {
    let fx = Fx::new("mcp");
    let commit = fx.call("mcp__rgit__git_commit", json!({"message": "x", "no_verify": true}));
    assert!(commit.as_deref().is_some_and(|r| r.contains("no_verify")), "{commit:?}");
    let config = fx.call("mcp__rgit__git_config_set", json!({"key": "core.hooksPath", "value": "/dev/null"}));
    assert!(config.as_deref().is_some_and(|r| r.contains("core.hooksPath")), "{config:?}");
    let nested = fx.call("mcp__git__run", json!({"options": {"noVerify": true}}));
    assert!(nested.is_some(), "a nested noVerify is denied");
    assert_eq!(fx.call("mcp__rgit__git_commit", json!({"message": "x", "no_verify": false})), None);
    assert_eq!(fx.call("mcp__rgit__git_commit", json!({"message": "explain core.hooksPath in the docs"})), None);
}

#[test]
fn monitor_commands_are_judged_as_bash() {
    let fx = Fx::new("monitor");
    let accept = fx.call("Monitor", json!({"command": "rvow accept CHG-x --yes", "description": "d", "timeout_ms": 1000}));
    assert!(accept.as_deref().is_some_and(|r| r.contains("rvow accept")), "{accept:?}");
    let push = fx.call("Monitor", json!({"command": "git push --no-verify", "description": "d", "timeout_ms": 1000}));
    assert!(push.is_some(), "a Monitor push that skips the hooks is denied");
    assert_eq!(fx.call("Monitor", json!({"command": "tail -f build.log", "description": "d", "timeout_ms": 1000})), None);
    assert_eq!(fx.call("Monitor", json!({"ws": {"url": "wss://example.com/s"}, "description": "d", "timeout_ms": 1000})), None);
}
