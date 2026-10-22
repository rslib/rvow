use rvow_core::{change, config, fingerprint, proof, store};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

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
        let root = std::env::temp_dir().join(format!("rvow-audit-ci-{}-{name}", std::process::id()));
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let home = root.join("home");
        let mut git = Command::new("git");
        git.current_dir(&repo).args(["init", "-q", "-b", "main"]);
        assert!(git.status().unwrap().success());
        let mut git = Command::new("git");
        git.current_dir(&repo).args(["-c", "user.name=audit", "-c", "user.email=audit@test", "commit", "--allow-empty", "-q", "-m", name]);
        assert!(git.status().unwrap().success());
        let project = store::init(&repo, &home, store::Mode::Home).unwrap();
        let f = Self { root, repo, home, project };
        let created = f.cli(&f.repo, &f.home, &["new", "--spec", "Out"], &[], true);
        let dir = PathBuf::from(created.lines().find_map(|s| s.strip_prefix("path: ")).unwrap());
        std::fs::write(dir.join("tasks.md"), "- TSK-abc123 Write output\n  covers: REQ-abc123\n  verify: test -f out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n").unwrap();
        std::fs::write(
            dir.join("specs/out.md"),
            "## ADD REQ-abc123 Output\nrvow SHALL write output.\n### Scenario: output\n- WHEN it runs\n- THEN out.txt exists\n",
        )
        .unwrap();
        let id = created.lines().find_map(|s| s.strip_prefix("created: ")).unwrap();
        f.cli(&f.repo, &f.home, &["accept", id, "--yes"], &[], true);
        f
    }
    fn confirm_edit(&self) {
        let amendments = rvow_core::amend::amend(&self.project, &self.home, &self.repo, None, "fixture", None).unwrap();
        let ids: Vec<_> = amendments.iter().map(|a| a.id.to_string()).collect();
        rvow_core::amend::confirm(&self.project, &ids, rvow_core::amend::Decision::Accept, None).unwrap();
    }
    fn cli(&self, cwd: &std::path::Path, home: &std::path::Path, args: &[&str], vars: &[(&str, &str)], success: bool) -> String {
        let mut c = Command::new(env!("CARGO_BIN_EXE_rvow"));
        c.current_dir(cwd)
            .args(args)
            .env("HOME", &self.root)
            .env("RVOW_HOME", home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1");
        for key in ["GITHUB_ACTIONS", "GITLAB_CI", "CIRCLECI", "BUILDKITE", "TF_BUILD", "JENKINS_URL", "CI"] {
            c.env_remove(key);
        }
        for (key, value) in vars {
            c.env(key, value);
        }
        let out = c.output().unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        assert_eq!(out.status.success(), success, "{args:?}: {text} {}", String::from_utf8_lossy(&out.stderr));
        text
    }
}

#[test]
fn audit_ci_provider_identity_and_generic_fallback() {
    for (vars, provider, run, url) in [
        (
            vec![("GITLAB_CI", "true"), ("CI_PIPELINE_ID", "21"), ("CI_PIPELINE_URL", "https://gitlab.test/p/21")],
            "gitlab-ci",
            "21",
            "https://gitlab.test/p/21",
        ),
        (
            vec![("CIRCLECI", "true"), ("CIRCLE_BUILD_NUM", "22"), ("CIRCLE_BUILD_URL", "https://circle.test/22")],
            "circleci",
            "22",
            "https://circle.test/22",
        ),
        (
            vec![("BUILDKITE", "true"), ("BUILDKITE_BUILD_ID", "23"), ("BUILDKITE_BUILD_URL", "https://buildkite.test/23")],
            "buildkite",
            "23",
            "https://buildkite.test/23",
        ),
        (
            vec![
                ("TF_BUILD", "True"),
                ("BUILD_BUILDID", "24"),
                ("SYSTEM_COLLECTIONURI", "https://azure.test/"),
                ("SYSTEM_TEAMPROJECT", "proj"),
            ],
            "azure-pipelines",
            "24",
            "https://azure.test/proj/_build/results?buildId=24",
        ),
        (
            vec![("JENKINS_URL", "https://jenkins.test/"), ("BUILD_NUMBER", "25"), ("BUILD_URL", "https://jenkins.test/job/25")],
            "jenkins",
            "25",
            "https://jenkins.test/job/25",
        ),
        (vec![("CI", "true")], "generic", "", ""),
    ] {
        let vars: BTreeMap<_, _> = vars.into_iter().collect();
        let got = fingerprint::ci_from(&|k| vars.get(k).map(|s| s.to_string())).unwrap();
        assert_eq!((&*got.provider, &*got.run, &*got.url), (provider, run, url));
    }
    assert!(fingerprint::ci_from(&|_| None).is_none());
}

