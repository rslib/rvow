//! TSK-qy1w0b: a Bash call that runs a relaxed task's own verify command
//! records an observed proof of it, and a strict task never gets one.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// The verify of the fixture task: a command that prints one passing test.
const PASS: &str = "echo 'test result: ok. 1 passed; 0 failed;'";

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
        let root = std::env::temp_dir().join(format!("rvow-observed-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        let root = root.canonicalize().unwrap();
        let r = Repo { root: root.clone(), repo: root.join("repo"), home: root.join("home") };
        std::fs::write(r.repo.join("a.txt"), "no\n").unwrap();
        std::fs::write(r.repo.join("b.txt"), "b\n").unwrap();
        r.git(&["init", "-q", "-b", "main"]);
        r.git(&["add", "."]);
        r.git(&["commit", "-q", "-m", "init"]);
        r.ok(&["init", "--in-repo"]);
        r
    }

    fn git(&self, args: &[&str]) {
        let ok = self.command("git").args(args).status().unwrap().success();
        assert!(ok, "git {args:?}");
    }

    fn command(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("RVOW_HOME", &self.home)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t");
        c
    }

    fn rvow(&self, args: &[&str]) -> (String, String, i32) {
        let out = self.command(env!("CARGO_BIN_EXE_rvow")).args(args).output().unwrap();
        (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap())
    }

    fn ok(&self, args: &[&str]) -> String {
        let (out, err, code) = self.rvow(args);
        assert_eq!(code, 0, "rvow {args:?}: {out}{err}");
        out
    }

    /// An accepted change on `main` with one task over `a.txt` and `b.txt`,
    /// marked `mode: relaxed` when `relaxed`. Returns its id and task id.
    fn plan(&self, relaxed: bool) -> (String, String) {
        let out = self.ok(&["new", "Out"]);
        let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        std::fs::write(dir.join("proposal.md"), "branch: main\n\n# Out\n\n## Why\nw\n\n## What changes\nw\n\n## Out of scope\nNone.\n")
            .unwrap();
        let mode = if relaxed { "  mode: relaxed\n" } else { "" };
        std::fs::write(dir.join("tasks.md"), format!("- TSK-new1 Write ok\n  verify: {PASS}\n  files: a.txt, b.txt\n{mode}")).unwrap();
        self.ok(&["accept", &id, "--yes"]);
        let written = PathBuf::from(self.ok(&["path", &id]).lines().find_map(|l| l.strip_prefix("path: ")).unwrap()).join("tasks.md");
        let task = std::fs::read_to_string(written)
            .unwrap()
            .lines()
            .find_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from))
            .expect("the tasks file names one task");
        (id, task)
    }

    /// Runs `rvow hook claude <event>` on `input`.
    fn hook(&self, event: &str, input: &Value) {
        let mut child = self
            .command(env!("CARGO_BIN_EXE_rvow"))
            .args(["hook", "claude", event])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "rvow hook {event} exited {}: {}", out.status, String::from_utf8_lossy(&out.stderr));
    }

    /// The pre-tool-use of a Bash call by session `s1` that runs `command`.
    fn pre(&self, call: &str, command: &str) {
        let input = json!({
            "cwd": self.repo,
            "session_id": "s1",
            "tool_use_id": call,
            "tool_name": "Bash",
            "tool_input": {"command": command},
        });
        self.hook("pre-tool-use", &input);
    }

    /// The post-tool-use of the Bash call `call`; `extra` carries `exit_ok`
    /// and `output_tail`.
    fn post(&self, call: &str, extra: Value) {
        let mut input = json!({"cwd": self.repo, "session_id": "s1", "tool_use_id": call});
        for (k, v) in extra.as_object().into_iter().flatten() {
            input[k.as_str()] = v.clone();
        }
        self.hook("post-tool-use", &input);
    }

    /// The `hook-state/` folder of the project.
    fn state(&self) -> PathBuf {
        self.repo.join("rvow/hook-state")
    }

    /// The pending observed runs the project holds, by file name.
    fn pending(&self) -> Vec<String> {
        let dir = self.state().join("observe");
        let mut out: Vec<String> =
            std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        out.sort();
        out
    }

    /// Every Prove event of the active change's ledger.
    fn proofs(&self) -> Vec<Value> {
        let dir = std::fs::read_dir(self.repo.join("rvow/active")).unwrap().next().unwrap().unwrap().path();
        std::fs::read_to_string(dir.join("ledger.jsonl"))
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|e| e["kind"] == "prove")
            .collect()
    }

    /// The `rvow status` line of `task`.
    fn status_line(&self, task: &str) -> String {
        let status = self.ok(&["--human", "status"]);
        status.lines().find(|l| l.contains(task)).unwrap_or_else(|| panic!("no status line for {task}: {status}")).to_string()
    }

    fn file(&self, rel: &str) -> PathBuf {
        self.repo.join(rel)
    }

    fn write(&self, rel: &str, text: &str) {
        std::fs::write(self.file(rel), text).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.file(rel)).unwrap()
    }
}

