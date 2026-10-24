//! `rvow gate --push`: the gate the git pre-push hook runs. It reads git's ref
//! lines on stdin, runs no task, no rerun and no project check, and fails only
//! on a broken record or on work that is not finished when the push is bound
//! for a land branch.

use std::path::PathBuf;
use std::process::{Command, Stdio};

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");
/// The sha of a ref the push carries: any sha but zeros is not a delete.
const SHA: &str = "1234567890123456789012345678901234567890";
/// The local sha of a delete, as git writes it.
const GONE: &str = "0000000000000000000000000000000000000000";

/// The counter a fixture's verify appends to. It is outside the repository, and
/// it is the same file before and after the fixture root is canonicalized, so a
/// test can name it in a config or a verify command before the fixture exists.
fn counter_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("rvow-push-gate-{}-{name}", std::process::id())).join("counter")
}

struct Fx {
    root: PathBuf,
    id: String,
    task: String,
    change: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        rvow_core::git::testrepo::remove_tree(&self.root);
    }
}

impl Fx {
    /// A repository on a `main` base branch with one accepted change on `feat/x`
    /// and one task that writes `out.txt`, not proved yet. `config` goes into
    /// the project config before accept, and the task's verify appends a line to
    /// the fixture's counter, outside the repository.
    fn new(name: &str, config: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-push-gate-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("rh")).unwrap();
        let mut fx = Fx { root: root.canonicalize().unwrap(), id: String::new(), task: String::new(), change: PathBuf::new() };
        std::fs::write(fx.repo().join("a.txt"), "a").unwrap();
        fx.git(&["init", "-q", "-b", "main"]);
        fx.git(&["add", "a.txt"]);
        fx.git(&["commit", "-q", "-m", "init"]);
        fx.git(&["checkout", "-q", "-b", "feat/x"]);
        let project = PathBuf::from(field(&fx.run(RVOW, &["init"]), "created"));
        if !config.is_empty() {
            std::fs::write(project.join("config.toml"), config).unwrap();
        }
        let out = fx.run(RVOW, &["new", "--spec", "Out"]);
        fx.id = field(&out, "created").to_string();
        let dir = PathBuf::from(field(&out, "path"));
        let verify = format!("echo ran >> {} && echo 'test result: ok. 1 passed; 0 failed;'", counter_path(name).display());
        std::fs::write(dir.join("tasks.md"), format!("- TSK-new1 Write out\n  covers: REQ-new1\n  verify: {verify}\n  files: out.txt\n"))
            .unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-new1 Out\nrvow SHALL write out.txt.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
        )
        .unwrap();
        fx.run(RVOW, &["accept", &fx.id, "--yes"]);
        // Accept moves the change into active/, so its path is read again.
        fx.change = PathBuf::from(field(&fx.run(RVOW, &["path", &fx.id]), "path"));
        fx.task = fx.tasks().remove(0);
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

    fn git(&self, args: &[&str]) {
        let out = self.cmd("git").args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// Runs a command in the repository and returns its stdout, asserting it
    /// succeeded.
    fn run(&self, program: &str, args: &[&str]) -> String {
        let out = self.cmd(program).args(args).output().unwrap();
        assert!(out.status.success(), "{program} {args:?}: {}", String::from_utf8_lossy(&out.stdout));
        String::from_utf8(out.stdout).unwrap()
    }

    /// Proves the fixture's task, which runs its verify once.
    fn prove(&self) {
        let out = self.run(RVOW, &["prove", &self.task]);
        assert!(out.contains("proved"), "{out}");
    }

    /// The work a proved task did, one line longer than what it was proved on,
    /// so its proof is stale.
    fn edit(&self) {
        std::fs::write(self.repo().join("out.txt"), "ok\nmore\n").unwrap();
    }

    /// The lines the fixture's verify wrote. A gate that runs nothing leaves it
    /// as it was.
    fn counter(&self) -> String {
        std::fs::read_to_string(self.root.join("counter")).unwrap_or_default()
    }

    /// Runs `rvow gate --push --human` with `refs` on stdin: its output, where
    /// each failure and each warning is one line, and its exit code.
    fn push(&self, refs: &str) -> (String, i32) {
        use std::io::Write;
        let mut child = self.cmd(RVOW).args(["gate", "--push", "--human"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(refs.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        (String::from_utf8(out.stdout).unwrap(), out.status.code().unwrap())
    }

    fn tasks(&self) -> Vec<String> {
        let text = std::fs::read_to_string(self.change.join("tasks.md")).unwrap();
        text.lines().filter_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).collect()
    }
}

/// One pre-push line: the local ref and its sha, then the remote ref and the sha
/// the remote holds.
fn line(local: &str, remote: &str) -> String {
    format!("{local} {SHA} {remote} {GONE}\n")
}

/// The value of `key` in a rvow reply.
fn field<'a>(out: &'a str, key: &str) -> &'a str {
    out.lines().find_map(|l| l.strip_prefix(&format!("{key}: "))).unwrap_or_else(|| panic!("no {key} in {out}"))
}