#[test]
fn audit_ci_bundle_runs_without_home_and_ingested_inputs_go_stale() {
    let f = Fixture::new("roundtrip");
    std::fs::write(f.repo.join("out.txt"), "ok").unwrap();
    assert!(Command::new("git").current_dir(&f.repo).args(["add", "out.txt"]).status().unwrap().success());
    assert!(
        Command::new("git")
            .current_dir(&f.repo)
            .args(["-c", "user.name=audit", "-c", "user.email=audit@test", "commit", "-q", "-m", "output"])
            .status()
            .unwrap()
            .success()
    );
    let bundle = f.root.join("bundle.json");
    f.cli(&f.repo, &f.home, &["bundle", "--out", bundle.to_str().unwrap()], &[], true);
    let checkout = f.root.join("checkout");
    assert!(Command::new("git").args(["clone", "-q"]).arg(&f.repo).arg(&checkout).status().unwrap().success());
    let missing_home = f.root.join("unavailable-home");
    let results = f.root.join("runs.jsonl");
    f.cli(
        &checkout,
        &missing_home,
        &["gate", "--bundle", bundle.to_str().unwrap(), "--out", results.to_str().unwrap()],
        &[("GITLAB_CI", "true"), ("CI_PIPELINE_ID", "42"), ("CI_PIPELINE_URL", "https://gitlab.test/p/42")],
        true,
    );
    assert!(!missing_home.exists());
    let text = std::fs::read_to_string(&results).unwrap();
    let run: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(run["run"]["ci"]["provider"], "gitlab-ci");
    assert_eq!(run["run"]["ci"]["run"], "42");
    f.cli(&f.repo, &f.home, &["ingest", results.to_str().unwrap()], &[], true);
    let (living, _) = change::load_living(&f.project).unwrap();
    let active = proof::active(&f.project, &living).unwrap();
    let cfg = config::load(&f.project, &f.home).unwrap();
    let now = proof::Now::new(&f.repo, &cfg, fingerprint::env(&f.repo, &cfg).unwrap(), &living);
    assert_eq!(proof::state(&active[0], &active[0].loaded.tasks[0], &now).unwrap(), proof::State::Proved);
    std::fs::write(f.repo.join("out.txt"), "changed").unwrap();
    assert!(matches!(proof::state(&active[0], &active[0].loaded.tasks[0], &now).unwrap(), proof::State::Stale(_)));
}

#[test]
fn audit_ci_setup_and_overrides_are_captured_after_setup() {
    let f = Fixture::new("setup");
    std::fs::write(f.repo.join("out.txt"), "ok").unwrap();
    std::fs::write(f.project.root.join("config.toml"), "setup = [\"printf tool-v2 > version.txt\"]\nenv = [\"cat version.txt\", \"printf '%s' \\\"$RVOW_AUDIT_CI_MODE\\\"\"]\nenv_vars = [\"RVOW_AUDIT_CI_MODE\"]\nchecks = [\"test \\\"$RVOW_AUDIT_CI_MODE\\\" = new\"]\n[rerun_env]\nRVOW_AUDIT_CI_MODE = \"new\"\n").unwrap();
    f.confirm_edit();
    let bundle = f.root.join("bundle.json");
    let results = f.root.join("runs.jsonl");
    f.cli(&f.repo, &f.home, &["bundle", "--out", bundle.to_str().unwrap()], &[], true);
    f.cli(
        &f.repo,
        &f.root.join("missing"),
        &["gate", "--bundle", bundle.to_str().unwrap(), "--out", results.to_str().unwrap()],
        &[("RVOW_AUDIT_CI_MODE", "old")],
        true,
    );
    let run: serde_json::Value = serde_json::from_str(std::fs::read_to_string(results).unwrap().lines().next().unwrap()).unwrap();
    assert_eq!(run["env"]["commands"]["cat version.txt"], "tool-v2");
    assert_eq!(run["env"]["commands"]["printf '%s' \"$RVOW_AUDIT_CI_MODE\""], "new");
    assert_eq!(run["env"]["vars"]["RVOW_AUDIT_CI_MODE"], "new");
    let regular = f.root.join("regular.jsonl");
    f.cli(&f.repo, &f.home, &["gate", "--rerun", "--out", regular.to_str().unwrap()], &[("RVOW_AUDIT_CI_MODE", "old")], false);
    let run: serde_json::Value = serde_json::from_str(std::fs::read_to_string(regular).unwrap().lines().next().unwrap()).unwrap();
    assert_eq!(run["env"]["commands"]["cat version.txt"], "tool-v2");
    assert_eq!(run["env"]["vars"]["RVOW_AUDIT_CI_MODE"], "new");
}

