//! One hook call asks git each question once, and the shell-write rule
//! counts a target only when its path is known.

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

impl Fx {
    /// A repository with a plan of one task on `src/a.rs`, not accepted yet.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-hook-calls-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo/src")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let mut fx = Fx { root: root.canonicalize().unwrap(), id: String::new() };
        std::fs::write(fx.repo().join("src/a.rs"), "x").unwrap();
        fx.run("git", &["init", "-q", "-b", "main"]);
        fx.run("git", &["add", "."]);
        fx.run("git", &["commit", "-q", "-m", "init"]);
        fx.run(RVOW, &["init"]);
        fx.id = fx.plan("Out");
        fx
    }

    /// A new plan of one task on `src/a.rs`; returns its id.
    fn plan(&self, title: &str) -> String {
        let out = self.run(RVOW, &["new", "--spec", title]);
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
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
        out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string()
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

    /// The deny reason of one pre-tool-use call, or `None` when it is allowed.
    fn call(&self, tool: &str, input: Value, git: Option<&PathBuf>) -> Option<String> {
        let payload = json!({"cwd": self.repo(), "tool_name": tool, "tool_input": input, "tool_use_id": "t1", "session_id": "s1"});
        let mut c = self.cmd(RVOW);
        if let Some(git) = git {
            c.env("RVOW_GIT", git);
        }
        let mut child = c.args(["hook", "claude", "pre-tool-use"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
        (!out.trim().is_empty()).then(|| {
            let v: Value = serde_json::from_str(&out).unwrap();
            v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().to_string()
        })
    }

    fn bash(&self, command: &str) -> Option<String> {
        self.call("Bash", json!({"command": command}), None)
    }

    /// A git that logs each call it gets to `log`, then runs the real git.
    fn logging_git(&self, log: &std::path::Path) -> PathBuf {
        let git = self.root.join("git-log.sh");
        std::fs::write(&git, format!("#!/bin/sh\necho \"$*\" >> '{}'\nexec /usr/bin/git \"$@\"\n", log.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
        git
    }
}

#[test]
fn once_an_edit_check_asks_git_no_question_twice() {
    let fx = Fx::new("once");
    fx.run(RVOW, &["accept", &fx.id, "--yes"]);
    let log = fx.root.join("git-calls.txt");
    let git = fx.logging_git(&log);
    fx.call("Edit", json!({"file_path": fx.repo().join("src/a.rs"), "old_string": "x", "new_string": "y"}), Some(&git));
    let calls = std::fs::read_to_string(&log).unwrap();
    let rev_parse: Vec<&str> = calls.lines().filter(|l| l.contains("rev-parse")).collect();
    assert!(!rev_parse.is_empty(), "the hook asks git through RVOW_GIT:\n{calls}");
    let mut seen = std::collections::BTreeSet::new();
    for line in &rev_parse {
        assert!(seen.insert(*line), "asked twice: {line}\nall calls:\n{calls}");
    }
}

#[test]
fn once_a_branch_switch_between_calls_is_seen() {
    let fx = Fx::new("switch");
    fx.run(RVOW, &["accept", &fx.id, "--yes"]);
    let edit = json!({"file_path": fx.repo().join("src/a.rs"), "old_string": "x", "new_string": "y"});
    assert!(fx.call("Edit", edit.clone(), None).is_none_or(|r| !r.contains("not accepted")), "the plan of main is accepted");
    fx.run("git", &["checkout", "-q", "-b", "b"]);
    fx.plan("On b");
    let reason = fx.call("Edit", edit, None);
    assert!(reason.as_deref().is_some_and(|r| r.contains("not accepted")), "the edit is judged by the plan of b: {reason:?}");
}

#[test]
fn targets_hold_a_variable_only_until_it_is_known() {
    let fx = Fx::new("vars");
    assert_eq!(fx.bash("S=/tmp/rvow-hook-calls-x; cat > $S/a.md"), None);
    assert_eq!(fx.bash("echo y > $NOT_SET_ANYWHERE_RVOW/a.rs"), None);
    let reason = fx.bash("D=src; echo y > $D/a.rs");
    assert!(reason.as_deref().is_some_and(|r| r.contains("not accepted")), "{reason:?}");
    let reason = fx.bash("D=src; sed -i.bak s/x/y/ $D/a.rs");
    assert!(reason.as_deref().is_some_and(|r| r.contains("not accepted")), "{reason:?}");
}

#[test]
fn targets_in_heredoc_text_do_not_count() {
    let fx = Fx::new("heredoc");
    assert_eq!(fx.bash("cat > /tmp/rvow-hook-calls-x.md <<'EOF'\necho y > src/a.rs\nEOF"), None);
    let reason = fx.bash("cat > src/a.rs <<'EOF'\ny\nEOF");
    assert!(reason.as_deref().is_some_and(|r| r.contains("not accepted")), "{reason:?}");
}
