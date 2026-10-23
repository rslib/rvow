use std::path::{Path, PathBuf};
use std::process::Command;

use rvow_core::{amend, change, gate, ledger, proof, store};
use serde_json::Value;

struct Fixture {
    temp: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    project: store::Project,
    change: String,
    dir: PathBuf,
    amendment: String,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        rvow_core::git::testrepo::remove_tree(&self.temp);
    }
}

impl Fixture {
    fn command(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.current_dir(&self.repo)
            .env("HOME", &self.temp)
            .env("RVOW_HOME", &self.home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "audit")
            .env("GIT_AUTHOR_EMAIL", "audit@test")
            .env("GIT_COMMITTER_NAME", "audit")
            .env("GIT_COMMITTER_EMAIL", "audit@test");
        c
    }

    fn cli(&self, args: &[&str], success: bool) -> String {
        let out = self.command(env!("CARGO_BIN_EXE_rvow")).args(args).output().unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        assert_eq!(out.status.success(), success, "{args:?}: {text} {}", String::from_utf8_lossy(&out.stderr));
        text
    }

    fn new(name: &str) -> Self {
        let temp = std::env::temp_dir().join(format!("rvow-audit-signing-{}-{name}", std::process::id()));
        std::fs::create_dir_all(temp.join("repo")).unwrap();
        let repo = temp.join("repo");
        let home = temp.join("home");
        let mut f = Self {
            temp,
            repo,
            home,
            project: store::Project { root: PathBuf::new(), meta: store::Meta { roots: vec![], remote: None, mode: store::Mode::Home } },
            change: String::new(),
            dir: PathBuf::new(),
            amendment: String::new(),
        };
        for args in [&["init", "-q", "-b", "main"][..], &["commit", "--allow-empty", "-q", "-m", name][..]] {
            assert!(f.command("git").args(args).status().unwrap().success());
        }
        f.project = store::init(&f.repo, &f.home, store::Mode::Home).unwrap();
        let out = f.cli(&["new", "--spec", "Out"], true);
        f.change = field(&out, "created").into();
        f.dir = field(&out, "path").into();
        std::fs::write(
            f.dir.join("tasks.md"),
            "- TSK-abc123 Write ok\n  covers: REQ-abc123\n  verify: test -f out.txt\n  files: out.txt\n",
        )
        .unwrap();
        std::fs::write(
            f.dir.join("specs/out.md"),
            "## ADD REQ-abc123 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
        )
        .unwrap();
        f.cli(&["accept", &f.change, "--yes"], true);
        f.dir = change::resolve(&f.project, &f.change).unwrap().path;
        std::fs::write(
            f.dir.join("tasks.md"),
            "- TSK-abc123 Write ok now\n  covers: REQ-abc123\n  verify: test -f out.txt\n  files: out.txt\n",
        )
        .unwrap();
        f.amendment = amend::amend(&f.project, &f.home, &f.repo, None, "clearer", None).unwrap()[0].id.to_string();
        let key = f.temp.join("key");
        assert!(f.command("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(&key).status().unwrap().success());
        let public = std::fs::read_to_string(key.with_extension("pub")).unwrap();
        std::fs::write(f.temp.join("allowed"), format!("owner {public}")).unwrap();
        std::fs::write(f.home.join("config.toml"), format!("signing_key = {:?}\n", key.to_str().unwrap())).unwrap();
        f
    }

    fn signed(&self, decision: &str) {
        self.cli(&["confirm", &self.amendment, decision, "--sign", "--signer", "owner"], true);
    }

    fn signature_failures(&self, project: &store::Project, _dir: &Path, id: &str) -> Vec<String> {
        let (living, _) = change::load_living(project).unwrap();
        let c = proof::active(project, &living).unwrap().into_iter().find(|c| c.dir.id.to_string() == id).unwrap();
        std::fs::write(
            project.root.join("config.toml"),
            format!("require_signature = true\nallowed_signers = {:?}\n", self.temp.join("allowed").to_str().unwrap()),
        )
        .unwrap();
        gate::change_gate(project, &self.home, &self.repo, &c).unwrap().failures.into_iter().filter(|s| s.contains("not signed")).collect()
    }
}

fn rewrite_events(f: &Fixture, mutate: impl Fn(&mut Value)) {
    let path = f.dir.join(change::LEDGER);
    let text = std::fs::read_to_string(&path).unwrap();
    let mut hashes: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut lines = Vec::new();
    for line in text.lines() {
        let mut value: Value = serde_json::from_str(line).unwrap();
        for parent in value["parents"].as_array_mut().unwrap() {
            if let Some(hash) = hashes.get(parent.as_str().unwrap()) {
                *parent = Value::String(hash.clone());
            }
        }
        if let Some(reference) = value.get_mut("amendment_event")
            && let Some(hash) = hashes.get(reference.as_str().unwrap())
        {
            *reference = Value::String(hash.clone());
        }
        mutate(&mut value);
        let new = value.to_string();
        hashes.insert(ledger::short_hash(line.as_bytes()), ledger::short_hash(new.as_bytes()));
        lines.push(new);
    }
    std::fs::write(path, format!("{}\n", lines.join("\n"))).unwrap();
}

#[test]
fn audit_signing_cli_refuses_unsigned_required_decisions_without_recording_them() {
    let f = Fixture::new("required-cli");
    std::fs::write(
        f.project.root.join("config.toml"),
        format!("require_signature = true\nallowed_signers = {:?}\n", f.temp.join("allowed").to_str().unwrap()),
    )
    .unwrap();
    let before = std::fs::read_to_string(f.dir.join(change::LEDGER)).unwrap();
    let out = f.cli(&["confirm", &f.amendment, "--accept"], false);
    assert!(out.contains("--sign"), "{out}");
    assert_eq!(std::fs::read_to_string(f.dir.join(change::LEDGER)).unwrap(), before);
    f.signed("--accept");
    assert!(f.signature_failures(&f.project, &f.dir, &f.change).is_empty());
}

fn field<'a>(out: &'a str, key: &str) -> &'a str {
    out.lines().find_map(|l| l.strip_prefix(&format!("{key}: "))).unwrap()
}

