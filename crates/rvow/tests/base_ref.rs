//! The vow and the gate use the newer merge base of the local and the remote
//! base branch (CHG-zjwww4). Each test drives the built binary in a throwaway
//! repository with a throwaway file-backed origin.

use std::path::{Path, PathBuf};
use std::process::Command;

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
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-baseref-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Fx { root: root.canonicalize().unwrap() }
    }

    fn env(&self, c: &mut Command) {
        c.env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.root.join("rh"))
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("PI_CODING_AGENT_DIR")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t");
    }

    fn git(&self, dir: &Path, args: &[&str]) {
        let mut c = Command::new("git");
        c.current_dir(dir);
        self.env(&mut c);
        let out = c.args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn rev(&self, dir: &Path, rev: &str) -> String {
        let mut c = Command::new("git");
        c.current_dir(dir);
        self.env(&mut c);
        let out = c.args(["rev-parse", rev]).output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn rvow(&self, dir: &Path, args: &[&str]) -> String {
        let mut c = Command::new(RVOW);
        c.current_dir(dir);
        self.env(&mut c);
        let out = c.args(args).output().unwrap();
        assert!(out.status.success(), "rvow {args:?}: {}", String::from_utf8_lossy(&out.stdout));
        String::from_utf8(out.stdout).unwrap()
    }

    fn commit(&self, dir: &Path, name: &str) {
        std::fs::write(dir.join(name), name).unwrap();
        self.git(dir, &["add", name]);
        self.git(dir, &["commit", "-q", "-m", name]);
    }

    /// A work repository whose `main` is pushed to a file-backed origin.
    fn pushed(&self) -> PathBuf {
        let origin = self.root.join("origin");
        self.git(&self.root, &["init", "--bare", "-q", "origin"]);
        let work = self.root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        self.git(&work, &["init", "-q", "-b", "main"]);
        self.commit(&work, "a.txt");
        self.git(&work, &["remote", "add", "origin", origin.to_str().unwrap()]);
        self.git(&work, &["push", "-q", "-u", "origin", "main"]);
        work.canonicalize().unwrap()
    }

    /// One accepted change with one task; returns the change id and task id.
    fn accepted(&self, work: &Path) -> (String, String) {
        self.rvow(work, &["init"]);
        let out = self.rvow(work, &["new", "--spec", "Out"]);
        let field = |k: &str| out.lines().find_map(|l| l.strip_prefix(&format!("{k}: "))).unwrap().to_string();
        let (id, dir) = (field("created"), PathBuf::from(field("path")));
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
        self.rvow(work, &["accept", &id, "--yes"]);
        let brief = self.rvow(work, &["brief", "--human"]);
        let at = brief.find("TSK-").unwrap();
        (id, brief[at..at + 10].to_string())
    }

    /// The base commit the vow recorded for a change.
    fn vow_base(&self, work: &Path, change: &str) -> String {
        let out = self.rvow(work, &["path", change]);
        let dir = out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap().trim().to_string();
        for line in std::fs::read_to_string(Path::new(&dir).join("ledger.jsonl")).unwrap().lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            if v.get("kind").and_then(|k| k.as_str()) == Some("vow") {
                return v.get("base_commit").and_then(|b| b.as_str()).unwrap().to_string();
            }
        }
        panic!("no vow event for {change}");
    }

    fn prove_task(&self, work: &Path, task: &str) {
        std::fs::write(work.join("out.txt"), "ok\n").unwrap();
        self.rvow(work, &["prove", task]);
    }
}

#[test]
fn base_ref_local_ahead_gate_passes_and_vow_records_local() {
    let fx = Fx::new("local-ahead");
    let work = fx.pushed();
    fx.commit(&work, "b.txt");
    fx.git(&work, &["checkout", "-q", "-b", "job"]);
    let (change, task) = fx.accepted(&work);
    assert_eq!(fx.vow_base(&work, &change), fx.rev(&work, "main"), "the vow records the local merge base");
    fx.prove_task(&work, &task);
    let out = fx.rvow(&work, &["gate"]);
    assert!(!out.contains("b.txt"), "no file of the unpushed main commit is named: {out}");
}

#[test]
fn base_ref_remote_ahead_vow_records_remote() {
    let fx = Fx::new("remote-ahead");
    let work = fx.pushed();
    fx.commit(&work, "c.txt");
    fx.git(&work, &["push", "-q", "origin", "main"]);
    fx.git(&work, &["reset", "-q", "--hard", "HEAD~1"]);
    fx.git(&work, &["checkout", "-q", "-b", "job", "origin/main"]);
    let (change, task) = fx.accepted(&work);
    assert_eq!(fx.vow_base(&work, &change), fx.rev(&work, "origin/main"), "the vow records the remote merge base");
    fx.prove_task(&work, &task);
    fx.rvow(&work, &["gate"]);
}

#[test]
fn base_ref_only_local_uses_local() {
    let fx = Fx::new("only-local");
    let work = fx.root.join("solo");
    std::fs::create_dir_all(&work).unwrap();
    fx.git(&work, &["init", "-q", "-b", "main"]);
    fx.commit(&work, "a.txt");
    let (change, task) = fx.accepted(&work);
    assert_eq!(fx.vow_base(&work, &change), fx.rev(&work, "main"));
    fx.prove_task(&work, &task);
    fx.rvow(&work, &["gate"]);
}

#[test]
fn base_ref_older_vow_passes_without_backwards_report() {
    let fx = Fx::new("older-vow");
    let work = fx.pushed();
    fx.git(&work, &["checkout", "-q", "-b", "job"]);
    let (_, task) = fx.accepted(&work);
    fx.prove_task(&work, &task);
    fx.git(&work, &["checkout", "-q", "main"]);
    fx.commit(&work, "d.txt");
    fx.git(&work, &["checkout", "-q", "job"]);
    fx.git(&work, &["merge", "-q", "main"]);
    let out = fx.rvow(&work, &["gate"]);
    assert!(!out.contains("moved backwards"), "an older vow still passes: {out}");
}
