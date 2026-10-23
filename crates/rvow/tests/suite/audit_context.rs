use std::path::PathBuf;
use std::process::Command;

use rvow_core::{config, guide, store};

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    project: store::Project,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        rvow_core::git::testrepo::remove_tree(&self.root);
    }
}
impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("rvow-audit-context-{}-{name}", std::process::id()));
        let repo = root.join("repo");
        let home = root.join("home");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&repo)
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
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("initial"), "initial").unwrap();
        git(&["add", "initial"]);
        git(&["commit", "-q", "-m", "initial"]);
        let project = store::init(&repo, &home, store::Mode::Home).unwrap();
        Self { root, repo, home, project }
    }
    fn guide(&self, artifact: &str) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_rvow"))
            .args(["guide", artifact, "--human"])
            .current_dir(&self.repo)
            .env("HOME", &self.root)
            .env("RVOW_HOME", &self.home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
        String::from_utf8(output.stdout).unwrap()
    }
}

#[test]
fn audit_context_guide_appends_context_and_only_the_selected_rules() {
    let f = Fixture::new("guides");
    std::fs::write(
        f.project.root.join("config.toml"),
        "context = \"Project context sentinel\"\n[rules]\nproposal = \"Proposal rules sentinel\"\nspec = \"Spec rules sentinel\"\n",
    )
    .unwrap();
    let proposal = f.guide("proposal");
    assert!(proposal.starts_with(guide::PROPOSAL.trim_end()), "{proposal}");
    assert!(proposal.contains("Project context sentinel") && proposal.contains("Proposal rules sentinel"), "{proposal}");
    assert!(!proposal.contains("Spec rules sentinel"));
    let spec = f.guide("spec");
    assert!(spec.starts_with(guide::SPEC.trim_end()));
    assert!(spec.contains("Project context sentinel") && spec.contains("Spec rules sentinel"));
    assert!(!spec.contains("Proposal rules sentinel"));
}

#[test]
fn audit_context_config_refuses_oversized_context_and_rules() {
    let f = Fixture::new("bounds");
    for text in [
        format!("context = \"{}\"\n", "a".repeat(2001)),
        format!("[rules]\nproposal = \"{}\"\n", "a".repeat(2001)),
        format!("context = \"{}\"\n", "\u{00e9}".repeat(1001)),
    ] {
        std::fs::write(f.project.root.join("config.toml"), text).unwrap();
        let error = config::load(&f.project, &f.home).unwrap_err();
        assert!(error.message.contains("2000"), "{error}");
    }
}

#[test]
fn audit_context_local_rules_override_only_the_named_artifact() {
    let f = Fixture::new("merge");
    std::fs::write(
        f.home.join("config.toml"),
        "context = \"Global context\"\n[rules]\nproposal = \"Global proposal\"\nspec = \"Global spec\"\n",
    )
    .unwrap();
    std::fs::write(f.project.root.join("config.toml"), "context = \"Local context\"\n[rules]\nproposal = \"Local proposal\"\n").unwrap();
    let proposal = f.guide("proposal");
    assert!(proposal.contains("Local context") && proposal.contains("Local proposal"));
    assert!(!proposal.contains("Global context") && !proposal.contains("Global proposal"));
    let spec = f.guide("spec");
    assert!(spec.contains("Local context") && spec.contains("Global spec"));
}
