//! REQ-serykj: `rvow archive` proves stale tasks again before its gate.

use std::path::PathBuf;
use std::process::Command;

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

impl Repo {
    fn new(name: &str) -> Repo {
        let root = std::env::temp_dir().join(format!("rvow-archive-reprove-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        let root = root.canonicalize().unwrap();
        let r = Repo { root: root.clone(), repo: root.join("repo"), home: root.join("home") };
        r.git(&["init", "-q", "-b", "main"]);
        std::fs::write(r.repo.join("a.txt"), "no\n").unwrap();
        std::fs::write(r.repo.join("b.txt"), "b\n").unwrap();
        r.git(&["add", "."]);
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

    fn rvow(&self, args: &[&str]) -> (String, String, i32) {
        let out = Command::new(env!("CARGO_BIN_EXE_rvow"))
            .current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("RVOW_HOME", &self.home)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .output()
            .unwrap();
        (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap())
    }

    fn ok(&self, args: &[&str]) -> String {
        let (out, err, code) = self.rvow(args);
        assert_eq!(code, 0, "rvow {args:?}: {out}{err}");
        out
    }

    /// An accepted change whose `tasks` are each `(title, verify)` over `files: a.txt, b.txt`,
    /// with every task proved and its work committed. Returns the change id and the task ids.
    fn proved(&self, tasks: &[(&str, &str)]) -> (String, Vec<String>) {
        let out = self.ok(&["new", "Out"]);
        let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        std::fs::write(dir.join("proposal.md"), "branch: main\n\n# Out\n\n## Why\nw\n\n## What changes\nw\n\n## Out of scope\nNone.\n")
            .unwrap();
        let text: String = tasks
            .iter()
            .enumerate()
            .map(|(i, (title, verify))| format!("- TSK-new{} {title}\n  verify: {verify}\n  files: a.txt, b.txt\n", i + 1))
            .collect();
        std::fs::write(dir.join("tasks.md"), text).unwrap();
        self.ok(&["accept", &id, "--yes"]);
        let tasks = std::fs::read_to_string(
            PathBuf::from(self.ok(&["path", &id]).lines().find_map(|l| l.strip_prefix("path: ")).unwrap()).join("tasks.md"),
        )
        .unwrap();
        let ids: Vec<String> = tasks.lines().filter_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).collect();
        std::fs::write(self.repo.join("a.txt"), "ok\n").unwrap();
        self.git(&["commit", "-q", "-am", "work"]);
        for task in &ids {
            assert!(self.ok(&["prove", task]).contains("result: proved"));
        }
        (id, ids)
    }

    /// Makes every task stale with a committed edit of `b.txt`.
    fn stale(&self) {
        std::fs::write(self.repo.join("b.txt"), "b again\n").unwrap();
        self.git(&["commit", "-q", "-am", "stale"]);
        assert!(self.ok(&["--human", "status"]).contains("stale"));
    }

    fn log(&self) -> PathBuf {
        self.root.join("runs.log")
    }
}

const PASS: &str = "echo 'test result: ok. 1 passed; 0 failed;'";

#[test]
fn archive_reproves_stale_tasks_that_still_pass() {
    let r = Repo::new("pass");
    let check = format!("grep -q ok a.txt && {PASS}");
    let (id, ids) = r.proved(&[("Write ok", &check)]);
    r.stale();
    let (out, err, code) = r.rvow(&["archive", &id]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains(&format!("reproved[1]:\n  {}", ids[0])), "the result names the task: {out}");
    assert!(err.contains(&format!("re-prove: {}", ids[0])), "the echo names the task while it runs: {err}");
    assert!(out.contains("archived"), "{out}");
}

#[test]
fn archive_reproves_stale_tasks_and_stops_on_a_failure() {
    let r = Repo::new("fail");
    let check = format!("grep -q ok a.txt && grep -q '^b$' b.txt && {PASS}");
    let (id, ids) = r.proved(&[("Write ok", &check)]);
    r.stale();
    let (out, err, code) = r.rvow(&["archive", &id]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.contains("code: gate"), "{out}");
    assert!(out.contains(&ids[0]) && out.contains("exit 1"), "the message names the task and its failure: {out}");
    assert!(r.ok(&["path", &id]).contains("/active/"), "the change stays active");
}

#[test]
fn archive_reproves_stale_tasks_only_when_some_are_stale() {
    let r = Repo::new("none");
    let check = format!("grep -q ok a.txt && echo run >> {} && {PASS}", r.log().display());
    let (id, _ids) = r.proved(&[("One", &check), ("Two", &check)]);
    let before = std::fs::read_to_string(r.log()).unwrap_or_default().lines().count();
    let (out, err, code) = r.rvow(&["archive", &id]);
    assert_eq!(code, 0, "{out}{err}");
    let after = std::fs::read_to_string(r.log()).unwrap_or_default().lines().count();
    assert_eq!(before, after, "no task verify ran at archive");
}
