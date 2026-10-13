//! Drift that the user did not cause: a new default config value, and a
//! placeholder that `rvow ids` minted after an older rvow put it in a
//! confirmed amendment.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use rvow_core::{amend, change, config, drift, git, ledger, proof, registry, store, vow};
use serde_json::{Map, Value, json};

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Setup {
    _tmp: PathBuf,
    home: PathBuf,
    repo: PathBuf,
    project: store::Project,
}

fn git_cmd(repo: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(ok.status.success(), "git {args:?}: {ok:?}");
}

const VERIFY: &str = "grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'";
const SPEC: &str = "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n";

/// An accepted change with one task, after `config` is written as the project config.
fn setup(name: &str, config: &str) -> Setup {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("rvow-driftfix-{}-{n}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let tmp = tmp.canonicalize().unwrap();
    let home = tmp.join("home");
    let repo = tmp.join("r");
    std::fs::create_dir_all(&repo).unwrap();
    git_cmd(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    git_cmd(&repo, &["add", "a.txt"]);
    git_cmd(&repo, &["commit", "-q", "-m", "init"]);

    let project = store::init(&repo, &home, store::Mode::Home).unwrap();
    if !config.is_empty() {
        std::fs::write(project.root.join("config.toml"), config).unwrap();
    }
    let c = change::new(&project, "Out", "main", false).unwrap();
    std::fs::write(c.path.join("tasks.md"), format!("- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: {VERIFY}\n  files: out.txt\n"))
        .unwrap();
    std::fs::write(c.path.join("specs/out.md"), SPEC).unwrap();
    let prepared = vow::prepare(&project, &home, c, &repo, &mut std::io::sink()).unwrap();
    vow::commit(&project, prepared).unwrap();
    Setup { _tmp: tmp, home, repo, project }
}

fn active(s: &Setup) -> proof::Active {
    let (living, _) = change::load_living(&s.project).unwrap();
    proof::active(&s.project, &living).unwrap().remove(0)
}

fn drift_with(s: &Setup, cfg: &config::Config) -> Vec<(drift::Class, String)> {
    let top = git::toplevel(&s.repo).unwrap();
    drift::drift(&s.project, &top, cfg, &active(s)).unwrap().into_iter().map(|i| (i.class, i.what)).collect()
}

fn drift_items(s: &Setup) -> Vec<(drift::Class, String)> {
    drift_with(s, &config::load(&s.project, &s.home).unwrap())
}

#[test]
fn config_new_default_is_not_drift() {
    let s = setup("default", "");
    let mut cfg = config::load(&s.project, &s.home).unwrap();
    // A newer rvow resolves a wider default, with no edit of config.toml.
    cfg.test_globs.push("**/new/**".into());
    let items = drift_with(&s, &cfg);
    assert!(!items.iter().any(|(_, w)| w == "config changed"), "{items:?}");
}

#[test]
fn config_user_edit_is_weaken() {
    let s = setup("edit", "");
    std::fs::write(s.project.root.join("config.toml"), "timeout = \"5m\"\n").unwrap();
    let items = drift_items(&s);
    assert!(items.contains(&(drift::Class::Weaken, "config changed".into())), "{items:?}");
}

#[test]
fn config_removed_user_value_is_weaken() {
    let s = setup("removed", "test_globs = [\"t/**\"]\n");
    std::fs::remove_file(s.project.root.join("config.toml")).unwrap();
    let items = drift_items(&s);
    assert!(items.contains(&(drift::Class::Weaken, "config changed".into())), "{items:?}");
}

/// Adds `TSK-new8` covering a new `REQ-new9`, and records and confirms an
/// amendment whose snapshot keeps both placeholders, as an rvow from before
/// amend minted ids did.
fn legacy_amend(s: &Setup) {
    let c = active(s);
    let tasks = c.dir.path.join("tasks.md");
    let text = std::fs::read_to_string(&tasks).unwrap();
    std::fs::write(&tasks, format!("{text}\n- TSK-new8 Write more\n  covers: REQ-new9\n  verify: {VERIFY}\n  files: out.txt\n")).unwrap();
    let spec = c.dir.path.join("specs/out.md");
    let spec_text = std::fs::read_to_string(&spec).unwrap();
    std::fs::write(
        &spec,
        format!("{spec_text}\n## ADD REQ-new9 More\nrvow SHALL write more.\n### Scenario: m\n- WHEN it runs\n- THEN out.txt holds more\n"),
    )
    .unwrap();
    let c = active(s);
    let top = git::toplevel(&s.repo).unwrap();
    let cfg = config::load(&s.project, &s.home).unwrap();
    let lock = s.project.lock().unwrap();
    let id = registry::mint(&s.project, &lock, rvow_core::id::Kind::Amd, Some(&c.dir.id)).unwrap();
    let mut data = Map::new();
    data.insert("snapshot".into(), Value::Object(drift::snapshot_now(&s.project, &top, &cfg, &c).unwrap()));
    data.insert("baseline".into(), json!([]));
    data.insert("amendment".into(), id.to_string().into());
    data.insert("reason".into(), "add a task".into());
    data.insert("items".into(), json!([]));
    data.insert("tasks".into(), json!([]));
    ledger::append(&c.dir.path.join(change::LEDGER), &lock, ledger::Kind::Amend, data).unwrap();
    drop(lock);
    amend::confirm(&s.project, &[id.to_string()], amend::Decision::Accept, None).unwrap();
    assert!(drift_items(s).is_empty(), "the legacy amendment must leave no drift: {:?}", drift_items(s));
}

fn minted(s: &Setup) -> String {
    let pairs = vow::mint_active(&s.project, &s.repo, None).unwrap();
    pairs.iter().find(|(from, _)| from == "TSK-new8").unwrap().1.to_string()
}

#[test]
fn minted_ids_after_a_confirmed_amend_are_not_drift() {
    let s = setup("rename", "");
    legacy_amend(&s);
    let task = minted(&s);
    assert_ne!(task, "TSK-new8");
    let items = drift_items(&s);
    assert!(items.is_empty(), "{items:?}");
}

#[test]
fn minted_task_gets_no_baseline_run_at_amend() {
    let s = setup("rerun", "");
    legacy_amend(&s);
    let task = minted(&s);
    std::fs::write(s.repo.join("out.txt"), "ok").unwrap();
    assert!(proof::prove(&s.project, &s.home, &s.repo, &task, &mut std::io::sink()).unwrap().proved);
    let c = active(&s);
    let design = c.dir.path.join("design.md");
    std::fs::write(&design, "# Design\nA note.\n").unwrap();
    amend::amend(&s.project, &s.home, &s.repo, None, "add a design note", None).unwrap();
    let (living, _) = change::load_living(&s.project).unwrap();
    let pending = amend::pending(&proof::active(&s.project, &living).unwrap());
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert!(pending[0].baseline.is_empty(), "a renamed task ran its baseline again: {:?}", pending[0].baseline);
}

#[test]
fn minted_task_with_a_changed_verify_is_drift() {
    let s = setup("changed", "");
    legacy_amend(&s);
    let task = minted(&s);
    let tasks = active(&s).dir.path.join("tasks.md");
    let text = std::fs::read_to_string(&tasks).unwrap();
    let mut out = String::new();
    let mut in_task = false;
    for line in text.lines() {
        if line.starts_with("- ") {
            in_task = line.contains(&task);
        }
        if in_task && line.trim_start().starts_with("verify:") {
            out.push_str("  verify: grep -q more out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n");
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    std::fs::write(&tasks, out).unwrap();
    let items = drift_items(&s);
    assert!(items.contains(&(drift::Class::Weaken, "TSK-new8 removed".into())), "{items:?}");
    assert!(items.iter().any(|(_, w)| w.starts_with(&format!("{task} added"))), "{items:?}");
}

/// Records and confirms an amendment whose snapshot holds `cfg` and no
/// `config_keys`, as an rvow from before those keys were recorded did.
fn legacy_config_baseline(s: &Setup, cfg: &config::Config) {
    let c = active(s);
    let top = git::toplevel(&s.repo).unwrap();
    let mut snapshot = drift::snapshot_now(&s.project, &top, cfg, &c).unwrap();
    assert!(snapshot.remove("config_keys").is_some(), "the snapshot records config_keys");
    let lock = s.project.lock().unwrap();
    let id = registry::mint(&s.project, &lock, rvow_core::id::Kind::Amd, Some(&c.dir.id)).unwrap();
    let mut data = Map::new();
    data.insert("snapshot".into(), Value::Object(snapshot));
    data.insert("baseline".into(), json!([]));
    data.insert("amendment".into(), id.to_string().into());
    data.insert("reason".into(), "legacy".into());
    data.insert("items".into(), json!([]));
    data.insert("tasks".into(), json!([]));
    ledger::append(&c.dir.path.join(change::LEDGER), &lock, ledger::Kind::Amend, data).unwrap();
    drop(lock);
    amend::confirm(&s.project, &[id.to_string()], amend::Decision::Accept, None).unwrap();
}

#[test]
fn config_legacy_earlier_default_is_not_drift() {
    let s = setup("legacydefault", "");
    let mut old = config::load(&s.project, &s.home).unwrap();
    old.test_globs = ["tests/**", "**/*_test.*", "**/test_*.py", "**/*.test.*", "**/*.spec.*"].map(String::from).to_vec();
    legacy_config_baseline(&s, &old);
    let items = drift_items(&s);
    assert!(!items.iter().any(|(_, w)| w == "config changed"), "{items:?}");
}

#[test]
fn config_legacy_removed_user_value_is_weaken() {
    let s = setup("legacyremoved", "test_globs = [\"t/**\"]\n");
    legacy_config_baseline(&s, &config::load(&s.project, &s.home).unwrap());
    std::fs::remove_file(s.project.root.join("config.toml")).unwrap();
    let items = drift_items(&s);
    assert!(items.contains(&(drift::Class::Weaken, "config changed".into())), "{items:?}");
}
