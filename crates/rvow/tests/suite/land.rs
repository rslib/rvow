//! `rvow land` applies the proved task branches of a change onto its branch in
//! a temporary worktree, re-proves the moved tasks and gates the result
//! (REQ-cdj5sx, REQ-6a2hvm, REQ-mkg0mg).

use std::path::{Path, PathBuf};
use std::process::Command;

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");
const PASS: &str = "echo 'test result: ok. 1 passed; 0 failed;'";
const SPEC: &str = "## ADD REQ-new1 Two\nrvow SHALL do both.\n### Scenario: s\n- WHEN it runs\n- THEN both are done\n";

struct Fx {
    root: PathBuf,
    repo: PathBuf,
    change: String,
    first: String,
    second: String,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = Command::new("git").args(["worktree", "prune"]).current_dir(&self.repo).output();
        rvow_core::git::testrepo::remove_tree(&self.root);
    }
}

fn field<'a>(out: &'a str, key: &str) -> &'a str {
    out.lines().find_map(|l| l.strip_prefix(&format!("{key}: "))).unwrap_or_else(|| panic!("no {key} in {out}"))
}

impl Fx {
    /// A repository on `feat/a` with an accepted change of two tasks, written
    /// from `tasks` (a tasks.md body), and a base `a.txt` both tasks can read.
    fn new(name: &str, tasks: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-land-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        let root = root.canonicalize().unwrap();
        let mut fx = Fx { repo: root.join("repo"), root, change: String::new(), first: String::new(), second: String::new() };
        let repo = fx.repo.clone();
        std::fs::write(repo.join("a.txt"), "base\n").unwrap();
        fx.git(&repo, &["init", "-q", "-b", "main"]);
        fx.git(&repo, &["add", "."]);
        fx.git(&repo, &["commit", "-q", "-m", "init"]);
        fx.git(&repo, &["checkout", "-q", "-b", "feat/a"]);

        fx.rvow(&repo, &["init"]);
        let out = fx.rvow(&repo, &["new", "--spec", "Two"]);
        let dir = PathBuf::from(field(&out, "path"));
        std::fs::write(dir.join("tasks.md"), tasks).unwrap();
        std::fs::write(dir.join("specs/out.md"), SPEC).unwrap();
        fx.change = field(&out, "created").to_owned();
        fx.rvow(&repo, &["accept", &fx.change.clone(), "--yes"]);
        let located = fx.rvow(&repo, &["path", &fx.change.clone()]);
        let text = std::fs::read_to_string(PathBuf::from(field(&located, "path")).join("tasks.md")).unwrap();
        let ids: Vec<String> = text.lines().filter_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).collect();
        fx.first = ids[0].clone();
        fx.second = ids[1].clone();
        fx
    }

    fn env(&self, c: &mut Command, dir: &Path) {
        // No `rgit` on this PATH, so the writes go through git.
        c.current_dir(dir)
            .env("HOME", self.root.join("home"))
            .env("RVOW_HOME", self.root.join("home"))
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env_remove("RVOW_GIT")
            .env_remove("EVAL_RVOW_GIT")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t");
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let mut c = Command::new("git");
        self.env(&mut c, dir);
        let out = c.args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn rvow(&self, dir: &Path, args: &[&str]) -> String {
        let mut c = Command::new(RVOW);
        self.env(&mut c, dir);
        let out = c.args(args).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Runs rvow in the repository: stdout and exit code.
    fn run(&self, args: &[&str]) -> (String, i32) {
        let mut c = Command::new(RVOW);
        let repo = self.repo.clone();
        self.env(&mut c, &repo);
        let out = c.args(args).output().unwrap();
        (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap())
    }

    fn worktree(&self, task: &str) -> PathBuf {
        self.root.join("repo.worktrees").join(task)
    }

    /// Leases `task` in a worktree, writes `files` there, commits and proves it.
    fn build(&self, task: &str, files: &[(&str, &str)], message: &str) {
        let out = self.rvow(&self.repo, &["delegate", task, "--worktree"]);
        assert!(out.contains("branch: "), "delegate {task}: {out}");
        let wt = self.worktree(task);
        for (path, body) in files {
            std::fs::write(wt.join(path), body).unwrap();
        }
        self.git(&wt, &["add", "."]);
        self.git(&wt, &["commit", "-q", "-m", message]);
        let proved = self.rvow(&wt, &["prove", task]);
        assert!(proved.contains("proved"), "prove {task}: {proved}");
    }

    fn tip(&self) -> String {
        self.git(&self.repo, &["rev-parse", "feat/a"])
    }
}

#[test]
fn land_applies_both_task_branches_in_task_order() {
    let fx = Fx::new(
        "both",
        &format!(
            "- TSK-new1 First\n  covers: REQ-new1\n  verify: grep -q one a.txt && {PASS}\n  files: a.txt\n\n- TSK-new2 Second\n  covers: REQ-new1\n  verify: grep -q two b.txt && {PASS}\n  files: b.txt\n"
        ),
    );
    let before = fx.tip();
    fx.build(&fx.first, &[("a.txt", "one\n")], "one");
    fx.build(&fx.second, &[("b.txt", "two\n")], "two");
    let (out, code) = fx.run(&["land", &fx.change]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains(&fx.first) && out.contains(&fx.second), "{out}");
    assert_ne!(fx.tip(), before, "the change branch moved");
    assert_eq!(fx.git(&fx.repo, &["log", "--format=%s", "-2", "feat/a"]), "two\none", "task order");
    assert_eq!(std::fs::read_to_string(fx.repo.join("a.txt")).unwrap(), "one\n", "the checkout carries the result");
    assert_eq!(std::fs::read_to_string(fx.repo.join("b.txt")).unwrap(), "two\n");
}

#[test]
fn land_one_task_moves_only_its_branch() {
    let fx = Fx::new(
        "one",
        &format!(
            "- TSK-new1 First\n  covers: REQ-new1\n  verify: grep -q one a.txt && {PASS}\n  files: a.txt\n\n- TSK-new2 Second\n  covers: REQ-new1\n  verify: grep -q two b.txt && {PASS}\n  files: b.txt\n"
        ),
    );
    fx.build(&fx.first, &[("a.txt", "one\n")], "one");
    fx.build(&fx.second, &[("b.txt", "two\n")], "two");
    let (out, code) = fx.run(&["land", &fx.second]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains(&fx.second), "{out}");
    assert!(!out.contains(&fx.first), "{out}");
    assert_eq!(fx.git(&fx.repo, &["log", "--format=%s", "-1", "feat/a"]), "two");
    assert!(!fx.git(&fx.repo, &["log", "--format=%s", "feat/a"]).contains("one"), "the other task stays out");
}

#[test]
fn land_with_nothing_to_land_moves_nothing() {
    let fx = Fx::new(
        "none",
        &format!(
            "- TSK-new1 First\n  covers: REQ-new1\n  verify: grep -q one a.txt && {PASS}\n  files: a.txt\n\n- TSK-new2 Second\n  covers: REQ-new1\n  verify: grep -q two b.txt && {PASS}\n  files: b.txt\n"
        ),
    );
    let before = fx.tip();
    let (out, code) = fx.run(&["land", &fx.change]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("nothing: true"), "{out}");
    assert_eq!(fx.tip(), before, "the change branch stays put");
}

#[test]
fn land_a_verify_that_fails_after_the_merge_moves_nothing() {
    let fx = Fx::new(
        "fails",
        &format!(
            "- TSK-new1 First\n  covers: REQ-new1\n  verify: grep -q base a.txt && test -f b.txt && {PASS}\n  files: a.txt, b.txt\n\n- TSK-new2 Second\n  covers: REQ-new1\n  verify: grep -q changed a.txt && {PASS}\n  files: a.txt\n"
        ),
    );
    let before = fx.tip();
    fx.build(&fx.first, &[("b.txt", "b\n")], "first");
    fx.build(&fx.second, &[("a.txt", "changed\n")], "second");
    let (out, code) = fx.run(&["land", &fx.change]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(&fx.first), "names the task whose verify failed: {out}");
    assert_eq!(fx.tip(), before, "the change branch stays put");
}

#[test]
fn land_a_dirty_checkout_of_the_change_branch_fails_before_any_work() {
    let fx = Fx::new(
        "dirty",
        &format!(
            "- TSK-new1 First\n  covers: REQ-new1\n  verify: grep -q one a.txt && {PASS}\n  files: a.txt\n\n- TSK-new2 Second\n  covers: REQ-new1\n  verify: grep -q two b.txt && {PASS}\n  files: b.txt\n"
        ),
    );
    fx.build(&fx.first, &[("a.txt", "one\n")], "one");
    fx.build(&fx.second, &[("b.txt", "two\n")], "two");
    std::fs::write(fx.repo.join("a.txt"), "dirty\n").unwrap();
    let before = fx.tip();
    let (out, code) = fx.run(&["land", &fx.change]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(&fx.repo.display().to_string()), "names the worktree: {out}");
    assert_eq!(fx.tip(), before, "nothing moved");
    assert!(!fx.repo.join("b.txt").exists(), "no file of a task landed");
}

#[test]
fn land_again_after_a_land_finds_nothing_and_no_conflict() {
    let fx = Fx::new(
        "again",
        &format!(
            "- TSK-new1 First\n  covers: REQ-new1\n  verify: grep -q one a.txt && {PASS}\n  files: a.txt\n\n- TSK-new2 Second\n  covers: REQ-new1\n  verify: grep -q two b.txt && {PASS}\n  files: b.txt\n"
        ),
    );
    fx.build(&fx.first, &[("a.txt", "one\n")], "one");
    fx.build(&fx.second, &[("b.txt", "two\n")], "two");
    let (out, code) = fx.run(&["land", &fx.change]);
    assert_eq!(code, 0, "{out}");
    let landed = fx.tip();
    let (out, code) = fx.run(&["land", &fx.change]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("nothing: true") && !out.contains("conflict"), "a landed branch is not picked again: {out}");
    assert_eq!(fx.tip(), landed);
}

#[test]
fn land_reports_a_task_branch_that_is_gone() {
    let fx = Fx::new(
        "gone",
        &format!(
            "- TSK-new1 First\n  covers: REQ-new1\n  verify: grep -q one a.txt && {PASS}\n  files: a.txt\n\n- TSK-new2 Second\n  covers: REQ-new1\n  verify: grep -q two b.txt && {PASS}\n  files: b.txt\n"
        ),
    );
    fx.build(&fx.first, &[("a.txt", "one\n")], "one");
    let branch = format!("rvow/{}/{}", fx.change, fx.first);
    let wt = fx.worktree(&fx.first);
    fx.git(&fx.repo, &["worktree", "remove", "--force", &wt.to_string_lossy()]);
    fx.git(&fx.repo, &["branch", "-D", &branch]);
    let before = fx.tip();
    let (out, code) = fx.run(&["land", &fx.change]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(&fx.first) && out.contains("gone"), "{out}");
    assert_eq!(fx.tip(), before);
}
