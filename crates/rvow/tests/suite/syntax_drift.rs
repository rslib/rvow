//! Protected files compared by canonical syntax: a compiled infra file that
//! only lost a blank line or a comment is no drift, a changed value is, and
//! a file whose grammar is not in the cache stays drift and names the fetch
//! command. One test per spec scenario, offline through the fixture folder.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::MutexGuard;

use rvow_core::store::{self, Project};
use rvow_core::syntax::canonical::{self, Compare};
use rvow_core::syntax::grammar;
use rvow_core::{change, config, drift, git, hook, proof, vow};

fn fixtures() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/grammars").display().to_string()
}

/// Tests in this binary share the process env, so every one that sets
/// RVOW_GRAMMAR_URL holds the shared lock of the test binary.
fn env_lock() -> MutexGuard<'static, ()> {
    crate::env_lock::lock()
}

struct Setup {
    _tmp: PathBuf,
    home: PathBuf,
    repo: PathBuf,
    project: Project,
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

/// An accepted change in a fresh repository. `.cargo/config.toml` is a
/// protected infra file whose language has a pinned grammar.
fn setup(name: &str) -> Setup {
    let tmp = std::env::temp_dir().join(format!("rvow-syntax-drift-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let tmp = tmp.canonicalize().unwrap();
    let home = tmp.join("home");
    let repo = tmp.join("r");
    std::fs::create_dir_all(&repo).unwrap();
    git_cmd(&repo, &["init", "-q", "-b", "main"]);
    // .cargo/config.toml holds one comment and one blank line to remove.
    std::fs::create_dir_all(repo.join(".cargo")).unwrap();
    std::fs::write(repo.join(".cargo/config.toml"), "# build notes\nkey_po = \"value\"\n\nflags = []\n").unwrap();
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    git_cmd(&repo, &["add", "."]);
    git_cmd(&repo, &["commit", "-q", "-m", "init"]);

    let project = store::init(&repo, &home, store::Mode::Home).unwrap();
    let c = change::new(&project, "Out", "main", false).unwrap();
    std::fs::write(
        c.path.join("tasks.md"),
        "- TSK-new1 Write ok\n  covers: REQ-new1\n  verify: grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n",
    )
    .unwrap();
    std::fs::write(
        c.path.join("specs/out.md"),
        "## ADD REQ-new1 Out\nrvow SHALL write ok.\n### Scenario: s\n- WHEN it runs\n- THEN out.txt holds ok\n",
    )
    .unwrap();
    let prepared = vow::prepare(&project, &home, c, &repo, &mut std::io::sink()).unwrap();
    vow::commit(&project, prepared).unwrap();
    Setup { _tmp: tmp, home, repo, project }
}

fn items(s: &Setup, _env: &MutexGuard<'static, ()>) -> Vec<(drift::Class, String)> {
    let (living, _) = change::load_living(&s.project).unwrap();
    let active = proof::active(&s.project, &living).unwrap().remove(0);
    let top = git::toplevel(&s.repo).unwrap();
    let cfg = config::load(&s.project, &s.home).unwrap();
    drift::drift_with_home(&s.project, Some(&s.home), &top, &cfg, &active).unwrap().into_iter().map(|i| (i.class, i.what)).collect()
}

fn rewrites(s: &Setup, text: &str, env: &MutexGuard<'static, ()>) {
    let _ = env;
    std::fs::write(s.repo.join(".cargo/config.toml"), text).unwrap();
}

fn edit_of(s: &Setup, from: &str, to: &str) -> String {
    let text = std::fs::read_to_string(s.repo.join(".cargo/config.toml")).unwrap();
    assert!(text.contains(from), "{from} in {text}");
    text.replace(from, to)
}

const WITH_TRIVIA: &str = "# build notes\nkey_po = \"value\"\n\nflags = []\n";

fn prime_toml(home: &Path) {
    let entry = grammar::by_name("toml").unwrap();
    grammar::acquire(home, entry, Some(&fixtures())).unwrap();
}

fn saved_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rvow-saved-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

#[test]
fn syntax_drift_saved_second_compare_does_not_parse() {
    let home = saved_home("hit");
    let entry = grammar::by_name("toml").unwrap();
    grammar::acquire(&home, entry, Some(&fixtures())).unwrap();
    let (before, after) = ("[t]\nk = 1\n", "[t]\nk = 2\n");
    let mut first = grammar::Session::new().unwrap();
    let outcome = canonical::compare(&mut first, &home, entry, Some(&fixtures()), before, after).unwrap();
    assert_eq!(outcome, Compare::Different);
    assert_eq!(first.parses(), 2);
    // A fresh session: the saved result answers without parsing either text.
    let mut second = grammar::Session::new().unwrap();
    let outcome = canonical::compare(&mut second, &home, entry, Some(&fixtures()), before, after).unwrap();
    assert_eq!(outcome, Compare::Different);
    assert_eq!(second.parses(), 0, "the saved result answers the second compare");
}

#[test]
fn syntax_drift_saved_corrupt_result_compares_again() {
    let home = saved_home("corrupt-result");
    let entry = grammar::by_name("toml").unwrap();
    grammar::acquire(&home, entry, Some(&fixtures())).unwrap();
    let (before, after) = ("[t]\nk = 1\n\n", "[t]\nk = 1\n");
    let mut first = grammar::Session::new().unwrap();
    assert_eq!(canonical::compare(&mut first, &home, entry, Some(&fixtures()), before, after).unwrap(), Compare::Same);
    let file = canonical::result_file(&home, entry, before, after);
    assert!(file.is_file(), "the first compare saves its result");
    std::fs::write(&file, b"garbage").unwrap();
    let mut second = grammar::Session::new().unwrap();
    let outcome = canonical::compare(&mut second, &home, entry, Some(&fixtures()), before, after).unwrap();
    assert_eq!(outcome, Compare::Same);
    assert_eq!(second.parses(), 2, "a corrupt result file means compare again");
}

#[test]
fn syntax_drift_blank_line_removal_of_infra_is_no_drift() {
    let _env = env_lock();
    let s = setup("blank");
    prime_toml(&s.home);
    assert!(items(&s, &_env).is_empty());

    // Remove only the blank line: whitespace only.
    rewrites(&s, &WITH_TRIVIA.replace("\n\n", "\n"), &_env);
    assert!(items(&s, &_env).is_empty(), "blank-line-only edits are not drift");
}

#[test]
fn syntax_drift_comment_removal_of_infra_is_drift() {
    let _env = env_lock();
    let s = setup("comment");
    prime_toml(&s.home);
    assert!(items(&s, &_env).is_empty());

    // Remove only the comment: kept comment text is form text.
    rewrites(&s, &WITH_TRIVIA.replace("# build notes\n", ""), &_env);
    let got = items(&s, &_env);
    assert!(got.contains(&(drift::Class::Weaken, "protected file .cargo/config.toml changed".to_string())), "{got:?}");
}

#[test]
fn syntax_drift_changed_value_is_drift() {
    let _env = env_lock();
    let s = setup("value");
    prime_toml(&s.home);
    rewrites(&s, &edit_of(&s, "\"value\"", "\"other\""), &_env);
    let got = items(&s, &_env);
    assert!(got.contains(&(drift::Class::Weaken, "protected file .cargo/config.toml changed".to_string())), "{got:?}");
}

#[test]
fn syntax_drift_missing_grammar_stays_drift_with_the_fetch_hint() {
    let _env = env_lock();
    let s = setup("missing");
    // No grammar in the cache: the raw rule stays in force.
    rewrites(&s, &edit_of(&s, "\n\n", "\n"), &_env);
    let got = items(&s, &_env);
    let text = got.iter().find(|(_, w)| w.contains(".cargo/config.toml changed")).map(|(_, w)| w.clone()).unwrap_or_default();
    assert!(text.contains("the toml grammar could not load"), "{got:?}");
    assert!(text.contains("rvow grammar fetch toml"), "{got:?}");
}

#[test]
fn syntax_drift_fetch_prove_caches_grammar_and_reports_no_drift() {
    let _env = env_lock();
    let s = setup("fetch-prove");
    // SAFETY: under the env lock, and these tests read the vars only here.
    unsafe {
        std::env::set_var("RVOW_GRAMMAR_URL", fixtures());
    }
    // Empty cache: the toml grammar is not there yet.
    let entry = grammar::by_name("toml").unwrap();
    assert!(!grammar::cache_file(&s.home, entry).is_file());
    // A blank-line edit to the protected infra file.
    rewrites(&s, &edit_of(&s, "\n\n", "\n"), &_env);
    let (living, _) = change::load_living(&s.project).unwrap();
    let active = proof::active(&s.project, &living).unwrap().remove(0);
    let task = active.loaded.tasks[0].id.to_string();
    proof::prove(&s.project, &s.home, &s.repo, &task, &mut std::io::sink()).unwrap();
    assert!(grammar::cache_file(&s.home, entry).is_file(), "prove fetches the missing grammar");
    assert!(items(&s, &_env).is_empty(), "the blank-line edit is no drift once the grammar is cached");
    unsafe {
        std::env::remove_var("RVOW_GRAMMAR_URL");
    }
}

#[test]
fn syntax_drift_fetch_hook_check_downloads_nothing() {
    let _env = env_lock();
    let s = setup("fetch-hook");
    // SAFETY: under the env lock, and these tests read the vars only here.
    unsafe {
        std::env::set_var("RVOW_GRAMMAR_URL", fixtures());
    }
    let entry = grammar::by_name("toml").unwrap();
    rewrites(&s, &edit_of(&s, "\n\n", "\n"), &_env);
    let _ = hook::stop_reason(&s.project, &s.home, &s.repo, None).unwrap();
    assert!(!grammar::cache_file(&s.home, entry).is_file(), "a hook drift check stays offline");
    unsafe {
        std::env::remove_var("RVOW_GRAMMAR_URL");
    }
}

#[test]
fn syntax_drift_a_corrupt_cache_stays_drift() {
    let _env = env_lock();
    let s = setup("corrupt");
    // A cache file whose hash does not match the pinned one is not used.
    let entry = grammar::by_name("toml").unwrap();
    let file = grammar::cache_file(&s.home, entry);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"corrupt bytes").unwrap();
    rewrites(&s, &edit_of(&s, "\n\n", "\n"), &_env);
    let got = items(&s, &_env);
    let text = got.iter().find(|(_, w)| w.contains(".cargo/config.toml changed")).map(|(_, w)| w.clone()).unwrap_or_default();
    assert!(text.contains("rvow grammar fetch toml"), "{got:?}");
}

#[test]
fn syntax_drift_offline_prove_opens_no_connection_and_names_the_fetch() {
    let _env = env_lock();
    let s = setup("offline-prove");
    let entry = grammar::by_name("toml").unwrap();
    rewrites(&s, &edit_of(&s, "\n\n", "\n"), &_env);
    let (living, _) = change::load_living(&s.project).unwrap();
    let active = proof::active(&s.project, &living).unwrap().remove(0);
    let task = active.loaded.tasks[0].id.to_string();
    let started = std::time::Instant::now();
    let _ = proof::prove(&s.project, &s.home, &s.repo, &task, &mut std::io::sink());
    let took = started.elapsed();
    let got = items(&s, &_env);
    assert!(!grammar::cache_file(&s.home, entry).is_file(), "no download happened");
    assert!(took < std::time::Duration::from_secs(5), "prove must not wait on a connection, took {took:?}");
    let text = got.iter().find(|(_, w)| w.contains(".cargo/config.toml changed")).map(|(_, w)| w.clone()).unwrap_or_default();
    assert!(text.contains("rvow grammar fetch toml"), "{got:?}");
}

#[test]
fn syntax_drift_offline_refuses_the_pinned_url_at_once() {
    let _env = env_lock();
    let s = setup("offline-acquire");
    let entry = grammar::by_name("toml").unwrap();
    let outcome = grammar::acquire(&s.home, entry, None);
    let err = outcome.expect_err("the pinned URL must be refused");
    assert!(err.message.contains("RVOW_GRAMMAR_OFFLINE"), "{}", err.message);
    assert!(!grammar::cache_file(&s.home, entry).is_file());
}

#[test]
fn syntax_drift_offline_flag_does_not_block_a_fixture_base() {
    let _env = env_lock();
    let s = setup("offline-base");
    let entry = grammar::by_name("toml").unwrap();
    let outcome = grammar::acquire(&s.home, entry, Some(&fixtures()));
    assert!(outcome.is_ok(), "{outcome:?}");
}
