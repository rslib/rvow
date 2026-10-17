//! REQ-8jba0p: the base commit baseline and the gate rerun reuse one warm worktree per project.

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

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
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
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

impl Repo {
    fn new(name: &str) -> Repo {
        let root = std::env::temp_dir().join(format!("rvow-warm-worktree-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        let root = root.canonicalize().unwrap();
        let r = Repo { root: root.clone(), repo: root.join("repo"), home: root.join("home") };
        git(&r.repo, &["init", "-q", "-b", "main"]);
        std::fs::write(r.repo.join("a.txt"), "old\n").unwrap();
        std::fs::write(r.repo.join("b.txt"), "old\n").unwrap();
        git(&r.repo, &["add", "."]);
        git(&r.repo, &["commit", "-q", "-m", "init"]);
        r.ok(&["init"]);
        r
    }

    fn ok(&self, args: &[&str]) -> String {
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
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success(), "rvow {args:?}: {text}");
        text
    }
}

/// REQ-8jba0p: two base commit baselines in one project run in one warm
/// worktree at a fixed path under the rvow home, which stays registered.
#[test]
fn warm_worktree_serves_every_base_commit_baseline_of_a_project() {
    let r = Repo::new("two");
    let out = r.ok(&["new", "Two"]);
    let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
    let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
    std::fs::write(dir.join("proposal.md"), "branch: main\n\n# Two\n\n## Why\nw\n\n## What changes\nw\n\n## Out of scope\nNone.\n")
        .unwrap();
    let check = |f: &str| format!("grep -q ok {f} && echo 'test result: ok. 1 passed; 0 failed;'");
    std::fs::write(
        dir.join("tasks.md"),
        format!("- TSK-new1 A\n  verify: {}\n  files: a.txt\n- TSK-new2 B\n  verify: {}\n  files: b.txt\n", check("a.txt"), check("b.txt")),
    )
    .unwrap();
    r.ok(&["accept", &id, "--yes"]);
    let tasks = std::fs::read_to_string(
        PathBuf::from(r.ok(&["path", &id]).lines().find_map(|l| l.strip_prefix("path: ")).unwrap()).join("tasks.md"),
    )
    .unwrap();
    let ids: Vec<String> = tasks.lines().filter_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).collect();
    // The work came first for both tasks, so each prove runs a base commit baseline.
    std::fs::write(r.repo.join("a.txt"), "ok\n").unwrap();
    std::fs::write(r.repo.join("b.txt"), "ok\n").unwrap();
    for task in &ids {
        assert!(r.ok(&["prove", task]).contains("result: proved"));
    }
    let warm: Vec<PathBuf> =
        std::fs::read_dir(r.home.join("worktrees")).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    assert_eq!(warm.len(), 1, "one warm worktree for the project: {warm:?}");
    let list = git(&r.repo, &["worktree", "list"]);
    assert_eq!(list.lines().count(), 2, "the main checkout and the warm tree, no temporary tree left: {list}");
}
