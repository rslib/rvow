//! Framework queries name the changed test: a drift item for a marked test
//! unit says "test <file> changed <name>", a project query adds to the
//! shipped one, and a missing or bad query file never makes the check
//! weaker. One test per spec scenario, offline through the fixture folder.

use std::path::{Path, PathBuf};
use std::process::Command;

use rvow_core::store::{self, Project};
use rvow_core::syntax::grammar;
use rvow_core::{change, config, drift, git, plan, proof, vow};

fn fixtures() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/grammars").display().to_string()
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

const APP_JS: &str = "function add(a, b) {\n  return a + b;\n}\n\ntest('adds two numbers', () => {\n  expect(add(1, 2)).toBe(3);\n});\n";

/// An accepted change in a fresh repository with one protected test file.
/// `config` is written into the project config before the vow, so a test
/// that names query files is not itself config drift.
fn setup(name: &str, file: &str, body: &str, config: &str) -> Setup {
    let tmp = std::env::temp_dir().join(format!("rvow-syntax-queries-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let tmp = tmp.canonicalize().unwrap();
    let home = tmp.join("home");
    let repo = tmp.join("r");
    std::fs::create_dir_all(&repo).unwrap();
    git_cmd(&repo, &["init", "-q", "-b", "main"]);
    std::fs::create_dir_all(repo.join("tests")).unwrap();
    std::fs::write(repo.join(file), body).unwrap();
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    git_cmd(&repo, &["add", "."]);
    git_cmd(&repo, &["commit", "-q", "-m", "init"]);

    let project = store::init(&repo, &home, store::Mode::Home).unwrap();
    let c = change::new(&project, "Queries", "main", false).unwrap();
    std::fs::write(
        c.path.join("tasks.md"),
        format!("- TSK-new1 Queries\n  covers: REQ-new1\n  verify: test -f done.marker\n  files: {file}\n"),
    )
    .unwrap();
    std::fs::write(
        c.path.join("specs/out.md"),
        "## ADD REQ-new1 Queries\nrvow SHALL name the changed test with framework queries.\n### Scenario: s\n- WHEN it runs\n- THEN the item names the test\n",
    )
    .unwrap();
    if !config.is_empty() {
        std::fs::write(project.root.join("config.toml"), config).unwrap();
    }
    let prepared = vow::prepare(&project, &home, c, &repo, &mut std::io::sink()).unwrap();
    vow::commit(&project, prepared).unwrap();
    Setup { _tmp: tmp, home, repo, project }
}

fn items(s: &Setup) -> Vec<(drift::Class, String)> {
    let (living, _) = change::load_living(&s.project).unwrap();
    let active = proof::active(&s.project, &living).unwrap().remove(0);
    let top = git::toplevel(&s.repo).unwrap();
    let cfg = config::load(&s.project, &s.home).unwrap();
    drift::drift_with_home(&s.project, Some(&s.home), &top, &cfg, &active).unwrap().into_iter().map(|i| (i.class, i.what)).collect()
}

fn prime(home: &Path, lang: &str) {
    let entry = grammar::by_name(lang).unwrap();
    grammar::acquire(home, entry, Some(&fixtures())).unwrap();
}

#[test]
fn syntax_queries_a_javascript_test_changed_names_the_test() {
    let s = setup("js", "tests/app.test.js", APP_JS, "");
    prime(&s.home, "javascript");
    assert!(items(&s).is_empty());

    let text = std::fs::read_to_string(s.repo.join("tests/app.test.js")).unwrap();
    std::fs::write(s.repo.join("tests/app.test.js"), text.replace("toBe(3)", "toBe(4)")).unwrap();
    let got = items(&s);
    let item = got.iter().find(|(c, _)| *c == drift::Class::Weaken).map(|(_, w)| w.clone()).unwrap_or_default();
    assert!(item.contains("tests/app.test.js"), "{got:?}");
    assert!(item.contains("adds two numbers"), "the item must name the test: {item}");
}

const CHECKS_SCM: &str = "(\n  (call_expression\n    function: (identifier) @fn\n    arguments: (arguments (string (string_fragment) @name))) @test\n  (#eq? @fn \"check\")\n)\n";

#[test]
fn syntax_queries_a_project_query_adds_to_the_shipped_one() {
    let s = setup("project", "tests/app.check.js", &APP_JS.replace("test(", "check("), "test_queries = { javascript = \"checks.scm\" }\n");
    prime(&s.home, "javascript");
    std::fs::write(s.repo.join("checks.scm"), CHECKS_SCM).unwrap();
    assert!(items(&s).is_empty());

    let text = std::fs::read_to_string(s.repo.join("tests/app.check.js")).unwrap();
    std::fs::write(s.repo.join("tests/app.check.js"), text.replace("toBe(3)", "toBe(4)")).unwrap();
    let got = items(&s);
    let item = got.iter().find(|(c, _)| *c == drift::Class::Weaken).map(|(_, w)| w.clone()).unwrap_or_default();
    assert_eq!(item, "test tests/app.check.js changed adds two numbers");
}

#[test]
fn syntax_queries_a_project_query_predicate_that_does_not_match_names_nothing() {
    // The query marks check(...) calls, but the file calls run(...): the
    // predicate never matches, the unit is no marked test, and the item
    // keeps no name.
    let s = setup("predicate", "tests/app.run.js", &APP_JS.replace("test(", "run("), "test_queries = { javascript = \"checks.scm\" }\n");
    prime(&s.home, "javascript");
    std::fs::write(s.repo.join("checks.scm"), CHECKS_SCM).unwrap();

    let text = std::fs::read_to_string(s.repo.join("tests/app.run.js")).unwrap();
    std::fs::write(s.repo.join("tests/app.run.js"), text.replace("toBe(3)", "toBe(4)")).unwrap();
    let got = items(&s);
    let item = got.iter().find(|(c, _)| *c == drift::Class::Weaken).map(|(_, w)| w.clone()).unwrap_or_default();
    assert!(item.contains("tests/app.run.js"), "still drift: {got:?}");
    assert!(!item.contains("adds two numbers"), "a predicate that does not match names nothing: {item}");
}

#[test]
fn syntax_queries_a_rust_test_changed_names_the_test() {
    let s = setup("rust", "tests/unit.rs", "#[test]\nfn a() {\n    assert_eq!(1, 1);\n}\n", "");
    prime(&s.home, "rust");
    assert!(items(&s).is_empty());

    let text = std::fs::read_to_string(s.repo.join("tests/unit.rs")).unwrap();
    std::fs::write(s.repo.join("tests/unit.rs"), text.replace("assert_eq!(1, 1)", "assert_eq!(1, 2)")).unwrap();
    let got = items(&s);
    let item = got.iter().find(|(c, _)| *c == drift::Class::Weaken).map(|(_, w)| w.clone()).unwrap_or_default();
    assert_eq!(item, "test tests/unit.rs changed a");
}

#[test]
fn syntax_queries_a_gtest_case_changed_names_the_test() {
    let s = setup("gtest", "tests/g.cpp", "TEST(Suite, Name) {\n    EXPECT_EQ(1, 1);\n}\n", "");
    prime(&s.home, "cpp");
    assert!(items(&s).is_empty());

    let text = std::fs::read_to_string(s.repo.join("tests/g.cpp")).unwrap();
    std::fs::write(s.repo.join("tests/g.cpp"), text.replace("EXPECT_EQ(1, 1)", "EXPECT_EQ(1, 2)")).unwrap();
    let got = items(&s);
    let item = got.iter().find(|(c, _)| *c == drift::Class::Weaken).map(|(_, w)| w.clone()).unwrap_or_default();
    assert_eq!(item, "test tests/g.cpp changed Suite.Name");
}

#[test]
fn syntax_queries_a_pytest_function_changed_names_the_test() {
    let s = setup("pytest", "tests/test_a.py", "def test_a():\n    assert 1 == 1\n", "");
    prime(&s.home, "python");
    assert!(items(&s).is_empty());

    let text = std::fs::read_to_string(s.repo.join("tests/test_a.py")).unwrap();
    std::fs::write(s.repo.join("tests/test_a.py"), text.replace("1 == 1", "1 == 2")).unwrap();
    let got = items(&s);
    let item = got.iter().find(|(c, _)| *c == drift::Class::Weaken).map(|(_, w)| w.clone()).unwrap_or_default();
    assert_eq!(item, "test tests/test_a.py changed test_a");
}

#[test]
fn syntax_queries_a_bad_project_query_stays_drift_without_the_name() {
    // The file holds check(...) calls, which only the project query marks:
    // a bad project query loses the name but never the drift.
    let s = setup("bad", "tests/app.check.js", &APP_JS.replace("test(", "check("), "test_queries = { javascript = \"bad.scm\" }\n");
    prime(&s.home, "javascript");
    std::fs::write(s.repo.join("bad.scm"), "(this is not a query)").unwrap();
    let text = std::fs::read_to_string(s.repo.join("tests/app.check.js")).unwrap();
    std::fs::write(s.repo.join("tests/app.check.js"), text.replace("toBe(3)", "toBe(4)")).unwrap();
    let got = items(&s);
    let item = got.iter().find(|(c, _)| *c == drift::Class::Weaken).map(|(_, w)| w.clone()).unwrap_or_default();
    assert!(item.contains("tests/app.check.js"), "still drift: {got:?}");
    assert!(!item.contains("adds two numbers"), "a bad query names nothing: {item}");
    // The bad file is a rvow check warning that names it.
    let reports = plan::check(&s.project, &s.repo, None).unwrap();
    let warned = reports.iter().flat_map(|r| &r.findings).any(|(_, d)| d.message.contains("bad.scm"));
    assert!(
        warned,
        "rvow check warns about the bad query: {:?}",
        reports.iter().flat_map(|r| &r.findings).map(|(_, d)| &d.message).collect::<Vec<_>>()
    );
}
