use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use rvow_core::{change, config, fingerprint, ledger, runner, show, store};
use serde_json::{Value, json};

struct Fixture {
    root: PathBuf,
    project: store::Project,
    change: change::ChangeDir,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("rvow-audit-proof-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        for args in [
            vec!["init", "-q"],
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
        let project = store::init(&repo, &root.join("home"), store::Mode::Home).unwrap();
        let change = change::new(&project, "Proof audit", "main", false).unwrap();
        std::fs::write(change.path.join("tasks.md"), "- TSK-x7k2q4 Proof\n  verify: true\n  files: src/**\n").unwrap();
        Self { root, project, change }
    }

    fn record(&self, proved: bool, inputs: &str, env: Value) {
        let lock = self.project.lock().unwrap();
        let data = json!({"task": "TSK-x7k2q4", "task_hash": "task-v1", "requirements": {"REQ-x7k2q4": "req-v1"}, "verify": "true", "inputs": inputs, "key": format!("{inputs}-{env}"), "proved": proved, "env": env});
        ledger::append(&self.change.path.join(change::LEDGER), &lock, ledger::Kind::Prove, data.as_object().unwrap().clone()).unwrap();
    }

    fn history(&self) -> String {
        show::show(&self.project, "TSK-x7k2q4").unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        rvow_core::git::testrepo::remove_tree(&self.root);
    }
}

#[test]
fn audit_proof_compares_newest_matching_target_not_environment_key() {
    let f = Fixture::new("compare");
    f.record(false, "same-input", json!({"commands": {"rustc --version": "rustc old"}}));
    f.record(true, "same-input", json!({"commands": {"rustc --version": "rustc middle"}}));
    f.record(false, "same-input", json!({"commands": {"rustc --version": "rustc new"}}));
    f.record(true, "unrelated-input", json!({"commands": {"rustc --version": "rustc unrelated"}}));
    let out = f.history();
    assert!(out.contains("rustc --version: passed rustc middle, failed rustc new"), "{out}");
    assert!(!out.contains("rustc unrelated"), "{out}");
}

#[test]
fn audit_proof_never_compares_unrelated_inputs() {
    let f = Fixture::new("mismatch");
    f.record(true, "input-a", json!({"commands": {"tool": "one"}}));
    f.record(false, "input-b", json!({"commands": {"tool": "two"}}));
    assert!(!f.history().contains("env differs on:"));
}

#[test]
fn audit_proof_target_fields_and_partial_legacy_identity_do_not_cross_compare() {
    for field in ["task_hash", "requirements", "verify", "inputs"] {
        let f = Fixture::new(&format!("target-{field}"));
        let base = json!({"task": "TSK-x7k2q4", "task_hash": "task-v1", "requirements": {"REQ-x7k2q4": "req-v1"}, "verify": "true", "inputs": "same", "key": "same-key", "proved": true, "env": {"commands": {"tool": "one"}}});
        for partial in [false, true] {
            let mut other = base.clone();
            other["proved"] = false.into();
            other["env"]["commands"]["tool"] = "two".into();
            if partial {
                other.as_object_mut().unwrap().remove(field);
            } else {
                other[field] = if field == "requirements" { json!({"REQ-x7k2q4": "req-v2"}) } else { "different".into() };
            }
            let lock = f.project.lock().unwrap();
            for value in [&base, &other] {
                ledger::append(&f.change.path.join(change::LEDGER), &lock, ledger::Kind::Prove, value.as_object().unwrap().clone())
                    .unwrap();
            }
            drop(lock);
            assert!(!f.history().contains("env differs on:"), "{}", f.history());
        }
    }
}

#[test]
fn audit_proof_added_removed_tools_vars_platform_and_sanitized_values() {
    let f = Fixture::new("fields");
    f.record(false, "same", json!({"commands": {"old-tool": "old", "url-tool": "https://alice:shortpass@example.com"}, "vars": {"API_TOKEN": "short-secret", "MODE": "old\u{001b}[31m"}, "os": "linux", "arch": "x86_64"}));
    f.record(true, "same", json!({"commands": {"new-tool": "new", "url-tool": "clean"}, "vars": {"API_TOKEN": "other-secret", "MODE": "new"}, "os": "macos", "arch": "aarch64"}));
    let out = f.history();
    for expected in [
        "old-tool: passed <absent>, failed old",
        "new-tool: passed new, failed <absent>",
        "var MODE:",
        "var API_TOKEN:",
        "os: passed macos, failed linux",
        "arch: passed aarch64, failed x86_64",
    ] {
        assert!(out.contains(expected), "{out}");
    }
    for secret in ["short-secret", "other-secret", "shortpass", "alice", "\u{001b}"] {
        assert!(!out.contains(secret), "leaked {secret}: {out}");
    }
}

