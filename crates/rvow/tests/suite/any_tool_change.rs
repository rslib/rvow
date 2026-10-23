//! REQ-zx21wm and REQ-x3hh9m: any writing tool records the files it changed
//! with one snapshot, and a read-only tool takes none.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

struct Repo {
    root: PathBuf,
    repo: PathBuf,
    home: PathBuf,
}

impl Drop for Repo {
    fn drop(&mut self) {
        rvow_core::git::testrepo::remove_tree(&self.root);
    }
}

impl Repo {
    fn new(name: &str) -> Repo {
        let root = std::env::temp_dir().join(format!("rvow-any-tool-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo/src")).unwrap();
        let root = root.canonicalize().unwrap();
        let r = Repo { root: root.clone(), repo: root.join("repo"), home: root.join("home") };
        std::fs::write(r.repo.join("src/a.rs"), "a\n").unwrap();
        for args in [vec!["init", "-q", "-b", "main"], vec!["add", "."], vec!["commit", "-q", "-m", "init"]] {
            let ok = Command::new("git")
                .current_dir(&r.repo)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t")
                .args(&args)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        }
        r.hook_raw("init", &[], None);
        r
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

    /// Runs rvow with `args` and, when given, `input` on stdin.
    fn hook_raw(&self, first: &str, rest: &[&str], input: Option<&Value>) {
        let mut child =
            self.command().arg(first).args(rest).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        if let Some(input) = input {
            child.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
        } else {
            drop(child.stdin.take());
        }
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "rvow {first} {rest:?}: {out:?}");
    }

    /// One call of `tool` by session `s1`: pre-tool-use, `work`, post-tool-use.
    fn call(&self, id: &str, tool: &str, args: Value, work: impl FnOnce(&Path)) {
        let input = json!({"cwd": self.repo, "session_id": "s1", "tool_use_id": id, "tool_name": tool, "tool_input": args});
        self.hook_raw("hook", &["claude", "pre-tool-use"], Some(&input));
        work(&self.repo);
        let post = json!({"cwd": self.repo, "session_id": "s1", "tool_use_id": id});
        self.hook_raw("hook", &["claude", "post-tool-use"], Some(&post));
    }

    /// Every provenance record of the project, one JSON value per line.
    fn records(&self) -> Vec<Value> {
        let mut out = Vec::new();
        let mut stack = vec![self.home.clone()];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.file_name().is_some_and(|n| n == "records.jsonl") && p.to_string_lossy().contains("/provenance/") {
                    let text = std::fs::read_to_string(&p).unwrap();
                    out.extend(text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()));
                }
            }
        }
        out
    }
}

#[test]
fn any_tool_change_an_mcp_tool_records_the_file_it_wrote() {
    let r = Repo::new("mcp");
    r.call("c1", "Edit", json!({"file_path": r.repo.join("src/a.rs")}), |repo| std::fs::write(repo.join("src/a.rs"), "a\n").unwrap());
    r.call("c2", "mcp__fs__write", json!({"path": "src/a.rs"}), |repo| std::fs::write(repo.join("src/a.rs"), "b\n").unwrap());
    let recs = r.records();
    assert!(
        recs.iter().any(|v| v["kind"] == "after" && v["tool_use_id"] == "c2" && v["path"] == "src/a.rs"),
        "the MCP write of src/a.rs is recorded: {recs:?}"
    );
    assert!(!recs.iter().any(|v| v["kind"] == "start" && v["tool_use_id"] == "c2"), "the call's start record is gone after it ends");
}

#[test]
fn any_tool_change_a_read_only_tool_takes_no_snapshot() {
    let r = Repo::new("reads");
    let grep = json!({"cwd": r.repo, "session_id": "s1", "tool_use_id": "g1", "tool_name": "Grep", "tool_input": {"pattern": "a"}});
    let search =
        json!({"cwd": r.repo, "session_id": "s1", "tool_use_id": "m1", "tool_name": "mcp__rkb__search", "tool_input": {"query": "a"}});
    r.hook_raw("hook", &["claude", "pre-tool-use"], Some(&grep));
    r.hook_raw("hook", &["claude", "pre-tool-use"], Some(&search));
    let recs = r.records();
    assert!(!recs.iter().any(|v| v["kind"] == "start"), "a read-only call records no start and takes no snapshot: {recs:?}");
}

