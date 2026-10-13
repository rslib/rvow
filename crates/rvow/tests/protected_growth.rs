//! A protected test file that only grew is not drift; losing or changing
//! lines still is, and so does any change to a protected infra file.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

/// The item text a protected-file change gets when its grammar is not in
/// the grammar cache: an empty temporary home keeps it deterministic.
fn grammar_hint(lang: &str) -> String {
    format!(
        "(the {lang} grammar could not load: run `rvow grammar fetch {lang}`, or check RVOW_GRAMMAR_URL when the download base is overridden)"
    )
}

use rvow_core::{change, config, drift, git, proof, store, vow};

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
const RUST_ONE: &str = "#[test]\nfn one() {\n    assert!(true);\n}\n";

/// An accepted change whose baseline snapshot holds a Rust test file, a shell
/// test file and an infra file as protected files.
fn setup(name: &str) -> Setup {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("rvow-growth-{n}-{name}-{}", std::process::id()));
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

    std::fs::create_dir_all(repo.join("tests")).unwrap();
    std::fs::write(repo.join("tests/cli.rs"), RUST_ONE).unwrap();
    std::fs::write(repo.join("tests/t.sh"), "check_one\nexit 0\n").unwrap();
    std::fs::write(repo.join("tox.ini"), "[tox]\n").unwrap();

    let project = store::init(&repo, &home, store::Mode::Home).unwrap();
    let c = change::new(&project, "Out", "main", false).unwrap();
    std::fs::write(c.path.join("tasks.md"), format!("- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: {VERIFY}\n  files: out.txt\n"))
        .unwrap();
    std::fs::write(c.path.join("specs/out.md"), SPEC).unwrap();
    let prepared = vow::prepare(&project, &home, c, &repo, &mut std::io::sink()).unwrap();
    vow::commit(&project, prepared).unwrap();
    Setup { _tmp: tmp, home, repo, project }
}

fn drift_items(s: &Setup) -> Vec<(drift::Class, String)> {
    let (living, _) = change::load_living(&s.project).unwrap();
    let active = proof::active(&s.project, &living).unwrap().remove(0);
    let top = git::toplevel(&s.repo).unwrap();
    let cfg = config::load(&s.project, &s.home).unwrap();
    // The setup home holds no grammar cache, so the syntax-aware rule never
    // applies here: a protected change keeps the fetch hint.
    drift::drift_with_home(&s.project, Some(&s.home), &top, &cfg, &active).unwrap().into_iter().map(|i| (i.class, i.what)).collect()
}

#[test]
fn protected_growth_rust_gain_is_not_drift() {
    let s = setup("rust-gain");
    let mut text = std::fs::read_to_string(s.repo.join("tests/cli.rs")).unwrap();
    text.push_str("\n#[test]\nfn two() {\n    assert!(true);\n}\n");
    std::fs::write(s.repo.join("tests/cli.rs"), text).unwrap();
    let items = drift_items(&s);
    assert!(items.is_empty(), "{items:?}");
}

#[test]
fn protected_growth_rust_loss_is_weaken() {
    let s = setup("rust-loss");
    std::fs::write(s.repo.join("tests/cli.rs"), "// no tests here\n").unwrap();
    let items = drift_items(&s);
    assert!(items.contains(&(drift::Class::Weaken, format!("protected file tests/cli.rs changed {}", grammar_hint("rust")))), "{items:?}");
}

#[test]
fn protected_growth_shell_gain_is_not_drift() {
    let s = setup("shell-gain");
    let mut text = std::fs::read_to_string(s.repo.join("tests/t.sh")).unwrap();
    text.push_str("assert_ok\n");
    std::fs::write(s.repo.join("tests/t.sh"), text).unwrap();
    let items = drift_items(&s);
    assert!(items.is_empty(), "{items:?}");
}

#[test]
fn protected_growth_infra_gain_is_weaken() {
    let s = setup("infra-gain");
    let mut text = std::fs::read_to_string(s.repo.join("tox.ini")).unwrap();
    text.push_str("# a comment\n");
    std::fs::write(s.repo.join("tox.ini"), text).unwrap();
    let items = drift_items(&s);
    assert!(items.contains(&(drift::Class::Weaken, format!("protected file tox.ini changed {}", grammar_hint("properties")))), "{items:?}");
}
