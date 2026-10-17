//! The Stop hook holds only for file content the session produced: blob
//! provenance from pre/post tool-use snapshots decides ownership.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");
const BIN: &str = "test -f out.txt && echo 'test result: ok. 1 passed; 0 failed;'";

/// Recursively finds a `provenance/*/records.jsonl` under `root`.
fn find_records(root: &Path) -> Option<String> {
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if p.file_name().and_then(|n| n.to_str()) == Some("provenance") {
                for sub in std::fs::read_dir(&p).ok()?.flatten() {
                    let f = sub.path().join("records.jsonl");
                    if f.is_file() {
                        return std::fs::read_to_string(f).ok();
                    }
                }
            } else if let Some(found) = find_records(&p) {
                return Some(found);
            }
        }
    }
    None
}

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
        let root = std::env::temp_dir().join(format!("rvow-provenance-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        let fx = Fx { root: root.canonicalize().unwrap() };
        for args in [vec!["init", "-q", "-b", "main"], vec!["commit", "-q", "--allow-empty", "-m", "init"]] {
            assert!(fx.cmd("git").args(&args).status().unwrap().success(), "git {args:?}");
        }
        fx.rvow(&["init"]);
        fx
    }

    fn cmd(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.current_dir(self.root.join("repo"))
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.root.join("rh"))
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t");
        c
    }

    fn hook(&self, event: &str, payload: &Value) -> Option<Value> {
        let mut child = self.cmd(RVOW).args(["hook", "claude", event]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
        (!out.trim().is_empty()).then(|| serde_json::from_str::<Value>(&out).unwrap())
    }

    fn rvow(&self, args: &[&str]) -> String {
        String::from_utf8_lossy(&self.cmd(RVOW).args(args).output().unwrap().stdout).into_owned()
    }

    /// Accepts a change whose one task writes out.txt, returns the task id.
    fn change(&self, title: &str, verify: &str) -> String {
        let out = self.rvow(&["new", "--spec", title]);
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        std::fs::write(dir.join("tasks.md"), format!("- TSK-new1 {title}\n  covers: REQ-new1\n  verify: {verify}\n  files: out.txt\n"))
            .unwrap();
        std::fs::write(
            dir.join("specs/x.md"),
            format!("## ADD REQ-new1 {title}\nrvow SHALL do it.\n### Scenario: s\n- WHEN it runs\n- THEN it is done\n"),
        )
        .unwrap();
        let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        self.rvow(&["accept", &id, "--yes"]);
        let located = self.rvow(&["path", &id]);
        let tasks =
            std::fs::read_to_string(PathBuf::from(located.lines().find_map(|l| l.strip_prefix("path: ")).unwrap().trim()).join("tasks.md"))
                .unwrap();
        tasks.lines().find_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).unwrap()
    }

    /// The stored records of the private store of this worktree.
    fn records(&self) -> Option<String> {
        let root = self.root.join("rh");
        find_records(&root)
    }

    /// A pre-tool-use call of `session` for a Bash or Write with `id`.
    fn pre(&self, session: &str, id: &str, tool: &str, args: Value) {
        let payload =
            json!({"cwd": self.root.join("repo"), "session_id": session, "tool_use_id": id, "tool_name": tool, "tool_input": args});
        let out = self.hook("pre-tool-use", &payload);
        assert!(out.is_none(), "the call of {tool} was denied: {out:?}");
    }

    /// A post-tool-use call of `session` for call `id`.
    fn post(&self, session: &str, id: &str) {
        let payload = json!({"cwd": self.root.join("repo"), "session_id": session, "tool_use_id": id});
        // The only answer is the first-edit notice of a baseline-pending task.
        let out = self.hook("post-tool-use", &payload);
        let notice = out.as_ref().and_then(|o| o["hookSpecificOutput"]["additionalContext"].as_str());
        assert!(out.is_none() || notice.is_some_and(|n| n.contains("baseline pending")), "{out:?}");
    }

    /// A Write call of `session` that sets `rel` to `content`, with call `id`.
    fn write(&self, session: &str, id: &str, rel: &str, content: &str) {
        let path = self.root.join("repo").join(rel);
        self.pre(session, id, "Write", json!({"file_path": path}));
        std::fs::write(&path, content).unwrap();
        self.post(session, id);
    }

    fn stop(&self, session: &str) -> Option<String> {
        let payload = json!({"cwd": self.root.join("repo"), "session_id": session});
        self.hook("stop", &payload).map(|v| v["reason"].as_str().unwrap().to_string())
    }
}

#[test]
fn provenance_stop_holds_only_when_the_session_wrote_the_current_blob() {
    let fx = Fx::new("owns");
    let task = fx.change("Output", BIN);
    // s1 writes out.txt, the current content.
    fx.write("s1", "c1", "out.txt", "ok");
    let reason = fx.stop("s1").expect("s1 wrote the current content");
    assert!(reason.contains(&task), "{reason}");

    // A worker rewrites out.txt: s1 no longer owns it and stops.
    fx.write("worker", "c2", "out.txt", "worker content");
    assert_eq!(fx.stop("s1"), None, "a worker rewrote out.txt, so s1 is not held");
}

#[test]
fn provenance_unknown_program_beside_readers_records_nothing() {
    let fx = Fx::new("reads");
    fx.change("Output", BIN);
    let args = json!({"command": "pgrep -fl \"a|b\"; echo === out.txt; cat out.txt"});
    fx.pre("s1", "r1", "Bash", args);
    fx.post("s1", "r1");
    let records = fx.records().expect("a checked call records its before snapshot");
    assert!(!records.contains("\"after\""), "an unknown program beside readers records no changed file: {records}");
    assert_eq!(fx.stop("s1"), None, "a read-only call records no file the session owns");

    // A script write is recorded: the Bash call may write.
    let args = json!({"command": "./fix.sh"});
    fx.pre("s1", "w1", "Bash", args);
    std::fs::write(fx.root.join("repo/out.txt"), "fixed").unwrap();
    fx.post("s1", "w1");
    let records = fx.records().expect("a write call records provenance");
    assert!(records.contains("out.txt"), "{records}");
    let reason = fx.stop("s1").expect("the session wrote out.txt");
    assert!(reason.contains("no proof"), "{reason}");
}

#[test]
fn provenance_an_older_mod_that_sends_only_pre_tool_use_still_holds() {
    let fx = Fx::new("oldmod");
    let task = fx.change("Output", BIN);
    // Only a pre-tool-use call, as an older mod sends; the before snapshot
    // records but no post-tool-use ever arrives.
    let path = fx.root.join("repo/out.txt");
    fx.pre("s1", "t1", "Write", json!({"file_path": path}));
    std::fs::write(&path, "ok").unwrap();
    assert!(fx.records().is_some());
    let reason = fx.stop("s1").expect("no after-call records keeps today's text record");
    assert!(reason.contains(&task), "{reason}");
}

#[test]
fn provenance_snapshots_do_not_touch_the_repository_state() {
    let fx = Fx::new("clean");
    let repo = fx.root.join("repo");
    std::fs::write(repo.join("new.txt"), "x").unwrap();
    let index_before = std::fs::read(repo.join(".git/index")).unwrap();
    fx.write("s1", "c1", "new.txt", "y");
    let index_after = std::fs::read(repo.join(".git/index")).unwrap();
    assert_eq!(index_after, index_before, "the repository index must not change");
    fx.rvow(&["status"]);
}
