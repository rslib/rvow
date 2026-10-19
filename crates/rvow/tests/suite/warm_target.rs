//! The base commit baseline in the warm worktree builds into its own cargo
//! folder, so it never overwrites the build of the user's checkout.

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
        let root = std::env::temp_dir().join(format!("rvow-warm-target-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        let root = root.canonicalize().unwrap();
        let r = Repo { root: root.clone(), repo: root.join("repo"), home: root.join("home") };
        r.git(&["init", "-q", "-b", "main"]);
        std::fs::write(r.repo.join("src.txt"), "old\n").unwrap();
        r.git(&["add", "src.txt"]);
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

    fn ok(&self, args: &[&str]) -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_rvow"))
            .current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("RVOW_HOME", &self.home)
            .env("CARGO_TARGET_DIR", self.root.join("shared-target"))
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
        assert_eq!(out.status.code(), Some(0), "rvow {args:?}: {text}");
        text
    }

    /// Accepts a change of one task whose verify writes its folder and its
    /// CARGO_TARGET_DIR to `seen`, then passes once src.txt holds ok.
    fn accepted(&self) -> String {
        let seen = self.root.join("seen");
        let verify = format!(
            "echo \"$PWD|$CARGO_TARGET_DIR\" >> {}; grep -q ok src.txt && echo 'test result: ok. 1 passed; 0 failed;'",
            seen.display()
        );
        let out = self.ok(&["new", "Out"]);
        let id = out.lines().find_map(|l| l.strip_prefix("created: ")).unwrap().to_string();
        let dir = PathBuf::from(out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap());
        std::fs::write(dir.join("proposal.md"), "branch: main\n\n# Out\n\n## Why\nw\n\n## What changes\nw\n\n## Out of scope\nNone.\n")
            .unwrap();
        std::fs::write(dir.join("tasks.md"), format!("- TSK-new1 Write ok\n  verify: {verify}\n  files: src.txt\n")).unwrap();
        self.ok(&["accept", &id, "--yes"]);
        let tasks = std::fs::read_to_string(
            PathBuf::from(self.ok(&["path", &id]).lines().find_map(|l| l.strip_prefix("path: ")).unwrap()).join("tasks.md"),
        )
        .unwrap();
        tasks.lines().find_map(|l| l.strip_prefix("- ")?.split_whitespace().next().map(String::from)).unwrap()
    }

    /// The CARGO_TARGET_DIR of each recorded run, keyed by whether it ran in the warm worktree.
    fn runs(&self) -> (Vec<String>, Vec<String>) {
        let text = std::fs::read_to_string(self.root.join("seen")).unwrap_or_default();
        let warm = self.home.join("worktrees");
        let (mut in_warm, mut live) = (Vec::new(), Vec::new());
        for line in text.lines() {
            let (dir, target) = line.split_once('|').unwrap();
            if PathBuf::from(dir).starts_with(&warm) { in_warm.push(target.to_string()) } else { live.push(target.to_string()) }
        }
        (in_warm, live)
    }
}

fn assert_own_folder(r: &Repo, in_warm: &[String]) {
    assert_eq!(in_warm.len(), 1, "one base commit run: {in_warm:?}");
    let target = PathBuf::from(&in_warm[0]);
    assert!(target.starts_with(r.home.join("worktrees")), "the warm run builds next to the warm worktree: {target:?}");
    assert_ne!(target, r.root.join("shared-target"), "the warm run must not share the user's build folder");
}

#[test]
fn warm_target_the_base_commit_baseline_builds_into_its_own_folder() {
    let r = Repo::new("own");
    let task = r.accepted();
    std::fs::write(r.repo.join("src.txt"), "ok\n").unwrap();
    assert!(r.ok(&["prove", &task]).contains("result: proved"));
    let (in_warm, live) = r.runs();
    assert_own_folder(&r, &in_warm);
    assert_eq!(live, vec![r.root.join("shared-target").display().to_string()], "the run in the checkout keeps the user's folder");
}

#[test]
fn warm_target_rerun_env_does_not_reach_the_warm_worktree() {
    let r = Repo::new("rerun-env");
    let project = rvow_core::store::find(&r.repo, &r.home).unwrap().unwrap();
    std::fs::write(
        project.root.join("config.toml"),
        format!("rerun_env = {{ CARGO_TARGET_DIR = \"{}\" }}\n", r.root.join("shared-target").display()),
    )
    .unwrap();
    let task = r.accepted();
    std::fs::write(r.repo.join("src.txt"), "ok\n").unwrap();
    assert!(r.ok(&["prove", &task]).contains("result: proved"));
    assert_own_folder(&r, &r.runs().0);
}
