//! `rvow hook json <event>` speaks one plain JSON format for any harness, and
//! `rvow hook codex <event>` answers Codex, which reads Claude's shape but
//! cannot ask.

use std::io::Write;
use std::path::{Path, PathBuf};
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
    /// A repository with an rvow store and a plan of one task on `out.txt`, not accepted yet.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-hook-json-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let mut fx = Fx { root: root.canonicalize().unwrap(), id: String::new() };
        std::fs::write(fx.repo().join("a.txt"), "a").unwrap();
        fx.run("git", &["init", "-q", "-b", "main"]);
        fx.run("git", &["add", "a.txt"]);
        fx.run("git", &["commit", "-q", "-m", "init"]);
        fx.run(RVOW, &["init"]);
        let out = fx.run(RVOW, &["new", "--spec", "Out"]);
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        fx.id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        std::fs::write(
            dir.join("tasks.md"),
            "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
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

    fn accept(&self) {
        self.run(RVOW, &["accept", &self.id, "--yes"]);
    }

    /// The JSON that `rvow hook <harness> <event>` prints for `input`, from `cwd`.
    fn hook_in(&self, cwd: &Path, harness: &str, event: &str, input: Value) -> Value {
        let mut payload = input;
        payload["cwd"] = json!(cwd);
        let mut child = self.cmd(RVOW).args(["hook", harness, event]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "a hook exits 0");
        let text = String::from_utf8(out.stdout).unwrap();
        serde_json::from_str(text.trim()).unwrap_or_else(|e| panic!("not one JSON object: {text:?}: {e}"))
    }

    fn hook(&self, harness: &str, event: &str, input: Value) -> Value {
        self.hook_in(&self.repo(), harness, event, input)
    }
}

#[test]
fn pre_tool_use_prints_a_neutral_decision() {
    let fx = Fx::new("pre");
    let deny = fx.hook("json", "pre-tool-use", json!({"tool": "Bash", "input": {"command": "rvow accept CHG-x"}}));
    assert_eq!(deny["decision"], "deny", "{deny}");
    assert!(deny["reason"].as_str().is_some_and(|r| r.contains("rvow accept")), "{deny}");
    let read = fx.hook("json", "pre-tool-use", json!({"tool": "Read", "input": {"file_path": "a.txt"}}));
    assert_eq!(read, json!({"decision": "allow"}));
}

#[test]
fn pre_tool_use_allows_in_a_repository_with_no_store() {
    let fx = Fx::new("none");
    let bare = fx.root.join("bare");
    std::fs::create_dir_all(&bare).unwrap();
    assert!(fx.cmd("git").current_dir(&bare).args(["init", "-q"]).status().unwrap().success());
    let out = fx.hook_in(&bare, "json", "pre-tool-use", json!({"tool": "Edit", "input": {"file_path": bare.join("x")}}));
    assert_eq!(out, json!({"decision": "allow"}));
}

#[test]
fn events_stop_and_session_start_use_the_neutral_format() {
    let fx = Fx::new("events");
    assert_eq!(fx.hook("json", "stop", json!({"session_id": "s1"})), json!({}), "nothing to hold");
    fx.accept();
    let write = json!({"tool": "Write", "input": {"file_path": fx.repo().join("out.txt")}, "session_id": "s1", "tool_use_id": "t1"});
    assert_eq!(fx.hook("json", "pre-tool-use", write)["decision"], "allow");
    std::fs::write(fx.repo().join("out.txt"), "no").unwrap();
    fx.hook("json", "post-tool-use", json!({"session_id": "s1", "tool_use_id": "t1"}));
    let stop = fx.hook("json", "stop", json!({"session_id": "s1"}));
    assert_eq!(stop["decision"], "block", "{stop}");
    assert!(stop["reason"].as_str().is_some_and(|r| !r.is_empty()), "{stop}");
    let start = fx.hook("json", "session-start", json!({"session_id": "s2", "source": "startup"}));
    assert!(start["context"].as_str().is_some_and(|c| c.contains("TSK-")), "{start}");
    assert!(start.get("hookSpecificOutput").is_none(), "{start}");
}

#[test]
fn codex_shell_calls_are_judged_and_an_ask_is_a_deny() {
    let fx = Fx::new("codex");
    let shell = json!({"tool_name": "shell", "tool_input": {"command": ["bash", "-lc", "rvow accept CHG-x"]}});
    let out = fx.hook("codex", "pre-tool-use", shell);
    assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny", "{out}");
    let edit = json!({"tool_name": "apply_patch", "tool_input": {"file_path": fx.repo().join("out.txt")}, "session_id": "s1"});
    for n in 1..=4 {
        let out = fx.hook("codex", "pre-tool-use", edit.clone());
        assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny", "call {n}: {out}");
    }
}

#[test]
fn docs_harnesses_page_names_every_harness() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let page = std::fs::read_to_string(root.join("docs/harnesses.md")).expect("docs/harnesses.md");
    for harness in ["claude", "codex", "json"] {
        assert!(page.contains(&format!("rvow hook {harness} ")), "docs/harnesses.md does not show `rvow hook {harness} <event>`");
    }
    for field in ["\"decision\"", "\"reason\"", "\"context\"", "\"tool\"", "\"input\"", "\"user_made\""] {
        assert!(page.contains(field), "docs/harnesses.md does not name {field}");
    }
    let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
    assert!(readme.contains("docs/harnesses.md"), "README.md links the harness page");
}
