//! `rvow land --merge` lands each task branch as one merge commit (REQ-3ypb5j).

use std::path::{Path, PathBuf};
use std::process::Command;

const RVOW: &str = env!("CARGO_BIN_EXE_rvow");
const PASS: &str = "echo 'test result: ok. 1 passed; 0 failed;'";
const SPEC: &str = "## ADD REQ-new1 One\nrvow SHALL land one.\n### Scenario: s\n- WHEN it runs\n- THEN it lands\n";

struct Fx {
    root: PathBuf,
    repo: PathBuf,
    change: String,
    task: String,
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
    fn new(name: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("rvow-land-merge-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        let root = root.canonicalize().unwrap();
        let mut fx = Fx { repo: root.join("repo"), root, change: String::new(), task: String::new() };
        let repo = fx.repo.clone();
        std::fs::write(repo.join("a.txt"), "base\n").unwrap();
        fx.git(&repo, &["init", "-q", "-b", "main"]);
        fx.git(&repo, &["add", "."]);
        fx.git(&repo, &["commit", "-q", "-m", "init"]);
        fx.git(&repo, &["checkout", "-q", "-b", "feat/a"]);

        fx.rvow(&repo, &["init"]);
        let out = fx.rvow(&repo, &["new", "--spec", "One"]);
        let dir = PathBuf::from(field(&out, "path"));
        std::fs::write(
            dir.join("tasks.md"),
            format!(
                "- TSK-new1 One\n  covers: REQ-new1\n  verify: test -f one.txt && test -f two.txt && {PASS}\n  files: one.txt, two.txt\n"
            ),
        )
        .unwrap();
        std::fs::write(dir.join("specs/out.md"), SPEC).unwrap();
        fx.change = field(&out, "created").to_owned();
        fx.rvow(&repo, &["accept", &fx.change.clone(), "--yes"]);
        let located = fx.rvow(&repo, &["path", &fx.change.clone()]);
        let text = std::fs::read_to_string(PathBuf::from(field(&located, "path")).join("tasks.md")).unwrap();
        fx.task = text.lines().find_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).unwrap();
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

    fn worktree(&self) -> PathBuf {
        self.root.join("repo.worktrees").join(&self.task)
    }
}

#[test]
fn land_merge_makes_one_merge_commit_whose_second_parent_is_the_task_branch() {
    let fx = Fx::new("merge");
    fx.rvow(&fx.repo, &["delegate", &fx.task, "--worktree"]);
    let wt = fx.worktree();
    std::fs::write(wt.join("one.txt"), "one\n").unwrap();
    fx.git(&wt, &["add", "."]);
    fx.git(&wt, &["commit", "-q", "-m", "one"]);
    std::fs::write(wt.join("two.txt"), "two\n").unwrap();
    fx.git(&wt, &["add", "."]);
    fx.git(&wt, &["commit", "-q", "-m", "two"]);
    let proved = fx.rvow(&wt, &["prove", &fx.task]);
    assert!(proved.contains("proved"), "{proved}");
    let task_tip = fx.git(&wt, &["rev-parse", "HEAD"]);

    let (out, code) = fx.run(&["land", &fx.change, "--merge"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains(&fx.task), "{out}");
    let parents = fx.git(&fx.repo, &["rev-list", "--parents", "-n", "1", "feat/a"]);
    let parents: Vec<&str> = parents.split_whitespace().collect();
    assert_eq!(parents.len(), 3, "the tip is a merge commit: {out}");
    assert_eq!(parents[2], task_tip, "the second parent is the tip of the task branch");
}
