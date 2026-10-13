use std::path::PathBuf;
use std::process::Command;

use rvow_core::{store, view};

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    project: store::Project,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("rvow-audit-view-{}-{name}", std::process::id()));
        let repo = root.join("repo");
        let home = root.join("home");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(&repo)
                .args(args)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .env_remove("GIT_COMMON_DIR")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
                .output()
                .unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q", "-b", "trunk"]);
        std::fs::write(repo.join("initial.txt"), "initial").unwrap();
        git(&["add", "initial.txt"]);
        git(&["commit", "-q", "-m", "initial"]);
        let project = store::init(&repo, &home, store::Mode::Home).unwrap();
        Self { root, repo, home, project }
    }

    fn cli(&self, mode: &str) -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_rvow"))
            .args(["view", mode])
            .current_dir(&self.repo)
            .env("HOME", &self.root)
            .env("RVOW_HOME", &self.home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
        String::from_utf8(out.stdout).unwrap()
    }
}

#[test]
fn audit_view_empty_dashboard_names_branch_and_empty_lists() {
    let f = Fixture::new("empty");
    for mode in ["--human", "--toon"] {
        let out = f.cli(mode);
        assert!(out.contains("trunk"), "{out}");
        for name in ["changes", "tasks", "specs", "backlog", "archive"] {
            assert!(out.contains(name) && out.contains('0'), "{name}: {out}");
        }
    }
}

#[test]
fn audit_view_both_outputs_include_shared_living_spec_identity() {
    let f = Fixture::new("specs");
    let spec = f.project.root.join("specs/cap/spec.md");
    std::fs::create_dir_all(spec.parent().unwrap()).unwrap();
    std::fs::write(&spec, "# Cap\n\n## REQ-q8w2c7 Shared identity\nrvow SHALL stay.\n### Scenario: s\n- WHEN a\n- THEN b\n").unwrap();
    for mode in ["--human", "--toon"] {
        let out = f.cli(mode);
        assert!(out.contains("REQ-q8w2c7") && out.contains("Shared identity"), "{out}");
    }
    let shared = view::view(&f.project, &f.home, &f.repo).unwrap().unwrap();
    assert_eq!(shared["specs"][0]["id"], "REQ-q8w2c7");
    let dashboard = view::dashboard(&f.project, &f.home, &f.repo).unwrap();
    assert_eq!(dashboard, shared);
}