#[test]
fn audit_signing_cli_accept_reject_and_unknown_signer() {
    for (name, decision) in [("accept", "--accept"), ("reject", "--reject")] {
        let f = Fixture::new(name);
        f.signed(decision);
        assert!(f.signature_failures(&f.project, &f.dir, &f.change).is_empty());
        let public = std::fs::read_to_string(f.temp.join("key.pub")).unwrap();
        std::fs::write(f.temp.join("allowed"), format!("stranger {public}")).unwrap();
        assert_eq!(f.signature_failures(&f.project, &f.dir, &f.change).len(), 1);
    }
}

#[test]
fn audit_signing_unsigned_default_and_required() {
    let f = Fixture::new("unsigned");
    f.cli(&["confirm", &f.amendment, "--accept"], true);
    let r = gate::gate(&f.project, &f.home, &f.repo, gate::Options::default(), &mut std::io::sink()).unwrap();
    assert!(!r.failures.iter().any(|s| s.contains("not signed")));
    assert_eq!(f.signature_failures(&f.project, &f.dir, &f.change).len(), 1);
}

#[test]
fn audit_signing_failed_key_writes_nothing() {
    let f = Fixture::new("failed");
    let path = f.dir.join(change::LEDGER);
    let before = std::fs::read(&path).unwrap();
    std::fs::write(f.home.join("config.toml"), "signing_key = '/missing/key'\n").unwrap();
    f.cli(&["confirm", &f.amendment, "--accept", "--sign", "--signer", "owner"], false);
    assert_eq!(std::fs::read(path).unwrap(), before);
    let encrypted = f.temp.join("encrypted");
    assert!(f.command("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "passphrase", "-f"]).arg(&encrypted).status().unwrap().success());
    std::fs::write(f.home.join("config.toml"), format!("signing_key = {:?}\n", encrypted.to_str().unwrap())).unwrap();
    let text = f.cli(&["confirm", &f.amendment, "--accept", "--sign", "--signer", "owner"], false);
    assert!(text.contains("error[sign]"), "{text}");
    assert_eq!(std::fs::read(f.dir.join(change::LEDGER)).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn audit_signing_batch_failure_records_no_decisions() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new("batch");
    amend::amend(&f.project, &f.home, &f.repo, None, "second amendment", None).unwrap();
    let (living, _) = change::load_living(&f.project).unwrap();
    let pending = amend::pending(&proof::active(&f.project, &living).unwrap());
    assert_eq!(pending.len(), 2);
    let digest = amend::digest(&pending);
    let before = std::fs::read(f.dir.join(change::LEDGER)).unwrap();
    let bin = f.temp.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let wrapper = bin.join("ssh-keygen");
    let counter = f.temp.join("signed-once");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nif test -f '{}'; then exit 1; fi\ntouch '{}'\nexec /usr/bin/ssh-keygen \"$@\"\n",
            counter.display(),
            counter.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let out = f
        .command(env!("CARGO_BIN_EXE_rvow"))
        .env("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()))
        .args(["confirm", "--all", &digest, "--accept", "--sign", "--signer", "owner"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    assert!(counter.exists());
    assert_eq!(std::fs::read(f.dir.join(change::LEDGER)).unwrap(), before);
}

#[test]
fn audit_signing_tampered_snapshot_event_decision_and_ancestor() {
    for mode in ["snapshot", "reason", "time", "decision", "ancestor", "reference"] {
        let f = Fixture::new(mode);
        f.signed("--accept");
        rewrite_events(&f, |value| {
            if value["kind"] == "amend" {
                match mode {
                    "snapshot" => value["snapshot"]["tampered"] = true.into(),
                    "reason" => value["reason"] = "replaced reason".into(),
                    "time" => value["time"] = 1.into(),
                    _ => {}
                }
            }
            if value["kind"] == "confirm" {
                match mode {
                    "decision" => value["decision"] = "reject".into(),
                    "ancestor" => value["parents"] = serde_json::json!([]),
                    "reference" => value["amendment_event"] = "other".into(),
                    _ => {}
                }
            }
        });
        assert_eq!(f.signature_failures(&f.project, &f.dir, &f.change).len(), 1, "{mode}");
    }
}

#[test]
fn audit_signing_project_change_replay_and_portable_identity() {
    let f = Fixture::new("identity");
    f.signed("--accept");
    let events = ledger::read(&f.dir.join(change::LEDGER)).unwrap().0;
    let amendment = events.iter().find(|e| e.kind == ledger::Kind::Amend).unwrap();
    let id = f.change.parse().unwrap();
    let payload = gate::bound_confirm_payload(&f.project, &id, amendment, "accept").unwrap();
    let mut portable = f.project.clone();
    portable.root = f.temp.join("elsewhere");
    portable.meta.mode = store::Mode::Repo;
    assert_eq!(payload, gate::bound_confirm_payload(&portable, &id, amendment, "accept").unwrap());
    for mode in ["roots", "remote"] {
        let mut other = f.project.clone();
        if mode == "roots" {
            other.meta.roots = vec!["different-root".into()];
        } else {
            other.meta.remote = Some("different-remote".into());
        }
        assert_eq!(f.signature_failures(&other, &f.dir, &f.change).len(), 1, "{mode}");
    }
    let mut other = Fixture::new("change-replay");
    other.project.meta = f.project.meta.clone();
    std::fs::write(other.dir.join(change::LEDGER), std::fs::read(f.dir.join(change::LEDGER)).unwrap()).unwrap();
    // Use the original allowed key to isolate the changed CHG identity.
    std::fs::copy(f.temp.join("allowed"), other.temp.join("allowed")).unwrap();
    assert_eq!(other.signature_failures(&other.project, &other.dir, &other.change).len(), 1);
}

#[test]
fn audit_signing_legacy_signature_is_not_authority() {
    let f = Fixture::new("legacy");
    let payload = gate::confirm_payload(&f.amendment, "accept");
    let signature = gate::sign(&f.temp.join("key"), &payload).unwrap();
    let lock = f.project.lock().unwrap();
    let data = serde_json::json!({ "amendment": f.amendment, "decision": "accept", "signer": "owner", "signature": signature })
        .as_object()
        .unwrap()
        .clone();
    ledger::append(&f.dir.join(change::LEDGER), &lock, ledger::Kind::Confirm, data).unwrap();
    drop(lock);
    rewrite_events(&f, |value| {
        if value["kind"] == "amend" {
            value["snapshot"]["replaced-content"] = true.into();
        }
    });
    let signature_file = f.temp.join("legacy.sig");
    std::fs::write(&signature_file, &signature).unwrap();
    let mut verifier = f
        .command("ssh-keygen")
        .args(["-Y", "verify", "-I", "owner", "-n", "rvow", "-f"])
        .arg(f.temp.join("allowed"))
        .arg("-s")
        .arg(signature_file)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    use std::io::Write;
    verifier.stdin.take().unwrap().write_all(payload.as_bytes()).unwrap();
    assert!(verifier.wait().unwrap().success(), "the old payload still verifies replaced content");
    assert_eq!(f.signature_failures(&f.project, &f.dir, &f.change).len(), 1);
}
