//! The first source edit of a baseline-pending task gets one notice that names `rvow prove`.

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
        let root = std::env::temp_dir().join(format!("rvow-first-edit-notice-{}-{name}", std::process::id()));
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

    /// An allowed `Edit` of `file`: its pre-tool-use, then its post-tool-use. Returns the post answer.
    fn pre(&self, session: &str, file: &str) -> Value {
        let before = self.hook("pre-tool-use", session, file);
        assert_eq!(before, Value::Null, "the pre-tool-use of an allowed call stays silent");
        self.hook("post-tool-use", session, file)
    }
}

/// The notice text of a hook answer, or none.
fn notice(out: &Value) -> Option<&str> {
    out["hookSpecificOutput"]["additionalContext"].as_str()
}

#[test]
fn first_edit_notice_names_prove_and_the_task_at_the_first_source_edit() {
    let r = Repo::new("first");
    let task = r.accepted();
    let out = r.pre("s1", "src/a.rs");
    assert_eq!(out["hookSpecificOutput"]["hookEventName"], "PostToolUse", "{out}");
    let text = notice(&out).unwrap_or_else(|| panic!("no notice: {out}"));
    assert!(text.contains(&format!("rvow prove {task}")), "{text}");
    assert!(!text.contains('\n'), "{text}");
}

#[test]
fn first_edit_notice_is_given_once_per_task_per_session() {
    let r = Repo::new("once");
    r.accepted();
    assert!(notice(&r.pre("s1", "src/a.rs")).is_some());
    assert_eq!(notice(&r.pre("s1", "src/a.rs")), None);
    assert!(notice(&r.pre("s2", "src/a.rs")).is_some(), "another session is told again");
}

#[test]
fn first_edit_notice_skips_a_test_file() {
    let r = Repo::new("test-file");
    r.accepted();
    assert_eq!(notice(&r.pre("s1", "tests/a.rs")), None);
    assert!(notice(&r.pre("s1", "src/a.rs")).is_some(), "a test edit does not use up the notice");
}

#[test]
fn first_edit_notice_skips_a_task_whose_baseline_is_recorded() {
    let r = Repo::new("recorded");
    let task = r.accepted();
    std::fs::create_dir_all(r.repo.join("tests")).unwrap();
    std::fs::write(r.repo.join("tests/a.rs"), "// the new test\n").unwrap();
    let out = r.ok(&["prove", &task]);
    assert!(out.contains("baseline: recorded"), "{out}");
    assert_eq!(notice(&r.pre("s1", "src/a.rs")), None);
}

#[test]
fn first_edit_notice_never_denies_and_skips_a_file_outside_the_task() {
    let r = Repo::new("outside");
    r.accepted();
    std::fs::write(r.repo.join("other.txt"), "x\n").unwrap();
    let out = r.pre("s1", "other.txt");
    assert_eq!(notice(&out), None, "{out}");
}

#[test]
fn first_edit_notice_covers_a_bash_command_that_writes_a_source_file() {
    let r = Repo::new("bash");
    let task = r.accepted();
    let input = json!({
        "cwd": r.repo.to_str().unwrap(),
        "session_id": "s1",
        "tool_name": "Bash",
        "tool_input": {"command": "echo ok > src/a.rs"},
        "tool_use_id": "use-bash",
    });
    assert_eq!(r.send("pre-tool-use", &input), Value::Null);
    let out = r.send("post-tool-use", &input);
    let text = notice(&out).unwrap_or_else(|| panic!("no notice: {out}"));
    assert!(text.contains(&format!("rvow prove {task}")), "{text}");
}

#[test]
fn first_edit_notice_echoes_the_failure_event_name() {
    // The post-tool-use handler also answers PostToolUseFailure, and Claude Code
    // rejects an answer whose event name differs from the call's.
    let r = Repo::new("failure");
    r.accepted();
    assert_eq!(r.hook("pre-tool-use", "s1", "src/a.rs"), Value::Null);
    let input = json!({
        "cwd": r.repo.to_str().unwrap(),
        "session_id": "s1",
        "tool_use_id": "use-src/a.rs",
        "hook_event_name": "PostToolUseFailure",
    });
    let out = r.send("post-tool-use", &input);
    assert_eq!(out["hookSpecificOutput"]["hookEventName"], "PostToolUseFailure", "{out}");
    assert!(notice(&out).is_some(), "{out}");
}

#[test]
fn first_edit_notice_survives_a_call_that_never_ran() {
    // A call denied after its pre-tool-use gets no post-tool-use: the task is
    // not marked as told, so the next source edit still gets the notice.
    let r = Repo::new("denied");
    r.accepted();
    assert_eq!(r.hook("pre-tool-use", "s1", "src/a.rs"), Value::Null);
    assert!(notice(&r.pre("s1", "src/a.rs")).is_some(), "the notice was used up by a call that never ran");
}
