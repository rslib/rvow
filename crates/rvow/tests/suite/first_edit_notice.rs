//! The first source edit of a baseline-pending task is denied once per session, with a reason that names `rvow prove`.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

struct Repo {
    root: PathBuf,
    repo: PathBuf,
    home: PathBuf,
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

const CHECK: &str = "grep -q ok src/a.rs && echo 'test result: ok. 1 passed; 0 failed;'";

impl Repo {
    /// A git repository with one commit and an rvow project.
    fn new(name: &str) -> Repo {
        let root = std::env::temp_dir().join(format!("rvow-first-edit-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo/src")).unwrap();
        let root = root.canonicalize().unwrap();
        let r = Repo { root: root.clone(), repo: root.join("repo"), home: root.join("home") };
        r.git(&["init", "-q", "-b", "main"]);
        std::fs::write(r.repo.join("src/a.rs"), "old\n").unwrap();
        r.git(&["add", "src/a.rs"]);
        r.git(&["commit", "-q", "-m", "init"]);
        r.ok(&["init"]);
        r
    }

    fn git(&self, args: &[&str]) {
        let ok = Command::new("git")
            .current_dir(&self.repo)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t")
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_rvow"));
        c.current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("RVOW_HOME", &self.home)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1");
        c
    }

    fn rvow(&self, args: &[&str]) -> (String, i32) {
        let out = self.command().args(args).output().unwrap();
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        (text, out.status.code().unwrap())
    }

    fn ok(&self, args: &[&str]) -> String {
        let (out, code) = self.rvow(args);
        assert_eq!(code, 0, "rvow {args:?}: {out}");
        out
    }

    /// Accepts a change of one task with `files: src/a.rs, tests/a.rs` and returns the task id.
    fn accepted(&self) -> String {
        let out = self.ok(&["new", "Out"]);
        let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        std::fs::write(dir.join("proposal.md"), "branch: main\n\n# Out\n\n## Why\nw\n\n## What changes\nw\n\n## Out of scope\nNone.\n")
            .unwrap();
        std::fs::write(dir.join("tasks.md"), format!("- TSK-new1 Write ok\n  verify: {CHECK}\n  files: src/a.rs, tests/a.rs\n")).unwrap();
        self.ok(&["accept", &id, "--yes"]);
        let tasks = std::fs::read_to_string(
            self.ok(&["path", &id]).lines().find_map(|l| l.strip_prefix("path: ")).map(PathBuf::from).unwrap().join("tasks.md"),
        )
        .unwrap();
        tasks.lines().find_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).unwrap()
    }

    /// What `rvow hook claude <event>` prints for an `Edit` of `file`, as JSON, or `Null` for nothing.
    fn hook(&self, event: &str, session: &str, file: &str) -> Value {
        let input = json!({
            "cwd": self.repo.to_str().unwrap(),
            "session_id": session,
            "tool_name": "Edit",
            "tool_input": {"file_path": self.repo.join(file).to_str().unwrap()},
            "tool_use_id": format!("use-{file}"),
        });
        self.send(event, &input)
    }

    /// The answer of `rvow hook claude <event>` to the raw `input`.
    fn send(&self, event: &str, input: &Value) -> Value {
        let mut child = self
            .command()
            .args(["hook", "claude", event])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        if text.trim().is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}")) }
    }

    /// The pre-tool-use answer for an `Edit` of `file`.
    fn pre(&self, session: &str, file: &str) -> Value {
        self.hook("pre-tool-use", session, file)
    }
}

/// The deny reason of a pre-tool-use answer, or none when the call is allowed.
fn denial(out: &Value) -> Option<&str> {
    let hook = &out["hookSpecificOutput"];
    if hook["permissionDecision"] == "deny" { hook["permissionDecisionReason"].as_str() } else { None }
}

#[test]
fn first_edit_deny_denies_the_first_source_edit_and_names_prove_and_the_task() {
    let r = Repo::new("first");
    let task = r.accepted();
    let out = r.pre("s1", "src/a.rs");
    let text = denial(&out).unwrap_or_else(|| panic!("no denial: {out}"));
    assert!(text.contains(&format!("rvow prove {task}")), "{text}");
    assert!(text.contains("has no baseline yet"), "{text}");
    assert!(r.ok(&["stats"]).contains("baseline-first"), "the denial is counted under its rule");
}

