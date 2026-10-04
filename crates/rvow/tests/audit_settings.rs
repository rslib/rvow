use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rvow_core::{change, config, ledger, store, vow};

struct Fixture {
    dir: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    project: store::Project,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rvow-audit-settings-{}-{name}", std::process::id()));
        let repo = make_repo(&dir, "repo", "initial");
        let home = dir.join("home");
        let project = store::init(&repo, &home, store::Mode::Home).unwrap();
        Self { dir, repo, home, project }
    }

    fn cli_at(&self, repo: &Path, args: &[&str], agent: Option<&str>) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rvow"));
        cmd.args(args)
            .current_dir(repo)
            .env("HOME", &self.dir)
            .env("RVOW_HOME", &self.home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("RVOW_AGENT");
        if let Some(agent) = agent {
            cmd.env("RVOW_AGENT", agent);
        }
        cmd.output().unwrap()
    }
}

fn make_repo(dir: &Path, name: &str, content: &str) -> PathBuf {
    let repo = dir.join(name);
    std::fs::create_dir_all(&repo).unwrap();
    let run = |args: &[&str]| {
        let output = Command::new("git")
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
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    };
    run(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("initial.txt"), content).unwrap();
    run(&["add", "initial.txt"]);
    run(&["commit", "-q", "-m", "initial"]);
    repo
}

#[test]
fn audit_settings_clarify_applies_the_configured_cap() {
    for cap in [1, 2, 3] {
        let f = Fixture::new(&format!("cap-{cap}"));
        std::fs::write(f.project.root.join("config.toml"), format!("interview_cap = {cap}\n")).unwrap();
        let c = change::new(&f.project, "Questions", "main", true).unwrap();
        let proposal = c.path.join("proposal.md");
        let mut text = std::fs::read_to_string(&proposal).unwrap();
        for n in 1..=6 {
            text.push_str(&format!("\n[NEEDS CLARIFICATION: Q{n}? | recommended: yes | options: no]\n"));
        }
        std::fs::write(proposal, text).unwrap();
        let out = f.cli_at(&f.repo, &["clarify", "--json"], None);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(value["questions"].as_array().unwrap().len(), cap as usize);
        assert_eq!(value["remaining"], 6 - cap);
    }
}

#[test]
fn audit_settings_invalid_interview_caps_are_refused() {
    let f = Fixture::new("invalid-cap");
    for cap in [0, 5, u64::MAX] {
        std::fs::write(f.project.root.join("config.toml"), format!("interview_cap = {cap}\n")).unwrap();
        assert!(config::load(&f.project, &f.home).is_err(), "cap {cap} accepted");
    }
}

#[test]
fn audit_settings_rebind_reads_a_commit_map_from_the_cli() {
    let f = Fixture::new("rebind");
    let rewritten = make_repo(&f.dir, "rewritten", "different root");
    let old = rvow_core::git::root_commits(&f.repo).unwrap()[0].clone();
    let new = rvow_core::git::root_commits(&rewritten).unwrap()[0].clone();
    assert_ne!(old, new);
    let map = f.dir.join("commit-map");
    std::fs::write(&map, format!("old new\n{old} {new}\n")).unwrap();
    let output = f.cli_at(&rewritten, &["rebind", "--commit-map", map.to_str().unwrap()], None);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    let found = store::find(&rewritten, &f.home).unwrap().unwrap();
    assert_eq!(found.root, f.project.root);
    assert_eq!(found.meta.roots, vec![new]);
    let output = f.cli_at(&rewritten, &["rebind", "--commit-map", "missing-map"], None);
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stdout).unwrap().contains("missing-map"));
}

#[test]
fn audit_settings_proofs_use_configured_agent_and_allow_env_override() {
    let f = Fixture::new("agent");
    std::fs::write(f.home.join("config.toml"), "agent = \"configured-agent\"\n").unwrap();
    let c = change::new(&f.project, "Output", "main", true).unwrap();
    std::fs::write(
        c.path.join("tasks.md"),
        "- TSK-new1 Output\n  verify: test -f out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n",
    )
    .unwrap();
    let prepared = vow::prepare(&f.project, &f.home, c, &f.repo, &mut std::io::sink()).unwrap();
    let accepted = vow::commit(&f.project, prepared).unwrap();
    let task = accepted.minted.iter().find(|(old, _)| old == "TSK-new1").unwrap().1.to_string();
    std::fs::write(f.repo.join("out.txt"), "ok").unwrap();
    for (override_agent, expected) in [(None, "configured-agent"), (Some("environment-agent"), "environment-agent")] {
        let output = f.cli_at(&f.repo, &["prove", &task], override_agent);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
        let events = ledger::read(&accepted.path.join("ledger.jsonl")).unwrap().0;
        let event = events.iter().rev().find(|e| e.kind == ledger::Kind::Prove).unwrap();
        assert_eq!(event.data["run"]["agent"], expected);
    }
}
