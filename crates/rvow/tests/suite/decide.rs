//! TSK-esdejz: `rvow decide` records the user's choice about a stale relaxed task.

use std::path::PathBuf;
use std::process::Command;

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
        let root = std::env::temp_dir().join(format!("rvow-decide-{}-{name}", std::process::id()));
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

    /// An accepted change with one task over `a.txt` and `b.txt`, proved and
    /// committed, marked `mode: relaxed` when `relaxed`. Returns its id and task id.
    fn proved(&self, relaxed: bool) -> (String, String) {
        let out = self.ok(&["new", "Out"]);
        let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        std::fs::write(dir.join("proposal.md"), "branch: main\n\n# Out\n\n## Why\nw\n\n## What changes\nw\n\n## Out of scope\nNone.\n")
            .unwrap();
        let mode = if relaxed { "  mode: relaxed\n" } else { "" };
        std::fs::write(
            dir.join("tasks.md"),
            format!("- TSK-new1 Write ok\n  verify: grep -q ok a.txt && {PASS}\n  files: a.txt, b.txt\n{mode}"),
        )
        .unwrap();
        self.ok(&["accept", &id, "--yes"]);
        let written = PathBuf::from(self.ok(&["path", &id]).lines().find_map(|l| l.strip_prefix("path: ")).unwrap()).join("tasks.md");
        let tasks = std::fs::read_to_string(written).unwrap();
        let task = tasks
            .lines()
            .find_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from))
            .expect("the tasks file names one task");
        std::fs::write(self.repo.join("a.txt"), "ok\n").unwrap();
        self.git(&["commit", "-q", "-am", "work"]);
        assert!(self.ok(&["prove", &task]).contains("result: proved"), "the task is proved");
        (id, task)
    }

    /// Makes the task stale with a committed edit of `b.txt`.
    fn stale(&self) {
        std::fs::write(self.repo.join("b.txt"), "b again\n").unwrap();
        self.git(&["commit", "-q", "-am", "stale"]);
    }

    /// The `rvow status` line of `task`.
    fn status_line(&self, task: &str) -> String {
        let status = self.ok(&["--human", "status"]);
        status.lines().find(|l| l.contains(task)).unwrap_or_else(|| panic!("no status line for {task}: {status}")).to_string()
    }
}

const PASS: &str = "echo 'test result: ok. 1 passed; 0 failed;'";

#[test]
fn decide_keep_makes_a_stale_relaxed_task_proved() {
    let r = Repo::new("keep");
    let (_id, task) = r.proved(true);
    r.stale();
    assert!(r.status_line(&task).contains("stale (inputs-changed, user-decides)"), "the task waits for the user");

    let out = r.ok(&["decide", &task, "keep"]);
    assert!(out.contains(&format!("decided: {task} keep")), "{out}");
    assert!(out.contains(&format!("next: the proof of {task} stands until its files change again")), "{out}");
    assert!(out.contains("pick: keep") && out.contains(&format!("task: {task}")), "{out}");
    assert!(r.status_line(&task).split_whitespace().any(|w| w == "proved"), "the kept proof counts: {}", r.status_line(&task));
}

#[test]
fn decide_refuses_a_strict_task() {
    let r = Repo::new("strict");
    let (_id, task) = r.proved(false);
    r.stale();
    let (out, err, code) = r.rvow(&["decide", &task, "keep"]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.contains("code: decide"), "{out}");
    assert!(out.contains(&task) && out.contains("strict"), "{out}");
    assert!(r.status_line(&task).contains("stale"), "the strict task stays stale");
}

#[test]
fn decide_list_names_the_waiting_task() {
    let r = Repo::new("list");
    let (_id, task) = r.proved(true);
    assert!(r.ok(&["decide", "--list"]).contains("undecided: 0 tasks wait for a decision"), "nothing waits yet");

    r.stale();
    let out = r.ok(&["decide", "--list"]);
    assert!(out.contains(&format!("undecided[1]{{id,title}}:\n  {task},Write ok")), "{out}");
    assert!(out.contains("Ask the user with the rvow_ask tool; each answer runs rvow decide <task> now|archive|keep"), "{out}");
}