#[test]
fn any_tool_change_a_light_call_records_no_before_tree() {
    // After the session's first call, an MCP writer takes one snapshot, after the call: its start record holds no before tree.
    let r = Repo::new("light");
    r.call("c1", "edit", json!({"filePath": r.repo.join("src/a.rs")}), |_| {});
    let input =
        json!({"cwd": r.repo, "session_id": "s1", "tool_use_id": "w1", "tool_name": "mcp__fs__write", "tool_input": {"path": "src/a.rs"}});
    r.hook_raw("hook", &["claude", "pre-tool-use"], Some(&input));
    let recs = r.records();
    let start = recs.iter().find(|v| v["kind"] == "start" && v["tool_use_id"] == "w1").unwrap_or_else(|| panic!("no start: {recs:?}"));
    assert_eq!(start["before"], ":last", "{start}");
}

#[test]
fn any_tool_change_knows_the_tools_of_each_harness() {
    use rvow_hook::hook::{ToolKind, tool_kind};
    // Claude Code, pi, OpenCode, Codex and Gemini CLI.
    for t in ["Edit", "Write", "MultiEdit", "edit", "write", "patch", "apply_patch", "write_file", "replace"] {
        assert_eq!(tool_kind(t), ToolKind::Editor, "{t}");
    }
    for t in ["Bash", "bash", "shell", "local_shell", "run_shell_command"] {
        assert_eq!(tool_kind(t), ToolKind::Shell, "{t}");
    }
    for t in ["Read", "Grep", "Glob", "read", "grep", "list", "read_file", "search_file_content", "mcp__rkb__search"] {
        assert_eq!(tool_kind(t), ToolKind::ReadOnly, "{t}");
    }
    for t in ["mcp__fs__write", "codemode", "some_new_tool"] {
        assert_eq!(tool_kind(t), ToolKind::Other, "{t}");
    }
}

#[test]
fn any_tool_change_a_shell_of_another_harness_gets_the_shell_rules() {
    // OpenCode names its shell `bash` and Codex sends an argv: both meet the
    // same rules as Claude Code's `Bash`, so neither runs an owner command.
    let r = Repo::new("shells");
    let project = rvow_core::store::find(&r.repo, &r.home).unwrap().unwrap();
    for (tool, command) in [("bash", json!("rvow accept CHG-x")), ("shell", json!(["bash", "-lc", "rvow accept CHG-x"]))] {
        let input = json!({"cwd": r.repo, "tool_name": tool, "tool_input": {"command": command}});
        let denial = rvow_hook::hook::pre_tool_use_denial(&project, &input).unwrap();
        assert!(denial.is_some(), "{tool} ran rvow accept");
    }
}

#[test]
fn any_tool_change_every_shell_name_gets_the_shell_rules() {
    let r = Repo::new("shell-names");
    let project = rvow_core::store::find(&r.repo, &r.home).unwrap().unwrap();
    for tool in ["Bash", "PowerShell", "shell", "shell_command", "local_shell", "exec_command", "run_shell_command"] {
        let input = json!({"cwd": r.repo, "tool_name": tool, "tool_input": {"command": "rvow accept CHG-x"}});
        let denial = rvow_hook::hook::pre_tool_use_denial(&project, &input).unwrap();
        assert!(denial.is_some(), "{tool} ran rvow accept");
    }
}

#[test]
fn any_tool_change_an_opencode_edit_records_its_file() {
    let r = Repo::new("opencode");
    r.call("c1", "edit", json!({"filePath": r.repo.join("src/a.rs")}), |repo| std::fs::write(repo.join("src/a.rs"), "b\n").unwrap());
    let recs = r.records();
    assert!(recs.iter().any(|v| v["kind"] == "after" && v["tool_use_id"] == "c1" && v["path"] == "src/a.rs"), "{recs:?}");
}
