use std::path::{Path, PathBuf};
use std::process::Command;

use rvow_gate::ledger::{self, Kind};
use rvow_gate::{archive, change, config, fingerprint, gate, proof, store, vow};

const PASS: &str = "echo 'test result: ok. 1 passed; 0 failed;'";

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

    /// Like `new`, but with the rvow tree inside the repository.
    fn repo(name: &str, config: &str) -> Self {
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
        let project = store::init(&repo, &home, store::Mode::Repo).unwrap();
        std::fs::write(project.root.join("config.toml"), config).unwrap();
        Self { dir, repo, home, project }
    }

    fn git(&self, args: &[&str]) {
        let output = git_command(&self.repo).args(args).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }

    fn exclude(&self, line: &str) {
        let path = self.repo.join(".git/info/exclude");
        let mut text = std::fs::read_to_string(&path).unwrap_or_default();
        text.push_str(line);
        text.push('\n');
        std::fs::write(path, text).unwrap();
    }

    /// Accepts a change whose single task writes `file` and returns (change, task, minted).
    fn accept(&self, title: &str, file: &str, spec: Option<&str>, covers: &str) -> (String, String, Vec<(String, String)>) {
        let branch = rvow_gate::git::branch(&self.repo).unwrap().unwrap();
        let plan = change::new(&self.project, title, &branch, spec.is_none()).unwrap();
        let covers = if covers.is_empty() { String::new() } else { format!("  covers: {covers}\n") };
        std::fs::write(
            plan.path.join("tasks.md"),
            format!("- TSK-new1 {title}\n{covers}  verify: grep -qx ok {file} && {PASS}\n  files: {file}\n"),
        )
        .unwrap();
        if let Some(spec) = spec {
            std::fs::write(plan.path.join("specs/out.md"), spec).unwrap();
        }
        let prepared = vow::prepare(&self.project, &self.home, plan, &self.repo, &mut std::io::sink()).unwrap();
        let accepted = vow::commit(&self.project, prepared).unwrap();
        let task = accepted.minted.iter().find(|(old, _)| old == "TSK-new1").unwrap().1.to_string();
        let minted = accepted.minted.iter().map(|(a, b)| (a.clone(), b.to_string())).collect();
        (accepted.id.to_string(), task, minted)
    }

    fn prove_and_commit(&self, task: &str, file: &str) -> proof::Proved {
        std::fs::write(self.repo.join(file), "ok\n").unwrap();
        let proved = proof::prove(&self.project, &self.home, &self.repo, task, &mut std::io::sink()).unwrap();
        assert!(proved.proved);
        self.git(&["add", file]);
        self.git(&["commit", "-q", "-m", file]);
        proved
    }

    fn archive(&self, change: &str) -> archive::Archived {
        archive::archive(&self.project, &self.home, &self.repo, change).unwrap()
    }

    fn rerun(&self) -> rvow_gate::error::Result<gate::Report> {
        gate::gate(&self.project, &self.home, &self.repo, gate::Options { strict: true, rerun: true, out: None }, &mut std::io::sink())
    }
}

#[test]
fn rerun_creates_no_links_by_default() {
    let f = Fixture::new("links-default", "checks = [\"test ! -e node_modules && test ! -e target\"]\n");
    f.exclude("node_modules/");
    f.exclude("target/");
    std::fs::create_dir_all(f.repo.join("node_modules")).unwrap();
    std::fs::create_dir_all(f.repo.join("target")).unwrap();
    let report = f.rerun().unwrap();
    assert!(report.passed(), "{report:?}");
}

#[test]
fn rerun_links_a_configured_directory() {
    let f = Fixture::new(
        "links-configured",
        "rerun_links = [\"node_modules\"]\nchecks = [\"test -L node_modules && test -f node_modules/marker && test ! -e target\"]\n",
    );
    f.exclude("node_modules/");
    f.exclude("target/");
    std::fs::create_dir_all(f.repo.join("node_modules")).unwrap();
    std::fs::create_dir_all(f.repo.join("target")).unwrap();
    std::fs::write(f.repo.join("node_modules/marker"), "x").unwrap();
    let report = f.rerun().unwrap();
    assert!(report.passed(), "{report:?}");
}