#[test]
fn observed_proof_pass_is_recorded() {
    let r = Repo::new("pass");
    let (change, task) = r.plan(true);
    r.pre("o1", PASS);
    assert_eq!(r.pending(), ["o1.json"], "the run waits for its end");
    r.post("o1", json!({"exit_ok": true, "output_tail": "test result: ok. 1 passed; 0 failed;"}));
    assert!(r.pending().is_empty(), "the pending run is gone: {:?}", r.pending());
    assert!(r.status_line(&task).split_whitespace().any(|w| w == "proved"), "the task is proved: {}", r.status_line(&task));
    let proofs = r.proofs();
    let last = proofs.last().unwrap_or_else(|| panic!("no proof recorded"));
    assert_eq!(last["observed"], true, "{last}");
    assert_eq!(last["proved"], true, "{last}");
    assert_eq!(last["task"], task.as_str(), "{last}");
    assert_eq!(last["tests_run"], 1, "{last}");
    assert_eq!(last["verify"], PASS, "{last}");
    assert!(last["seal"].is_string(), "the event is sealed: {last}");
    let shown = r.ok(&["show", &task]);
    assert!(shown.contains(&change), "show names the change: {shown}");
    assert!(shown.contains("proof runs: 1 in total"), "show lists the run: {shown}");
}

#[test]
fn observed_proof_file_change_during_run_records_nothing() {
    let r = Repo::new("changed");
    let (_change, task) = r.plan(true);
    r.pre("o2", PASS);
    assert_eq!(r.pending(), ["o2.json"], "the run waits for its end");
    r.write("a.txt", "ok\n");
    r.post("o2", json!({"exit_ok": true, "output_tail": "test result: ok. 1 passed; 0 failed;"}));
    assert!(r.pending().is_empty(), "the pending run is gone: {:?}", r.pending());
    assert!(r.proofs().is_empty(), "no proof is recorded: {:?}", r.proofs());
    assert!(r.status_line(&task).contains("missing"), "the task stands unproved: {}", r.status_line(&task));
}

#[test]
fn observed_proof_failed_run_records_nothing() {
    let r = Repo::new("failed");
    let (_change, task) = r.plan(true);
    r.pre("o3", PASS);
    assert_eq!(r.pending(), ["o3.json"], "the run waits for its end");
    r.post("o3", json!({"exit_ok": false, "output_tail": "error: no such file\n"}));
    assert!(r.pending().is_empty(), "the pending run is gone: {:?}", r.pending());
    assert!(r.proofs().is_empty(), "no proof is recorded: {:?}", r.proofs());
    assert!(r.status_line(&task).contains("missing"), "the task stands unproved: {}", r.status_line(&task));
}

#[test]
fn observed_proof_zero_tests_records_nothing() {
    let r = Repo::new("none");
    let (_change, task) = r.plan(true);
    r.pre("o4", PASS);
    assert_eq!(r.pending(), ["o4.json"], "the run waits for its end");
    r.post("o4", json!({"exit_ok": true, "output_tail": "running 0 tests\n\ntest result: ok. 0 passed; 0 failed;\n"}));
    assert!(r.pending().is_empty(), "the pending run is gone: {:?}", r.pending());
    assert!(r.proofs().is_empty(), "no proof is recorded: {:?}", r.proofs());
    assert!(r.status_line(&task).contains("missing"), "the task stands unproved: {}", r.status_line(&task));
}

#[test]
fn observed_proof_strict_task_records_nothing() {
    let r = Repo::new("strict");
    let (_change, task) = r.plan(false);
    r.pre("o5", PASS);
    assert!(r.pending().is_empty(), "a strict task leaves no pending run: {:?}", r.pending());
    r.post("o5", json!({"exit_ok": true, "output_tail": "test result: ok. 1 passed; 0 failed;"}));
    assert!(r.proofs().is_empty(), "a strict task gets no observed proof: {:?}", r.proofs());
    assert!(r.status_line(&task).contains("missing"), "the task stands unproved: {}", r.status_line(&task));
}

#[test]
fn observed_proof_other_command_writes_no_pending_file() {
    let r = Repo::new("other");
    let (_change, _task) = r.plan(true);
    r.pre("o6", "git status --short");
    assert!(r.pending().is_empty(), "another command writes no pending run: {:?}", r.pending());
    assert!(!r.state().join("observe").exists(), "the folder of pending runs is not created");
    // A command that only holds the verify inside a longer line is not the verify either.
    r.pre("o7", "echo");
    assert!(r.pending().is_empty(), "a prefix of the verify is not the verify: {:?}", r.pending());
    assert!(r.read("a.txt").contains("no"), "the fixture file is untouched");
}
