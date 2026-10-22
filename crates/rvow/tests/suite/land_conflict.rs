//! A conflicting task branch is left out, the others land, and land prints the
//! steps that rebase, resolve, prove and land it again (REQ-n5wc8g).

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
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn field<'a>(out: &'a str, key: &str) -> &'a str {
    out.lines().find_map(|l| l.strip_prefix(&format!("{key}: "))).unwrap_or_else(|| panic!("no {key} in {out}"))
}

impl Fx {
    /// A repository on `feat/a` with an accepted change of two tasks that both
    /// rewrite line 1 of `a.txt`, and prove when their own line is there.
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-land-conflict-{}-{name}", std::process::id()));
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
        std::fs::write(
            dir.join("tasks.md"),
            format!(
                "- TSK-new1 First\n  covers: REQ-new1\n  verify: grep -q one a.txt && {PASS}\n  files: a.txt\n\n- TSK-new2 Second\n  covers: REQ-new1\n  verify: grep -q two a.txt && {PASS}\n  files: a.txt\n"
            ),
        )
        .unwrap();
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

    fn build(&self, task: &str, line: &str, message: &str) {
        let out = self.rvow(&self.repo, &["delegate", task, "--worktree"]);
        assert!(out.contains("branch: "), "delegate {task}: {out}");
        let wt = self.worktree(task);
        std::fs::write(wt.join("a.txt"), format!("{line}\n")).unwrap();
        self.git(&wt, &["add", "."]);
        self.git(&wt, &["commit", "-q", "-m", message]);
        let proved = self.rvow(&wt, &["prove", task]);
        assert!(proved.contains("proved"), "prove {task}: {proved}");
    }
}

#[test]
fn land_leaves_a_conflict_out_and_prints_the_steps_to_fix_it() {
    let fx = Fx::new("one");
    fx.build(&fx.first, "one", "one");
    fx.build(&fx.second, "two", "two");
    let (out, code) = fx.run(&["land", &fx.change]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(fx.git(&fx.repo, &["log", "--format=%s", "-1", "feat/a"]), "one", "the clean task landed: {out}");
    assert!(out.contains(&fx.second), "names the conflicting task: {out}");
    assert!(out.contains("a.txt"), "names the conflicting file: {out}");
    assert!(out.contains("rebase feat/a"), "names the rebase onto the change branch: {out}");
    assert!(out.contains("prove") && out.contains("land"), "names the steps after the rebase: {out}");
}