#[test]
fn push_gate_share_push_runs_nothing_and_warns() {
    let fx = Fx::new("share", "");
    fx.prove();
    let before = fx.counter();
    fx.edit();
    let (out, code) = fx.push(&line("refs/heads/feat/x", "refs/heads/feat/x"));
    assert_eq!(code, 0, "{out}");
    assert_eq!(fx.counter(), before, "the share push ran the task: {out}");
    assert!(out.contains(&format!("warning: {} is stale", fx.task)), "{out}");
    assert!(!out.contains("fail:"), "{out}");
    assert!(out.contains("share push of feat/x: 1 open items stay for rvow archive and CI"), "{out}");
}

#[test]
fn push_gate_land_push_of_unfinished_work_fails() {
    let fx = Fx::new("land", "");
    fx.prove();
    let before = fx.counter();
    fx.edit();
    let (out, code) = fx.push(&line("refs/heads/feat/x", "refs/heads/main"));
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(&format!("fail: {} is stale", fx.task)), "{out}");
    assert!(out.contains("gate failed"), "{out}");
    assert_eq!(fx.counter(), before, "the land push ran the task: {out}");
}

#[test]
fn push_gate_land_branches_setting_is_read() {
    let fx = Fx::new("branches", "land_branches = [\"release/*\"]\n");
    fx.prove();
    fx.edit();
    let (out, code) = fx.push(&line("refs/heads/feat/x", "refs/heads/release/1"));
    assert_eq!(code, 1, "release/1 matches release/*: {out}");
    assert!(out.contains("gate failed"), "{out}");
    let (out, code) = fx.push(&line("refs/heads/feat/x", "refs/heads/main"));
    assert_eq!(code, 0, "main is no land branch any more: {out}");
    assert!(out.contains("gate passed"), "{out}");
    assert!(out.contains(&format!("warning: {} is stale", fx.task)), "{out}");
}

#[test]
fn push_gate_tag_push_is_a_land_push() {
    let fx = Fx::new("tag", "");
    fx.prove();
    fx.edit();
    let (out, code) = fx.push(&line("refs/tags/v1", "refs/tags/v1"));
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(&format!("fail: {} is stale", fx.task)), "{out}");
}

#[test]
fn push_gate_archived_work_lands() {
    let fx = Fx::new("archived", "");
    std::fs::write(fx.repo().join("out.txt"), "ok").unwrap();
    fx.prove();
    fx.run(RVOW, &["archive", &fx.id]);
    let (out, code) = fx.push(&line("refs/heads/feat/x", "refs/heads/main"));
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("gate passed"), "{out}");
}

#[test]
fn push_gate_a_forged_ledger_fails_even_on_a_share_push() {
    let fx = Fx::new("forged", "");
    fx.prove();
    let ledger = fx.change.join("ledger.jsonl");
    let mut text = std::fs::read_to_string(&ledger).unwrap();
    text.push_str("garbage\n");
    std::fs::write(&ledger, text).unwrap();
    let (out, code) = fx.push(&line("refs/heads/feat/x", "refs/heads/feat/x"));
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("ledger.jsonl"), "{out}");
}

#[test]
fn push_gate_a_delete_is_ignored() {
    let fx = Fx::new("delete", "");
    let (out, code) = fx.push(&format!("(delete) {GONE} refs/heads/feat/x {SHA}\n"));
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("fail:"), "{out}");
}

#[test]
fn push_gate_runs_no_project_check() {
    let fx = Fx::new("checks", &format!("checks = [\"echo x >> {}\"]\n", counter_path("checks").display()));
    fx.prove();
    let before = fx.counter();
    let (out, code) = fx.push(&line("refs/heads/feat/x", "refs/heads/feat/x"));
    assert_eq!(code, 0, "{out}");
    assert_eq!(fx.counter(), before, "the push gate ran a project check: {out}");
}