#[test]
fn audit_ci_pending_export_and_wrong_project_are_refused() {
    let f = Fixture::new("identity");
    let bundle = f.root.join("bundle.json");
    f.cli(&f.repo, &f.home, &["bundle", "--out", bundle.to_str().unwrap()], &[], true);
    let other = Fixture::new("other-identity");
    let out = f.cli(&other.repo, &f.root.join("missing"), &["gate", "--bundle", bundle.to_str().unwrap()], &[], false);
    assert!(out.contains("bundle-project"), "{out}");
    let (living, _) = change::load_living(&f.project).unwrap();
    let active = proof::active(&f.project, &living).unwrap();
    let path = active[0].dir.path.join("tasks.md");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("Write output", "Write output now")).unwrap();
    rvow_core::amend::amend(&f.project, &f.home, &f.repo, None, "pending", None).unwrap();
    let out = f.cli(&f.repo, &f.home, &["bundle", "--out", bundle.to_str().unwrap()], &[], false);
    assert!(out.contains("waits for the user"), "{out}");
}

#[test]
fn audit_ci_ingest_refuses_project_path_traversal() {
    let f = Fixture::new("traversal");
    std::fs::write(f.repo.join("out.txt"), "ok").unwrap();
    let bundle = f.root.join("bundle.json");
    let results = f.root.join("runs.jsonl");
    f.cli(&f.repo, &f.home, &["bundle", "--out", bundle.to_str().unwrap()], &[], true);
    f.cli(&f.repo, &f.root.join("missing"), &["gate", "--bundle", bundle.to_str().unwrap(), "--out", results.to_str().unwrap()], &[], true);
    let mut run: serde_json::Value = serde_json::from_str(std::fs::read_to_string(&results).unwrap().lines().next().unwrap()).unwrap();
    let (living, _) = change::load_living(&f.project).unwrap();
    let active = proof::active(&f.project, &living).unwrap();
    let ledger = active[0].dir.path.join(change::LEDGER);
    let before = std::fs::read_to_string(&ledger).unwrap();
    for locator in [f.project.root.to_str().unwrap().to_string(), "../home/local/repo".to_string()] {
        run["project"] = locator.into();
        std::fs::write(&results, format!("{run}\n")).unwrap();
        f.cli(&f.repo, &f.home, &["ingest", results.to_str().unwrap()], &[], false);
        assert_eq!(std::fs::read_to_string(&ledger).unwrap(), before);
    }
}

#[test]
fn audit_ci_ingest_cannot_inject_event_authority() {
    let f = Fixture::new("event-authority");
    std::fs::write(f.repo.join("out.txt"), "ok").unwrap();
    let bundle = f.root.join("bundle.json");
    let results = f.root.join("runs.jsonl");
    f.cli(&f.repo, &f.home, &["bundle", "--out", bundle.to_str().unwrap()], &[], true);
    f.cli(&f.repo, &f.root.join("missing"), &["gate", "--bundle", bundle.to_str().unwrap(), "--out", results.to_str().unwrap()], &[], true);
    let original: serde_json::Value = serde_json::from_str(std::fs::read_to_string(&results).unwrap().lines().next().unwrap()).unwrap();
    let (living, _) = change::load_living(&f.project).unwrap();
    let active = proof::active(&f.project, &living).unwrap();
    let ledger = active[0].dir.path.join(change::LEDGER);
    let before = std::fs::read_to_string(&ledger).unwrap();
    for (field, value) in [("kind", serde_json::json!("confirm")), ("parents", serde_json::json!([])), ("time", serde_json::json!(0))] {
        let mut run = original.clone();
        run[field] = value;
        std::fs::write(&results, format!("{run}\n")).unwrap();
        f.cli(&f.repo, &f.home, &["ingest", results.to_str().unwrap()], &[], false);
        assert_eq!(std::fs::read_to_string(&ledger).unwrap(), before);
    }
}

