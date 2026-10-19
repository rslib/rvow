//! A shell command that writes a file follows the same plan and lease rules
//! as an Edit of that file.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use rvow_core::{lease, store};
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
    /// A repository with a plan of two tasks, one on `src/a.rs` and one on `src/b.rs`, not accepted yet.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-gate-writes-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo/src")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let mut fx = Fx { root: root.canonicalize().unwrap(), id: String::new() };
        std::fs::write(fx.repo().join("src/a.rs"), "x").unwrap();
        std::fs::write(fx.repo().join("src/b.rs"), "x").unwrap();
        fx.run("git", &["init", "-q", "-b", "main"]);
        fx.run("git", &["add", "."]);
        fx.run("git", &["commit", "-q", "-m", "init"]);
        fx.run(RVOW, &["init"]);
        let out = fx.run(RVOW, &["new", "--spec", "Out"]);
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        fx.id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        let ok = "verify: grep -q y src/a.rs && echo 'test result: ok. 1 passed; 0 failed;'";
        std::fs::write(
            dir.join("tasks.md"),
            format!(
                "- TSK-new1 Write a\n  covers: REQ-new1\n  {ok}\n  files: src/a.rs\n\n- TSK-new2 Write b\n  covers: REQ-new1\n  {ok}\n  files: src/b.rs\n"
            ),
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

    fn home(&self) -> PathBuf {
        self.root.join("rh")
    }

    fn cmd(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.current_dir(self.repo())
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.home())
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

    /// The deny reason of one Bash call, or `None` when it is allowed.
    fn bash(&self, command: &str, agent: Option<&str>) -> Option<String> {
        let mut payload = json!({"cwd": self.repo(), "tool_name": "Bash", "tool_input": {"command": command}});
        if let Some(agent) = agent {
            payload["agent_id"] = json!(agent);
        }
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

    fn tasks(&self) -> Vec<String> {
        let located = self.run(RVOW, &["path", &self.id]);
        let dir = PathBuf::from(located.lines().find_map(|l| l.strip_prefix("path: ")).unwrap().trim());
        let text = std::fs::read_to_string(dir.join("tasks.md")).unwrap();
        text.lines().filter_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).collect()
    }
}

#[test]
fn shell_writes_before_accept_are_denied() {
    let fx = Fx::new("plan");
    for command in ["echo y > src/a.rs", "sed -i.bak s/x/y/ src/a.rs", "tee src/b.rs < /dev/null", "cp src/a.rs src/c.rs"] {
        let reason = fx.bash(command, None);
        assert!(reason.as_deref().is_some_and(|r| r.contains("not accepted")), "{command}: {reason:?}");
    }
    for command in [
        "cat src/a.rs",
        "cargo build",
        "ls src",
        "echo y > /tmp/rvow-gate-writes-elsewhere.txt",
        "git commit -m 'bump v1.2'",
        "git add src/a.rs && git commit -m 'fix: update src/a.rs'",
        "cargo test --manifest-path Cargo.toml",
        "python3 gen.py",
        "mkdir tmp.d",
        "rvow put CHG-x proposal < notes.md",
    ] {
        assert_eq!(fx.bash(command, None), None, "{command}");
    }
    fx.run(RVOW, &["yolo", "audit", "--for", "30m"]);
    assert_eq!(fx.bash("echo y > src/a.rs", None), None, "yolo audit lets it through");
    assert!(fx.run(RVOW, &["stats"]).contains("unaccepted-plan,1"), "the audit is counted");
}

#[test]
fn shell_writes_of_a_leased_subagent_stay_in_its_lease() {
    let fx = Fx::new("lease");
    fx.run(RVOW, &["accept", &fx.id, "--yes"]);
    let tasks = fx.tasks();
    let project = store::find(&fx.repo(), &fx.home()).unwrap().unwrap();
    fx.run(RVOW, &["delegate", &tasks[0]]);
    assert!(lease::bind(&project, &tasks[0], "a1").unwrap());
    let reason = fx.bash("sed -i.bak s/x/y/ src/b.rs", Some("a1"));
    assert!(reason.as_deref().is_some_and(|r| r.contains("outside the files of")), "{reason:?}");
    assert_eq!(fx.bash("echo y > src/a.rs", Some("a1")), None);
    assert_eq!(fx.bash("echo y > src/b.rs", None), None, "the main session has no lease");
}
