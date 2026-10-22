//! REQ-bfpfqg: an editor call hashes only the files it names, so a change a
//! background job made while the call was open is not recorded for the session;
//! the next shell call scans the whole tree and finds it, and a target that git
//! ignores is allowed and falls back to a full scan.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use rvow_core::{provenance, store};
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
    /// A repository with an rvow store, a committed `a.txt` and whatever
    /// `.gitignore` holds.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-editor-snapshot-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let fx = Fx { root: root.canonicalize().unwrap() };
        std::fs::write(fx.at("a.txt"), "a").unwrap();
        std::fs::write(fx.at(".gitignore"), "*.log\n").unwrap();
        for args in [&["init", "-q", "-b", "main"][..], &["add", "a.txt", ".gitignore"], &["commit", "-q", "-m", "init"]] {
            assert!(fx.cmd("git").args(args).status().unwrap().success(), "git {args:?}");
        }
        assert!(fx.cmd(RVOW).arg("init").output().unwrap().status.success());
        fx
    }

    fn at(&self, name: &str) -> PathBuf {
        self.root.join("repo").join(name)
    }

    fn cmd(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.current_dir(self.at(""))
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

    /// Runs `rvow hook claude <event>` on `payload` and returns its stdout. A
    /// hook always exits 0, since a broken hook must not stop the agent.
    fn hook(&self, event: &str, payload: Value) -> String {
        let mut child = self.cmd(RVOW).args(["hook", "claude", event]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{event} exited {}: {}", out.status, String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// The deny reason of one pre-tool-use call, or `None` when it is allowed.
    fn pre(&self, tool: &str, id: &str, input: Value) -> Option<String> {
        let out = self.hook(
            "pre-tool-use",
            json!({"cwd": self.at(""), "tool_name": tool, "tool_input": input, "session_id": "s1", "tool_use_id": id}),
        );
        (!out.trim().is_empty()).then(|| {
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny", "{out}");
            v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().to_string()
        })
    }

    /// The post-tool-use of the call `id`, in the shape the mods send: no tool
    /// name and no tool input.
    fn post(&self, id: &str) {
        let out = self.hook("post-tool-use", json!({"cwd": self.at(""), "session_id": "s1", "tool_use_id": id}));
        assert!(out.trim().is_empty(), "a post-tool-use hook prints nothing: {out}");
    }

    /// The paths the provenance records say `s1` owns right now.
    fn owned(&self) -> BTreeSet<String> {
        let project = store::find(&self.at(""), &self.root.join("rh")).unwrap().unwrap();
        provenance::owned_paths(&project, &self.at(""), "s1").unwrap().unwrap_or_default()
    }

    /// One Edit of `a.txt` with a background change to `bg.txt` landing while
    /// the call is open, the way a background job's write does. An earlier
    /// Edit gives the session its first snapshot, which is always a full one.
    fn edit_beside_a_background_change(&self) {
        assert_eq!(self.pre("Edit", "e0", json!({"file_path": self.at("a.txt"), "old_string": "a", "new_string": "a"})), None);
        self.post("e0");
        assert_eq!(self.pre("Edit", "e1", json!({"file_path": self.at("a.txt"), "old_string": "a", "new_string": "bb"})), None);
        std::fs::write(self.at("bg.txt"), "from a background job").unwrap();
        std::fs::write(self.at("a.txt"), "bb").unwrap();
        self.post("e1");
    }
}

#[test]
fn editor_snapshot_an_edit_names_its_file_and_not_the_background_change() {
    let fx = Fx::new("named");
    fx.edit_beside_a_background_change();
    let owned = fx.owned();
    assert!(owned.contains("a.txt"), "the call wrote a.txt: {owned:?}");
    assert!(!owned.contains("bg.txt"), "the call did not write bg.txt: {owned:?}");
}

#[test]
fn editor_snapshot_the_next_shell_call_finds_the_background_change() {
    let fx = Fx::new("shell");
    fx.edit_beside_a_background_change();
    assert!(!fx.owned().contains("bg.txt"), "the edit's snapshot saw only its own file");
    assert_eq!(fx.pre("Bash", "b1", json!({"command": "echo x > c.txt"})), None);
    std::fs::write(fx.at("c.txt"), "x\n").unwrap();
    fx.post("b1");
    let owned = fx.owned();
    assert!(owned.contains("bg.txt"), "a shell call scans the whole tree: {owned:?}");
    assert!(owned.contains("c.txt"), "the shell call wrote c.txt: {owned:?}");
}

#[test]
fn editor_snapshot_an_ignored_target_is_allowed_and_scanned_whole() {
    let fx = Fx::new("ignored");
    let write = json!({"file_path": fx.at("run.log"), "content": "x", "old_string": "", "new_string": "x"});
    assert_eq!(fx.pre("Write", "w1", write), None, "an ignored target is not a reason to deny");
    std::fs::write(fx.at("run.log"), "x").unwrap();
    fx.post("w1");
    // The pathspec git refuses takes the full scan, which still records a real edit.
    assert_eq!(fx.pre("Edit", "e2", json!({"file_path": fx.at("a.txt"), "old_string": "a", "new_string": "bb"})), None);
    std::fs::write(fx.at("a.txt"), "bb").unwrap();
    fx.post("e2");
    let owned = fx.owned();
    assert!(owned.contains("a.txt"), "the hook still snapshots after an ignored target: {owned:?}");
}

#[test]
fn editor_snapshot_a_first_edit_does_not_take_a_dirty_file_of_the_user() {
    let fx = Fx::new("dirty");
    std::fs::write(fx.at("user.txt"), "the user's own edit").unwrap();
    assert_eq!(fx.pre("Edit", "e1", json!({"file_path": fx.at("a.txt"), "old_string": "a", "new_string": "bb"})), None);
    std::fs::write(fx.at("a.txt"), "bb").unwrap();
    fx.post("e1");
    assert_eq!(fx.pre("Bash", "b1", json!({"command": "echo x > c.txt"})), None);
    std::fs::write(fx.at("c.txt"), "x\n").unwrap();
    fx.post("b1");
    let owned = fx.owned();
    assert!(owned.contains("a.txt"), "{owned:?}");
    assert!(!owned.contains("user.txt"), "a file the user changed before the session is not the session's: {owned:?}");
}