#[test]
fn audit_proof_masks_long_blobs_credentials_and_secret_fields() {
    for secret in [
        "abcdefabcdefabcdefabcdefabcdefab",
        "12345678901234567890123456789012",
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef",
        "https://bob:tiny@example.com/path",
        "PASSWORD='small-secret'",
    ] {
        let masked = runner::mask(secret, None, "");
        assert!(!masked.contains(secret), "{masked}");
        assert!(!masked.contains("tiny") && !masked.contains("small-secret"), "{masked}");
    }
}

#[test]
fn audit_proof_masked_nonsecret_variables_keep_distinct_fingerprints() {
    if std::env::var_os("RVOW_AUDIT_MODE_CHILD").is_none() {
        let executable = std::env::current_exe().unwrap();
        let mut keys = Vec::new();
        for value in ["a".repeat(64), "b".repeat(64)] {
            let out = Command::new(&executable)
                .args(["--exact", "audit_proof::audit_proof_masked_nonsecret_variables_keep_distinct_fingerprints", "--nocapture"])
                .env("RVOW_AUDIT_MODE_CHILD", "1")
                .env("RVOW_AUDIT_MODE", &value)
                .output()
                .unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
            let stdout = String::from_utf8(out.stdout).unwrap();
            assert!(!stdout.contains(&value));
            keys.push(stdout.lines().find(|line| line.starts_with("fingerprint=")).unwrap().to_string());
        }
        assert_ne!(keys[0], keys[1]);
        return;
    }
    let f = Fixture::new("masked-mode-child");
    let mut cfg = config::load(&f.project, &f.root.join("home")).unwrap();
    cfg.env_vars = vec!["RVOW_AUDIT_MODE".into()];
    let captured = fingerprint::env(&f.root, &cfg).unwrap();
    println!("fingerprint={}", captured.key());
}

#[test]
fn audit_proof_pem_tail_state_survives_header_eviction_and_keeps_live_echo() {
    let text = "-----BEGIN RSA PRIVATE KEY-----\nshort-body\none\ntwo\nthree\nfour\nfive\n-----END RSA PRIVATE KEY-----\ntest result: ok. 2 passed; 0 failed;\n";
    let cmd = format!("printf '%s' '{text}'");
    let mut echo = Vec::new();
    let outcome = runner::run(&cmd, &std::env::temp_dir(), &[], Duration::from_secs(10), &mut echo).unwrap();
    assert_eq!(outcome.tests_run, Some(2));
    assert_eq!(String::from_utf8(echo).unwrap(), text);
    assert_eq!(outcome.tail.len(), runner::TAIL_LINES);
    for body in ["short-body", "one", "two", "three", "four", "five"] {
        assert!(!outcome.tail.iter().any(|line| line == body), "{:?}", outcome.tail);
    }
    let truncated = runner::run(
        "printf '%s\\n' '-----BEGIN PRIVATE KEY-----' tiny tail",
        &std::env::temp_dir(),
        &[],
        Duration::from_secs(10),
        &mut std::io::sink(),
    )
    .unwrap();
    assert_eq!(truncated.tail, ["-----BEGIN PRIVATE KEY-----", "<masked>", "<masked>"]);
}

#[test]
fn audit_proof_secret_environment_values_have_stable_content_fingerprints() {
    if std::env::var_os("RVOW_AUDIT_SECRET_CHILD").is_none() {
        let f = Fixture::new("secret-env-parent");
        let executable = std::env::current_exe().unwrap();
        let mut results = Vec::new();
        for secret in ["short-first", "short-first", "short-second"] {
            let out = Command::new(&executable)
                .args(["--exact", "audit_proof::audit_proof_secret_environment_values_have_stable_content_fingerprints", "--nocapture"])
                .env("RVOW_AUDIT_SECRET_CHILD", &f.root)
                .env("RVOW_AUDIT_TOKEN", secret)
                .output()
                .unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
            let stdout = String::from_utf8(out.stdout).unwrap();
            assert!(!stdout.contains(secret), "{stdout}");
            results.push(stdout.lines().find(|line| line.starts_with("fingerprint=")).unwrap().to_string());
        }
        assert_eq!(results[0], results[1]);
        assert_ne!(results[1], results[2]);
        return;
    }
    let f = Fixture::new("secret-env-child");
    let mut cfg = config::load(&f.project, &f.root.join("home")).unwrap();
    cfg.env = vec!["printf '%s' https://bob:tiny@example.com".into()];
    cfg.env_vars = vec!["RVOW_AUDIT_TOKEN".into()];
    let captured = fingerprint::env(&f.root, &cfg).unwrap();
    assert!(captured.vars["RVOW_AUDIT_TOKEN"].starts_with("sha256:"));
    let encoded = serde_json::to_string(&captured).unwrap();
    assert!(!encoded.contains("tiny") && !encoded.contains("bob"), "{encoded}");
    println!("fingerprint={}", captured.key());
}
