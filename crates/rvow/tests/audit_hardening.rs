use rvow_core::{change, ledger, store, vow};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Command;

struct Packet {
    identity: String,
}
struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    project: store::Project,
    dir: PathBuf,
}
impl Fixture {
    fn new(name: &str, mutants: Option<&str>) -> Self {
        let root = std::env::temp_dir().join(format!("rvow-hardening-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        let home = root.join("store");
        std::fs::create_dir_all(&repo).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "--allow-empty", "-qm", "init"],
        ] {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(&repo)
                    .env_remove("GIT_DIR")
                    .env_remove("GIT_WORK_TREE")
                    .env_remove("GIT_INDEX_FILE")
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let project = store::init(&repo, &home, store::Mode::Home).unwrap();
        let c = change::new(&project, "Hardening", "main", true).unwrap();
        let text = format!(
            "- TSK-x7k2q4 Check\n  verify: printf 'test result: ok. 1 passed; 0 failed;\\n'\n  files: src/**\n{}",
            mutants.map(|m| format!("  mutants: {m}\n")).unwrap_or_default()
        );
        std::fs::write(c.path.join("tasks.md"), text).unwrap();
        let dir = project.root.join("active").join(c.path.file_name().unwrap());
        std::fs::rename(&c.path, &dir).unwrap();
        let loaded = change::load(&dir, &[]).unwrap();
        let config = rvow_core::config::load(&project, &home).unwrap();
        let data = vow::snapshot(&project, &repo, &config, &loaded, "main").unwrap();
        let lock = project.lock().unwrap();
        ledger::append(&dir.join(change::LEDGER), &lock, ledger::Kind::Vow, data).unwrap();
        drop(lock);
        Self { root, repo, home, project, dir }
    }
    fn cli(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_rvow"))
            .args(args)
            .current_dir(&self.repo)
            .env("HOME", self.root.join("user"))
            .env("RVOW_HOME", &self.home)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap()
    }
    fn text(&self, args: &[&str]) -> String {
        String::from_utf8(self.cli(args).stdout).unwrap()
    }
    fn hidden(&self) -> PathBuf {
        self.home.join("hidden-checks").join(self.project.root.strip_prefix(&self.home).unwrap()).join("TSK-x7k2q4.sh")
    }
    fn events(&self) -> Vec<ledger::Event> {
        ledger::read(&self.dir.join(change::LEDGER)).unwrap().0
    }
    fn id(&self) -> String {
        self.dir.file_name().unwrap().to_str().unwrap()[..10].to_string()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn audit_hardening_optional_mutation_hash_compatibility_and_repo_private_identity() {
    let source = "- TSK-x7k2q4 Check\n  verify: true\n  files: src/**\n";
    let task = rvow_core::tasks::parse_tasks(source).0.remove(0);
    let legacy = json!({"title": task.title, "verify": task.verify, "covers": [], "files": task.files, "baseline_pass": task.baseline_pass, "timeout_secs": null});
    assert_eq!(vow::task_hash(&task), vow::sha256_hex(legacy.to_string().as_bytes()));
    let mutated = rvow_core::tasks::parse_tasks(&format!("{source}  mutants: false\n")).0.remove(0);
    assert_eq!(mutated.mutants.as_deref(), Some("false"));
    assert!(!rvow_core::tasks::parse_tasks(&format!("{source}  mutation: false\n")).1.is_empty());
    assert_ne!(vow::task_hash(&task), vow::task_hash(&mutated));
    let f = Fixture::new("identity", None);
    let mut project = f.project.clone();
    project.meta.mode = store::Mode::Repo;
    project.root = f.repo.join("rvow");
    let path = rvow_core::hardening::private_path(&project, &f.home, &task.id).unwrap();
    assert!(path.starts_with(f.home.join("hidden-checks/repo")));
    assert!(!path.starts_with(&f.repo));
    project.root = f.root.join("moved-repo/rvow");
    assert_eq!(rvow_core::hardening::private_path(&project, &f.home, &task.id).unwrap(), path);
    project.meta.mode = store::Mode::Home;
    project.meta.remote = Some("github.com/owner/project".into());
    project.root = f.home.join("github.com/owner/project");
    assert_eq!(rvow_core::hardening::project_home(&project).unwrap(), f.home);
    project.root = f.home.join("github.com/different/project");
    assert!(rvow_core::hardening::project_home(&project).is_err());
}

#[test]
fn audit_hardening_mutation_failure_pass_and_freshness() {
    let f = Fixture::new("mutation", Some("false"));
    let out = f.cli(&["prove", "TSK-x7k2q4"]);
    assert!(!out.status.success());
    assert!(String::from_utf8(out.stdout).unwrap().contains("mutation"));
    assert_eq!(f.events().last().unwrap().data["mutants"]["passed"], false);
    let tasks = f.dir.join("tasks.md");
    let text = std::fs::read_to_string(&tasks).unwrap();
    std::fs::write(&tasks, text.replace("mutants: false", "mutants: true")).unwrap();
    assert!(f.cli(&["prove", "TSK-x7k2q4"]).status.success());
    assert_eq!(f.events().last().unwrap().data["mutants"]["passed"], true);
    std::fs::write(&tasks, text.replace("mutants: false", "mutants: printf ok")).unwrap();
    assert!(f.text(&["status"]).contains("stale"));
    assert!(f.text(&["status"]).contains("mutants changed"));
    std::fs::write(&tasks, text.replace("  mutants: false\n", "")).unwrap();
    assert!(f.text(&["status"]).contains("mutants changed"));
}

#[test]
fn audit_hardening_private_output_secrecy_and_policy_freshness() {
    let f = Fixture::new("private", None);
    assert!(f.cli(&["prove", "TSK-x7k2q4"]).status.success());
    let path = f.hidden();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "printf 'PRIVATE-SENTINEL'; printf 'PRIVATE-SENTINEL' >&2; exit 1").unwrap();
    assert!(f.text(&["status"]).contains("stale"));
    let out = f.cli(&["prove", "TSK-x7k2q4"]);
    assert!(!out.status.success());
    for bytes in [&out.stdout, &out.stderr] {
        assert!(!String::from_utf8_lossy(bytes).contains("PRIVATE-SENTINEL"));
    }
    let event = f.events().pop().unwrap();
    assert_eq!(event.data["private_check"]["passed"], false);
    assert!(!serde_json::to_string(&event).unwrap().contains("PRIVATE-SENTINEL"));
    std::fs::write(&path, "exit 0").unwrap();
    assert!(f.cli(&["prove", "TSK-x7k2q4"]).status.success());
    std::fs::write(&path, "true\n").unwrap();
    assert!(f.text(&["status"]).contains("stale"));
    assert!(f.cli(&["prove", "TSK-x7k2q4"]).status.success());
    std::fs::remove_file(path).unwrap();
    assert!(f.text(&["status"]).contains("stale"));
}

#[test]
fn audit_hardening_legacy_run_key_is_preserved() {
    let f = Fixture::new("legacy-key", None);
    rvow_core::proof::prove(&f.project, &f.home, &f.repo, "TSK-x7k2q4", &mut std::io::sink()).unwrap();
    let event = f.events().pop().unwrap();
    let data = &event.data;
    let legacy = vow::sha256_hex(json!([data["task_hash"], data["requirements"], data["inputs"], data["env_key"]]).to_string().as_bytes());
    assert_eq!(data["key"], legacy);
}

#[test]
fn audit_hardening_explicit_home_and_requirement_checks() {
    if std::env::var_os("RVOW_HARDENING_LIBRARY_CHILD").is_none() {
        let root = std::env::temp_dir().join(format!("rvow-hardening-env-{}", std::process::id()));
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "audit_hardening_explicit_home_and_requirement_checks", "--nocapture"])
            .env("RVOW_HARDENING_LIBRARY_CHILD", "1")
            .env("HOME", root.join("user"))
            .env("RVOW_HOME", root.join("elsewhere"))
            .env_remove("XDG_CONFIG_HOME")
            .output()
            .unwrap();
        let _ = std::fs::remove_dir_all(root);
        assert!(output.status.success(), "{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }
    use rvow_core::{config, fingerprint, proof};
    let f = Fixture::new("library-context", None);
    assert_ne!(store::home_dir().unwrap(), f.home);
    proof::prove(&f.project, &f.home, &f.repo, "TSK-x7k2q4", &mut std::io::sink()).unwrap();
    let path = f.hidden();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "exit 0").unwrap();
    let (living, _) = change::load_living(&f.project).unwrap();
    let changes = proof::active(&f.project, &living).unwrap();
    let cfg = config::load(&f.project, &f.home).unwrap();
    let now = proof::Now { top: &f.repo, config: &cfg, env: fingerprint::env(&f.repo, &cfg).unwrap(), living: &living };
    assert!(matches!(proof::state(&changes[0], &changes[0].loaded.tasks[0], &now).unwrap(), proof::State::Stale(_)));
    assert!(matches!(
        proof::state_for(&f.project, &f.home, &changes[0], &changes[0].loaded.tasks[0], &now).unwrap(),
        proof::State::Stale(_)
    ));
    proof::prove(&f.project, &f.home, &f.repo, "TSK-x7k2q4", &mut std::io::sink()).unwrap();
    std::fs::write(&path, "true\n").unwrap();
    let changes = proof::active(&f.project, &living).unwrap();
    assert!(matches!(proof::state(&changes[0], &changes[0].loaded.tasks[0], &now).unwrap(), proof::State::Stale(_)));

    let spec_dir = f.project.root.join("specs/hardening");
    std::fs::create_dir_all(&spec_dir).unwrap();
    std::fs::write(spec_dir.join("spec.md"), "## REQ-x7k2q4 Private check\nrvow SHALL check the requirement.\n### Scenario: check\n- WHEN proof runs\n- THEN the private check runs\n").unwrap();
    let task_file = f.dir.join("tasks.md");
    let text = std::fs::read_to_string(&task_file).unwrap();
    std::fs::write(&task_file, format!("{text}  covers: REQ-x7k2q4\n")).unwrap();
    let req_path = path.parent().unwrap().join("REQ-x7k2q4.sh");
    std::fs::write(&req_path, "printf REQUIREMENT-PRIVATE-SENTINEL; exit 1").unwrap();
    let mut echoed = Vec::new();
    let result = proof::prove(&f.project, &f.home, &f.repo, "TSK-x7k2q4", &mut echoed).unwrap();
    assert!(!result.proved);
    assert!(!String::from_utf8(echoed).unwrap().contains("REQUIREMENT-PRIVATE-SENTINEL"));
    let event = f.events().pop().unwrap();
    assert!(!serde_json::to_string(&event).unwrap().contains("REQUIREMENT-PRIVATE-SENTINEL"));
    assert_eq!(event.data["private_check"]["checks"].as_array().unwrap().len(), 2);
    std::fs::write(&req_path, "exit 0").unwrap();
    let result = proof::prove(&f.project, &f.home, &f.repo, "TSK-x7k2q4", &mut std::io::sink()).unwrap();
    assert!(result.proved);
    let private = result.private_check.unwrap();
    assert_eq!(private.checks.len(), 2);
    assert!(private.checks.iter().all(|c| c.passed));
    let event = f.events().pop().unwrap();
    let data = &event.data;
    let keyed = vow::sha256_hex(
        json!([data["task_hash"], data["requirements"], data["inputs"], data["env_key"], data["private_policy"]]).to_string().as_bytes(),
    );
    assert_eq!(data["key"], keyed);
    let (living, _) = change::load_living(&f.project).unwrap();
    let now = proof::Now { top: &f.repo, config: &cfg, env: fingerprint::env(&f.repo, &cfg).unwrap(), living: &living };
    let changes = proof::active(&f.project, &living).unwrap();
    let active = &changes[0];
    let task = &active.loaded.tasks[0];
    assert_eq!(rvow_core::hardening::task_policy_digest(&f.project, &f.home, task).unwrap(), Some(private.digest));
    assert_eq!(proof::state_for(&f.project, &f.home, active, task, &now).unwrap(), proof::State::Proved);
    std::fs::write(&req_path, "true\n").unwrap();
    assert_eq!(proof::state_for(&f.project, &f.home, active, task, &now).unwrap(), proof::State::Stale(vec!["private-policy-changed"]));
    std::fs::remove_file(&req_path).unwrap();
    assert_eq!(proof::state_for(&f.project, &f.home, active, task, &now).unwrap(), proof::State::Stale(vec!["private-policy-changed"]));
}

#[test]
fn audit_hardening_review_bounded_stale_and_no_authority() {
    let f = Fixture::new("review", None);
    let id = f.id();
    let output = f.cli(&["review", &id, "--packet"]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("TSK-x7k2q4"));
    let packet = Packet { identity: text.lines().find_map(|l| l.strip_prefix("identity: ")).unwrap().trim_matches('"').to_string() };
    let file = f.root.join("review.json");
    let findings: Vec<Value> = (0..21).map(|i| json!({"message": format!("finding {i}")})).collect();
    std::fs::write(&file, json!({"packet": packet.identity, "findings": findings}).to_string()).unwrap();
    assert!(f.cli(&["review", &id, "--record", file.to_str().unwrap()]).status.success());
    let events = f.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].kind, ledger::Kind::Note);
    assert_eq!(events[1].data["findings"].as_array().unwrap().len(), 20);
    assert_eq!(events[1].data["total"], 21);
    assert_eq!(events[1].data["truncated"], true);
    assert!(f.text(&["review", &id]).contains("before you accept, check:"));
    assert!(!f.cli(&["review", &id, "--packet", "--record", file.to_str().unwrap()]).status.success());
    std::fs::write(&file, json!({"packet": packet.identity, "findings": [{"message": "advice", "decision": "accept"}]}).to_string())
        .unwrap();
    assert!(!f.cli(&["review", &id, "--record", file.to_str().unwrap()]).status.success());
    for field in ["authority", "decision", "approved"] {
        std::fs::write(&file, json!({"packet": packet.identity, "findings": [], field: true}).to_string()).unwrap();
        assert!(!f.cli(&["review", &id, "--record", file.to_str().unwrap()]).status.success());
    }
    std::fs::create_dir_all(f.repo.join("src")).unwrap();
    std::fs::write(f.repo.join("src/new.rs"), "changed").unwrap();
    std::fs::write(&file, json!({"packet": packet.identity, "findings": []}).to_string()).unwrap();
    assert!(!f.cli(&["review", &id, "--record", file.to_str().unwrap()]).status.success());
    assert_eq!(f.events().len(), 2);
}