#[test]
fn audit_ci_guard_objects_transfer_without_accepting_corrupt_content() {
    let f = Fixture::new("objects");
    std::fs::write(f.repo.join("out.txt"), "ok").unwrap();
    let body = "test -f out.txt\n";
    let (living, _) = change::load_living(&f.project).unwrap();
    let active = proof::active(&f.project, &living).unwrap();
    let path = active[0].dir.path.join("tasks.md");
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("files: out.txt", "files: out.txt, tests/**")
        .replace("test -f out.txt && echo", "test -f out.txt && sh tests/check.sh && echo");
    std::fs::write(path, text).unwrap();
    f.confirm_edit();
    let bundle = f.root.join("bundle.json");
    let results = f.root.join("runs.jsonl");
    f.cli(&f.repo, &f.home, &["bundle", "--out", bundle.to_str().unwrap()], &[], true);
    let checkout = f.root.join("checkout");
    assert!(Command::new("git").args(["clone", "-q"]).arg(&f.repo).arg(&checkout).status().unwrap().success());
    std::fs::create_dir_all(checkout.join("tests")).unwrap();
    std::fs::write(checkout.join("tests/check.sh"), body).unwrap();
    std::fs::write(checkout.join("out.txt"), "ok").unwrap();
    f.cli(
        &checkout,
        &f.root.join("missing"),
        &["gate", "--bundle", bundle.to_str().unwrap(), "--out", results.to_str().unwrap()],
        &[],
        true,
    );
    let original: serde_json::Value = serde_json::from_str(std::fs::read_to_string(&results).unwrap().lines().next().unwrap()).unwrap();
    let hash = rvow_core::vow::sha256_hex(body.as_bytes());
    assert_eq!(original["tests"]["tests/check.sh"], hash);
    assert!(!f.project.root.join("objects").join(&hash).exists());
    let mut corrupt = original.clone();
    corrupt["test_objects"][&hash] = "corrupt".into();
    std::fs::write(&results, format!("{corrupt}\n")).unwrap();
    f.cli(&f.repo, &f.home, &["ingest", results.to_str().unwrap()], &[], false);
    assert!(!f.project.root.join("objects").join(&hash).exists());
    std::fs::create_dir_all(f.repo.join("tests")).unwrap();
    std::fs::write(f.repo.join("tests/check.sh"), body).unwrap();
    std::fs::write(&results, format!("{original}\n")).unwrap();
    f.cli(&f.repo, &f.home, &["ingest", results.to_str().unwrap()], &[], true);
    assert_eq!(std::fs::read_to_string(f.project.root.join("objects").join(hash)).unwrap(), body);
    let active = proof::active(&f.project, &living).unwrap();
    assert!(rvow_core::guard::drift(&f.project, &f.repo, &active[0]).is_empty());
    std::fs::write(f.repo.join("tests/check.sh"), "").unwrap();
    assert!(!rvow_core::guard::drift(&f.project, &f.repo, &active[0]).is_empty());
}

#[test]
fn audit_ci_failed_bundle_writes_failed_results() {
    let f = Fixture::new("failure");
    let bundle = f.root.join("bundle.json");
    let results = f.root.join("runs.jsonl");
    f.cli(&f.repo, &f.home, &["bundle", "--out", bundle.to_str().unwrap()], &[], true);
    f.cli(
        &f.repo,
        &f.root.join("missing"),
        &["gate", "--bundle", bundle.to_str().unwrap(), "--out", results.to_str().unwrap()],
        &[],
        false,
    );
    let text = std::fs::read_to_string(results).unwrap();
    let run: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(run["proved"], false);
}