#[test]
fn first_edit_deny_second_try_goes_through_and_another_session_is_denied_again() {
    let r = Repo::new("once");
    r.accepted();
    assert!(denial(&r.pre("s1", "src/a.rs")).is_some());
    assert_eq!(r.pre("s1", "src/a.rs"), Value::Null, "the second try of the same session goes through");
    assert!(denial(&r.pre("s2", "src/a.rs")).is_some(), "another session is denied again");
}

#[test]
fn first_edit_deny_skips_a_test_file() {
    let r = Repo::new("test-file");
    r.accepted();
    assert_eq!(r.pre("s1", "tests/a.rs"), Value::Null);
    assert!(denial(&r.pre("s1", "src/a.rs")).is_some(), "a test edit does not use up the denial");
}

#[test]
fn first_edit_deny_skips_a_task_whose_baseline_is_recorded() {
    let r = Repo::new("recorded");
    let task = r.accepted();
    std::fs::create_dir_all(r.repo.join("tests")).unwrap();
    std::fs::write(r.repo.join("tests/a.rs"), "// the new test\n").unwrap();
    let out = r.ok(&["prove", &task]);
    assert!(out.contains("baseline: recorded"), "{out}");
    assert_eq!(r.pre("s1", "src/a.rs"), Value::Null);
}

#[test]
fn first_edit_deny_skips_a_file_outside_the_task() {
    let r = Repo::new("outside");
    r.accepted();
    std::fs::write(r.repo.join("other.txt"), "x\n").unwrap();
    assert_eq!(r.pre("s1", "other.txt"), Value::Null);
}

#[test]
fn first_edit_deny_covers_a_bash_command_that_writes_a_source_file() {
    let r = Repo::new("bash");
    let task = r.accepted();
    let input = json!({
        "cwd": r.repo.to_str().unwrap(),
        "session_id": "s1",
        "tool_name": "Bash",
        "tool_input": {"command": "echo ok > src/a.rs"},
        "tool_use_id": "use-bash",
    });
    let out = r.send("pre-tool-use", &input);
    let text = denial(&out).unwrap_or_else(|| panic!("no denial: {out}"));
    assert!(text.contains(&format!("rvow prove {task}")), "{text}");
    assert_eq!(r.send("pre-tool-use", &input), Value::Null, "the second try goes through");
}

#[test]
fn first_edit_deny_is_audited_under_any_yolo_tier() {
    for tier in ["audit", "auto", "bypass"] {
        let r = Repo::new(&format!("yolo-{tier}"));
        r.accepted();
        r.ok(&["yolo", tier, "--for", "30m"]);
        assert_eq!(r.pre("s1", "src/a.rs"), Value::Null, "yolo {tier} lets the first edit through");
    }
}

#[test]
fn first_edit_deny_survives_a_yolo_period() {
    let r = Repo::new("yolo-ends");
    r.accepted();
    r.ok(&["yolo", "audit", "--for", "30m"]);
    assert_eq!(r.pre("s1", "src/a.rs"), Value::Null, "yolo lets the first edit through");
    r.ok(&["yolo", "off"]);
    assert!(denial(&r.hook("pre-tool-use", "s1", "src/a.rs")).is_some(), "the session's one denial was not used up");
}

#[test]
fn first_edit_deny_post_tool_use_answers_nothing_for_a_success_or_a_failure() {
    let r = Repo::new("post");
    r.accepted();
    assert!(denial(&r.pre("s1", "src/a.rs")).is_some());
    assert_eq!(r.pre("s1", "src/a.rs"), Value::Null);
    assert_eq!(r.hook("post-tool-use", "s1", "src/a.rs"), Value::Null);
    let input = json!({
        "cwd": r.repo.to_str().unwrap(),
        "session_id": "s1",
        "tool_use_id": "use-src/a.rs",
        "hook_event_name": "PostToolUseFailure",
    });
    assert_eq!(r.send("post-tool-use", &input), Value::Null);
}
