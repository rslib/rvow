use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rvow_core::{config, store};

struct Fixture {
    dir: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    project: store::Project,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        rvow_core::git::testrepo::remove_tree(&self.dir);
    }
}

fn git_command(repo: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(repo)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid");
    command
}

impl Fixture {
    fn new(name: &str, config: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rvow-fix-gate-{}-{name}", std::process::id()));
        let repo = dir.join("repo");
        let home = dir.join("home");
        std::fs::create_dir_all(&repo).unwrap();
        let fixture_git = |args: &[&str]| {
            let output = git_command(&repo).args(args).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        };
        fixture_git(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("initial.txt"), "initial").unwrap();
        fixture_git(&["add", "initial.txt"]);
        fixture_git(&["commit", "-q", "-m", "initial"]);
        let project = store::init(&repo, &home, store::Mode::Home).unwrap();
        std::fs::write(project.root.join("config.toml"), config).unwrap();
        Self { dir, repo, home, project }
    }

    fn cli(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_rvow"))
            .args(args)
            .current_dir(cwd)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env("HOME", &self.dir)
            .env("RVOW_HOME", &self.home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap()
    }
}

#[test]
fn gate_out_requires_rerun_or_bundle() {
    let f = Fixture::new("out-needs-rerun", "");
    let out = f.dir.join("runs.jsonl");
    let result = f.cli(&f.repo, &["gate", "--out", out.to_str().unwrap()]);
    assert_eq!(result.status.code(), Some(2), "{}", String::from_utf8_lossy(&result.stdout));
    assert!(!out.exists());
}

#[test]
fn ingest_of_an_empty_file_is_an_error() {
    let f = Fixture::new("ingest-empty", "");
    let file = f.dir.join("empty.jsonl");
    std::fs::write(&file, "").unwrap();
    let result = f.cli(&f.repo, &["ingest", file.to_str().unwrap()]);
    let text = String::from_utf8_lossy(&result.stdout);
    assert_eq!(result.status.code(), Some(1), "{text}");
    assert!(text.contains("rvow gate --rerun --out <file>"), "{text}");
}

#[test]
fn interview_cap_is_one_to_three() {
    let f = Fixture::new("cap", "");
    for cap in [1, 2, 3] {
        std::fs::write(f.project.root.join("config.toml"), format!("interview_cap = {cap}\n")).unwrap();
        assert_eq!(config::load(&f.project, &f.home).unwrap().interview_cap, cap);
    }
    std::fs::write(f.project.root.join("config.toml"), "interview_cap = 4\n").unwrap();
    let err = config::load(&f.project, &f.home).unwrap_err();
    assert!(err.fix.contains("interview_cap"), "{}", err.fix);
    let help = f.cli(&f.repo, &["clarify", "--help"]);
    let text = String::from_utf8_lossy(&help.stdout);
    assert!(text.contains("interview_cap") && !text.contains("at most 4"), "{text}");
}