#[test]
fn rerun_links_refuses_tracked_and_escaping_names() {
    for (n, name) in ["initial.txt", "../outside", "/etc"].iter().enumerate() {
        let f = Fixture::new(&format!("links-bad-{n}"), &format!("rerun_links = [\"{name}\"]\n"));
        let err = f.rerun().unwrap_err();
        assert_eq!(err.code, "config", "{name}");
        assert!(err.fix.contains("rerun_links"), "{name}: {}", err.fix);
        let trees = git_command(&f.repo).args(["worktree", "list", "--porcelain"]).output().unwrap();
        assert_eq!(String::from_utf8(trees.stdout).unwrap().matches("worktree ").count(), 1, "{name} left a worktree behind");
    }
}

#[test]
fn rerun_links_is_read_from_the_global_config_too() {
    let f = Fixture::new("links-global", "");
    std::fs::write(f.home.join("config.toml"), "rerun_links = [\"dist\"]\n").unwrap();
    assert_eq!(config::rerun_links(&f.project, &f.home).unwrap(), ["dist"]);
    std::fs::write(f.project.root.join("config.toml"), "rerun_links = []\n").unwrap();
    assert!(config::rerun_links(&f.project, &f.home).unwrap().is_empty());
}

#[test]
fn proof_event_carries_machine_fields_that_stay_outside_the_key() {
    let f = Fixture::new("machine-fields", "");
    let (c, task, _) = f.accept("Write output", "out.txt", None, "");
    let proved = f.prove_and_commit(&task, "out.txt");
    let dir = change::list(&f.project).unwrap().0.into_iter().find(|d| d.id.to_string() == c).unwrap();
    let (events, _) = ledger::read(&dir.path.join(change::LEDGER)).unwrap();
    let event = events.iter().rev().find(|e| e.kind == Kind::Prove && e.hash == proved.event).unwrap();
    let env: fingerprint::Env = serde_json::from_value(event.data["env"].clone()).unwrap();
    assert!(!env.cpu.is_empty(), "{env:?}");
    assert!(env.cores.is_some_and(|n| n > 0), "{env:?}");
    assert!(env.memory.is_some_and(|n| n > 0), "{env:?}");
    assert_eq!(event.data["env_key"].as_str(), Some(env.key().as_str()));
    let mut other = env.clone();
    other.cpu = "another cpu".into();
    other.cores = env.cores.map(|n| n + 1);
    other.memory = env.memory.map(|n| n + 1);
    assert_eq!(other.key(), env.key());
}

#[test]
fn renamed_requirement_title_merges_by_id() {
    let f = Fixture::new("rename", "");
    let spec = "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n\
## ADD REQ-new2 Other\nrvow SHALL stay put.\n### Scenario: s\n- WHEN archived\n- THEN it is unchanged\n";
    let (a, a_task, minted) = f.accept("Add output", "out.txt", Some(spec), "REQ-new1, REQ-new2");
    let id = minted.iter().find(|(old, _)| old == "REQ-new1").unwrap().1.clone();
    let other = minted.iter().find(|(old, _)| old == "REQ-new2").unwrap().1.clone();
    f.prove_and_commit(&a_task, "out.txt");
    f.archive(&a);
    let living = f.project.root.join("specs/out/spec.md");
    let before = std::fs::read_to_string(&living).unwrap();
    assert!(before.contains(&format!("## {id} Out\n")), "{before}");

    let rename = format!("## CHANGE {id} Output file\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n");
    let (b, b_task, _) = f.accept("Rename output", "out2.txt", Some(&rename), &id);
    f.prove_and_commit(&b_task, "out2.txt");
    f.archive(&b);
    let after = std::fs::read_to_string(&living).unwrap();
    assert!(after.contains(&format!("## {id} Output file\n")), "{after}");
    assert!(!after.contains(&format!("## {id} Out\n")), "{after}");
    assert_eq!(after.matches(&format!("## {id} ")).count(), 1, "{after}");
    assert_eq!(after, before.replace(&format!("## {id} Out\n"), &format!("## {id} Output file\n")), "something else moved");
    assert!(after.contains(&format!("## {other} Other\n")));
}

