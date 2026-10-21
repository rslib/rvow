//! The first source edit of a task is never denied for a baseline: rvow takes the baseline from the code at accept.

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

#[test]
fn first_edit_allowed_a_source_edit_is_not_denied_for_a_baseline() {
    let r = Repo::new("first");
    r.accepted();
    for session in ["s1", "s2"] {
        let out = r.pre(session, "src/a.rs");
        assert_eq!(out, Value::Null, "{out}");
    }
    assert!(!r.ok(&["stats"]).contains("baseline-first"), "no denial is counted");
}

#[test]
fn first_edit_allowed_a_bash_command_that_writes_a_source_file_is_not_denied() {
    let r = Repo::new("bash");
    r.accepted();
    let input = json!({
        "cwd": r.repo.to_str().unwrap(),
        "session_id": "s1",
        "tool_name": "Bash",
        "tool_input": {"command": "echo ok > src/a.rs"},
        "tool_use_id": "use-bash",
    });
    let out = r.send("pre-tool-use", &input);
    assert_eq!(out, Value::Null, "{out}");
}

#[test]
fn first_edit_allowed_a_test_file_and_a_file_outside_the_task_are_not_denied() {
    let r = Repo::new("others");
    r.accepted();
    std::fs::write(r.repo.join("other.txt"), "x\n").unwrap();
    assert_eq!(r.pre("s1", "tests/a.rs"), Value::Null);
    assert_eq!(r.pre("s1", "other.txt"), Value::Null);
}