#[test]
fn a_changed_allowed_signers_file_is_weaken_drift() {
    let dir = std::env::temp_dir().join(format!("rvow-fix-signers-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let signers = dir.join("allowed_signers");
    std::fs::write(&signers, "alice@example ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA\n").unwrap();
    let f = Fixture::new("signers-drift", &format!("require_signature = true\nallowed_signers = \"{}\"\n", signers.display()));
    let (_, _, _) = f.accept("Write output", "out.txt", None, "");
    let options = || gate::Options { strict: false, rerun: false, out: None };
    let before = gate::gate(&f.project, &f.home, &f.repo, options(), &mut std::io::sink()).unwrap();
    assert!(!before.failures.iter().any(|l| l.contains("allowed_signers")), "{:?}", before.failures);
    std::fs::write(&signers, "mallory@example ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIB\n").unwrap();
    let after = gate::gate(&f.project, &f.home, &f.repo, options(), &mut std::io::sink()).unwrap();
    assert!(after.failures.iter().any(|l| l.contains("weaken drift") && l.contains("allowed_signers")), "{:?}", after.failures);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gate_fails_work_before_accept_shell_work() {
    let f = Fixture::new("before-accept", "");
    f.git(&["checkout", "-q", "-b", "work"]);
    let plan = change::new(&f.project, "Waiting", "work", true).unwrap();
    std::fs::create_dir_all(f.repo.join("src")).unwrap();
    std::fs::write(f.repo.join("src/a.rs"), "fn a() {}").unwrap();
    f.git(&["add", "src/a.rs"]);
    f.git(&["commit", "-q", "-m", "shell work"]);
    let report =
        gate::gate(&f.project, &f.home, &f.repo, gate::Options { strict: false, rerun: false, out: None }, &mut std::io::sink()).unwrap();
    assert!(!report.passed());
    assert!(report.failures.iter().any(|l| l.contains("src/a.rs")), "{report:?}");
    assert!(report.failures.iter().any(|l| l.contains(&plan.id.to_string())), "{report:?}");
}

#[test]
fn gate_fails_work_before_accept_no_plan_quiet() {
    let f = Fixture::new("no-plan", "");
    f.git(&["checkout", "-q", "-b", "work"]);
    std::fs::create_dir_all(f.repo.join("src")).unwrap();
    std::fs::write(f.repo.join("src/a.rs"), "fn a() {}").unwrap();
    f.git(&["add", "src/a.rs"]);
    f.git(&["commit", "-q", "-m", "shell work"]);
    let report =
        gate::gate(&f.project, &f.home, &f.repo, gate::Options { strict: false, rerun: false, out: None }, &mut std::io::sink()).unwrap();
    assert!(report.passed(), "{report:?}");
}

#[test]
fn gate_fails_work_before_accept_plan_files_pass() {
    let f = Fixture::repo("plan-files", "");
    f.git(&["add", "-A"]);
    f.git(&["commit", "-q", "-m", "rvow tree"]);
    f.git(&["checkout", "-q", "-b", "work"]);
    change::new(&f.project, "Waiting", "work", true).unwrap();
    std::fs::write(f.repo.join("rvow/note.txt"), "plan files only").unwrap();
    f.git(&["add", "rvow/note.txt"]);
    f.git(&["commit", "-q", "-m", "plan files"]);
    let report =
        gate::gate(&f.project, &f.home, &f.repo, gate::Options { strict: false, rerun: false, out: None }, &mut std::io::sink()).unwrap();
    assert!(report.passed(), "{report:?}");
}

#[test]
fn gate_fails_work_before_accept_done_change_covers() {
    let f = Fixture::new("covered", "");
    f.git(&["checkout", "-q", "-b", "work"]);
    let spec = "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN src/a.rs holds ok\n";
    let (c, task, _) = f.accept("Done", "src/a.rs", Some(spec), "REQ-new1");
    std::fs::create_dir_all(f.repo.join("src")).unwrap();
    f.prove_and_commit(&task, "src/a.rs");
    f.archive(&c);
    change::new(&f.project, "Waiting", "work", true).unwrap();
    std::fs::write(f.repo.join("src/a.rs"), "ok changed\n").unwrap();
    f.git(&["add", "src/a.rs"]);
    f.git(&["commit", "-q", "-m", "covered edit"]);
    let report =
        gate::gate(&f.project, &f.home, &f.repo, gate::Options { strict: false, rerun: false, out: None }, &mut std::io::sink()).unwrap();
    assert!(report.passed(), "{report:?}");
}
