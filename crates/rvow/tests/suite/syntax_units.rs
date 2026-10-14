//! Protected test files compared by syntax unit: a moved test, a new test or
//! a new helper is no drift, a changed assert is, and a file whose grammar
//! does not load keeps today's rule. One test per spec scenario, offline
//! through the fixture folder.

use std::path::{Path, PathBuf};
use std::process::Command;

use rvow_core::store::{self, Project};
use rvow_core::syntax::grammar;
use rvow_core::{change, config, drift, git, proof, vow};

fn fixtures() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/grammars").display().to_string()
}

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
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

const UNIT_RS: &str = "fn helper() -> u32 {\n    1\n}\n\n#[test]\nfn a() {\n    assert_eq!(helper(), 1);\n}\n\n#[test]\nfn b() {\n    assert_ne!(helper(), 2);\n}\n";

/// The same tests with b above a and no other edit.
const UNIT_RS_MOVED: &str = "fn helper() -> u32 {\n    1\n}\n\n#[test]\nfn b() {\n    assert_ne!(helper(), 2);\n}\n\n#[test]\nfn a() {\n    assert_eq!(helper(), 1);\n}\n";

const T_SH: &str = "check_one\ncheck_two\nexit 0\n";

/// An accepted change in a fresh repository. `tests/unit.rs` and `tests/t.sh`
/// are protected test files.
fn setup(name: &str) -> Setup {
    setup_with(name, &[("tests/unit.rs", UNIT_RS), ("tests/t.sh", T_SH)])
}

/// An accepted change in a fresh repository whose protected test files are
/// the given paths and bodies. The task scope covers every file.
fn setup_with(name: &str, files: &[(&str, &str)]) -> Setup {
    setup_cfg(name, files, "")
}

/// Like `setup_with`, and `config` is written into the project config before
/// the vow, so a test that names query files is not itself config drift.
fn setup_cfg(name: &str, files: &[(&str, &str)], config: &str) -> Setup {
    let tmp = std::env::temp_dir().join(format!("rvow-syntax-units-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let tmp = tmp.canonicalize().unwrap();
    let home = tmp.join("home");
    let repo = tmp.join("r");
    std::fs::create_dir_all(&repo).unwrap();
    git_cmd(&repo, &["init", "-q", "-b", "main"]);
    for (rel, body) in files {
        if let Some(dir) = Path::new(rel).parent() {
            std::fs::create_dir_all(repo.join(dir)).unwrap();
        }
        std::fs::write(repo.join(rel), body).unwrap();
    }
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    git_cmd(&repo, &["add", "."]);
    git_cmd(&repo, &["commit", "-q", "-m", "init"]);

    let project = store::init(&repo, &home, store::Mode::Home).unwrap();
    if !config.is_empty() {
        std::fs::write(project.root.join("config.toml"), config).unwrap();
    }
    let c = change::new(&project, "Units", "main", false).unwrap();
    let scope = files.iter().map(|(rel, _)| *rel).collect::<Vec<_>>().join(", ");
    std::fs::write(
        c.path.join("tasks.md"),
        format!("- TSK-new1 Units\n  covers: REQ-new1\n  verify: test -f done.marker\n  files: {scope}\n"),
    )
    .unwrap();
    std::fs::write(
        c.path.join("specs/out.md"),
        "## ADD REQ-new1 Units\nrvow SHALL judge protected test files by syntax unit.\n### Scenario: s\n- WHEN it runs\n- THEN units compare\n",
    )
    .unwrap();
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

fn rewrite(s: &Setup, name: &str, text: &str) {
    std::fs::write(s.repo.join("tests").join(name), text).unwrap();
}

fn edit_of(s: &Setup, name: &str, from: &str, to: &str) -> String {
    let text = std::fs::read_to_string(s.repo.join("tests").join(name)).unwrap();
    assert!(text.contains(from), "{from} in {text}");
    text.replace(from, to)
}

fn prime_rust(home: &Path) {
    let entry = grammar::by_name("rust").unwrap();
    grammar::acquire(home, entry, Some(&fixtures())).unwrap();
}

const MOD_RS: &str = "mod tests {\n    #[test]\n    fn a() {\n        assert_eq!(1, 1);\n    }\n}\n";

const DESCRIBE_JS: &str = "describe('group', () => {\n  it('a', () => {\n    expect(1).toBe(1);\n  });\n});\n";

fn weaken_for(s: &Setup, file: &str) -> Vec<String> {
    items(s).into_iter().filter(|(c, _)| *c == drift::Class::Weaken).map(|(_, w)| w).filter(|w| w.contains(file)).collect()
}

#[test]
fn syntax_units_moved_test_is_no_drift() {
    let s = setup("moved");
    prime_rust(&s.home);
    assert!(items(&s).is_empty());

    rewrite(&s, "unit.rs", UNIT_RS_MOVED);
    assert!(items(&s).is_empty(), "a moved test is not drift: {:?}", items(&s));
}

#[test]
fn syntax_units_added_test_is_no_drift() {
    let s = setup("added");
    prime_rust(&s.home);
    assert!(items(&s).is_empty());

    let added = format!("{UNIT_RS}\n#[test]\nfn c() {{\n    assert_eq!(helper() + 1, 2);\n}}\n");
    rewrite(&s, "unit.rs", &added);
    assert!(items(&s).is_empty(), "a new test is free: {:?}", items(&s));
}

#[test]
fn syntax_units_added_rust_helper_is_drift() {
    // A helper is no test, so no shipped query marks it free: it is drift.
    let s = setup("helper");
    prime_rust(&s.home);
    assert!(items(&s).is_empty());

    let helper = edit_of(&s, "unit.rs", "#[test]\nfn a()", "fn d() -> u32 {\n    3\n}\n\n#[test]\nfn a()");
    rewrite(&s, "unit.rs", &helper);
    assert_eq!(weaken_for(&s, "tests/unit.rs"), ["protected file tests/unit.rs changed".to_string()], "a new helper is drift");
}

fn prime(home: &Path, lang: &str) {
    let entry = grammar::by_name(lang).unwrap();
    grammar::acquire(home, entry, Some(&fixtures())).unwrap();
}

#[test]
fn syntax_units_rust_comment_above_test_is_no_drift() {
    // A plain // comment is left out of a unit form, as guard_rust ignores it.
    let s = setup("rust-comment");
    prime_rust(&s.home);
    assert!(items(&s).is_empty());
    rewrite(&s, "unit.rs", &edit_of(&s, "unit.rs", "#[test]\nfn a()", "// note\n#[test]\nfn a()"));
    assert!(items(&s).is_empty(), "a plain comment is no drift: {:?}", items(&s));
}

const PY_TEST: &str = "def test_a():\n    assert 1 == 1\n";

#[test]
fn syntax_units_python_hook_added_is_drift() {
    // setup_function is no test, so it is drift even though it names no old unit.
    let s = setup_with("py-hook", &[("tests/test_app.py", PY_TEST), ("tests/t.sh", T_SH)]);
    prime(&s.home, "python");
    assert!(items(&s).is_empty());
    let hook = format!("def setup_function(function):\n    pytest.skip('x')\n\n{PY_TEST}");
    rewrite_at(&s, "tests/test_app.py", &hook);
    assert_eq!(weaken_for(&s, "test_app.py"), ["protected file tests/test_app.py changed".to_string()]);
}

#[test]
fn syntax_units_python_test_redefined_is_drift() {
    // A second def test_a reuses a name the old text already uses.
    let s = setup_with("py-redefined", &[("tests/test_app.py", PY_TEST), ("tests/t.sh", T_SH)]);
    prime(&s.home, "python");
    assert!(items(&s).is_empty());
    let twice = format!("{PY_TEST}\ndef test_a():\n    assert 2 == 2\n");
    rewrite_at(&s, "tests/test_app.py", &twice);
    assert_eq!(weaken_for(&s, "test_app.py"), ["protected file tests/test_app.py changed".to_string()]);
}

#[test]
fn syntax_units_python_fixture_added_is_drift() {
    // An autouse fixture with a fresh name is no test attribute, so it is drift.
    let s = setup_with("py-fixture", &[("tests/test_app.py", PY_TEST), ("tests/t.sh", T_SH)]);
    prime(&s.home, "python");
    assert!(items(&s).is_empty());
    let fixture = format!("@pytest.fixture(autouse=True)\ndef fresh_setup():\n    pass\n\n{PY_TEST}");
    rewrite_at(&s, "tests/test_app.py", &fixture);
    assert_eq!(weaken_for(&s, "test_app.py"), ["protected file tests/test_app.py changed".to_string()]);
}

#[test]
fn syntax_units_python_fixture_named_like_a_test_is_drift() {
    // A test_* name does not free an autouse fixture: its decorators are
    // checked like prefix attributes.
    let s = setup_with("py-fixture-test-name", &[("tests/test_app.py", PY_TEST), ("tests/t.sh", T_SH)]);
    prime(&s.home, "python");
    assert!(items(&s).is_empty());
    let fixture = format!("@pytest.fixture(autouse=True)\ndef test_zzz():\n    pytest.skip('off')\n\n{PY_TEST}");
    rewrite_at(&s, "tests/test_app.py", &fixture);
    assert_eq!(weaken_for(&s, "test_app.py"), ["protected file tests/test_app.py changed".to_string()]);
}

const PY_CLASS_MOVED_OUT: &str = "class TestBox:\n    def test_a(self):\n        assert 1 == 1\n\ndef test_b():\n    assert 2 == 2\n";

#[test]
fn syntax_units_python_test_moved_out_of_class_is_drift() {
    // Units keep their container: test_a leaves the class, so its old unit
    // is gone and the new module-level test_a is no free unit.
    let s = setup_with("py-moved", &[("tests/test_box.py", PY_CLASS_MOVED_OUT), ("tests/t.sh", T_SH)]);
    prime(&s.home, "python");
    assert!(items(&s).is_empty());
    let moved = "def test_b():\n    assert 2 == 2\n\ndef test_a(self):\n    assert 1 == 1\n\nclass TestBox:\n    pass\n";
    rewrite_at(&s, "tests/test_box.py", moved);
    assert_eq!(weaken_for(&s, "test_box.py"), ["test tests/test_box.py changed test_a".to_string()]);
}

#[test]
fn syntax_units_python_test_added_in_decorated_class_is_no_drift() {
    // A decorated class splits like a class, and its header keeps the
    // decorator, so a new test method inside it is free.
    let body = "@pytest.mark.slow\nclass TestBox:\n    def test_a(self):\n        assert 1 == 1\n";
    let s = setup_with("py-decorated", &[("tests/test_box.py", body), ("tests/t.sh", T_SH)]);
    prime(&s.home, "python");
    assert!(items(&s).is_empty());
    let added = format!("{body}    def test_b(self):\n        assert 2 == 2\n");
    rewrite_at(&s, "tests/test_box.py", &added);
    assert!(items(&s).is_empty(), "a new test in a decorated class is free: {:?}", items(&s));
}

const DESCRIBE_JS_MOVED: &str = "describe('group', () => {\n});\nit('a', () => {\n  expect(1).toBe(1);\n});\n";

#[test]
fn syntax_units_js_test_moved_out_of_describe_is_drift() {
    // The test leaves the describe, so its old unit (keyed with the describe
    // header) is gone and the top-level it is no free unit.
    let s = setup_with("js-moved", &[("tests/app.test.js", DESCRIBE_JS), ("tests/t.sh", T_SH)]);
    prime(&s.home, "javascript");
    assert!(items(&s).is_empty());
    rewrite_at(&s, "tests/app.test.js", DESCRIBE_JS_MOVED);
    assert_eq!(weaken_for(&s, "app.test.js"), ["test tests/app.test.js changed a".to_string()]);
}

#[test]
fn syntax_units_js_import_added_is_drift() {
    // An import is no test, so it is drift.
    let s = setup_with("js-import", &[("tests/app.test.js", DESCRIBE_JS), ("tests/t.sh", T_SH)]);
    prime(&s.home, "javascript");
    assert!(items(&s).is_empty());
    rewrite_at(&s, "tests/app.test.js", &format!("import './x.js';\n{DESCRIBE_JS}"));
    assert_eq!(weaken_for(&s, "app.test.js"), ["protected file tests/app.test.js changed".to_string()]);
}

#[test]
fn syntax_units_cpp_include_added_is_drift() {
    // An include is no test, so it is drift.
    let cpp = "TEST(Suite, Adds) {\n  EXPECT_EQ(1, 1);\n}\n";
    let s = setup_with("cpp-include", &[("tests/unit.cpp", cpp), ("tests/t.sh", T_SH)]);
    prime(&s.home, "cpp");
    assert!(items(&s).is_empty());
    rewrite_at(&s, "tests/unit.cpp", &format!("#include \"x.h\"\n{cpp}"));
    assert_eq!(weaken_for(&s, "unit.cpp"), ["protected file tests/unit.cpp changed".to_string()]);
}

#[test]
fn syntax_units_jsdoc_before_first_test_is_drift() {
    // A test added first in the file is free only without a comment above it:
    // a docblock before the first old unit makes it drift. Without the
    // docblock the same test is free.
    let s = setup_with("jsdoc", &[("tests/app.test.js", APP_JS_UNITS), ("tests/t.sh", T_SH)]);
    prime(&s.home, "javascript");
    assert!(items(&s).is_empty());
    let plain = format!("test('adds three numbers', () => {{\n  expect(3).toBe(3);\n}});\n{APP_JS_UNITS}");
    rewrite_at(&s, "tests/app.test.js", &plain);
    assert!(items(&s).is_empty(), "a plain new test first is free: {:?}", items(&s));
    let docblock = format!("/** @jest-environment node */\n{plain}");
    rewrite_at(&s, "tests/app.test.js", &docblock);
    assert_eq!(weaken_for(&s, "app.test.js"), ["protected file tests/app.test.js changed".to_string()]);
}

const APP_JS_UNITS: &str =
    "function add(a, b) {\n  return a + b;\n}\n\ntest('adds two numbers', () => {\n  expect(add(1, 2)).toBe(3);\n});\n";

#[test]
fn syntax_units_project_query_does_not_make_an_assignment_free() {
    // The project query marks every assignment as a test. Only shipped
    // queries make a unit free, so global.expect = ... is still drift.
    let cfg = "test_queries = { javascript = \"assign.scm\" }\n";
    let s = setup_cfg("project-assign", &[("tests/app.test.js", APP_JS_UNITS), ("tests/t.sh", T_SH)], cfg);
    prime(&s.home, "javascript");
    std::fs::write(s.repo.join("assign.scm"), "(expression_statement (assignment_expression) @test)\n").unwrap();
    assert!(items(&s).is_empty());
    rewrite_at(&s, "tests/app.test.js", &format!("{APP_JS_UNITS}global.expect = jest.fn();\n"));
    assert_eq!(weaken_for(&s, "app.test.js"), ["protected file tests/app.test.js changed".to_string()]);
}

fn rewrite_at(s: &Setup, rel: &str, text: &str) {
    std::fs::write(s.repo.join(rel), text).unwrap();
}

#[test]
fn syntax_units_changed_assert_is_drift() {
    let s = setup("assert");
    prime_rust(&s.home);
    rewrite(&s, "unit.rs", &edit_of(&s, "unit.rs", "assert_eq!(helper(), 1);", "assert_eq!(helper(), 2);"));
    let got = items(&s);
    assert!(got.iter().any(|(c, w)| *c == drift::Class::Weaken && w.contains("tests/unit.rs")), "{got:?}");
}

#[test]
fn syntax_units_no_grammar_keeps_the_line_rule() {
    let _env = env_lock();
    // SAFETY: under the env lock, and the var is read only by this test.
    unsafe {
        std::env::set_var("RVOW_GRAMMAR_OFFLINE", "1");
    }
    let s = setup("no-grammar");
    // The bash grammar is not in the cache and offline is on, so the shell
    // file keeps today's rule: a removed check line is drift and the item
    // names the fetch, exactly as today's drift check prints it.
    rewrite(&s, "t.sh", &edit_of(&s, "t.sh", "check_two\n", ""));
    let got = items(&s);
    let item = "protected file tests/t.sh changed (the bash grammar could not load: run `rvow grammar fetch bash`, or check RVOW_GRAMMAR_URL when the download base is overridden)";
    assert!(got.contains(&(drift::Class::Weaken, item.to_string())), "{got:?}");
    // The var stays set: .cargo/config.toml sets it for the whole test
    // process, and removing it would let a concurrent test download.
}

#[test]
fn syntax_units_test_turned_off_is_drift() {
    // Removing #[test] from test b changes its unit: the attribute is part
    // of the unit, and the old query still names the test b.
    let s = setup("turned-off-attr");
    prime_rust(&s.home);
    assert!(items(&s).is_empty());
    rewrite(&s, "unit.rs", &edit_of(&s, "unit.rs", "#[test]\nfn b()", "fn b()"));
    assert_eq!(weaken_for(&s, "tests/unit.rs"), ["test tests/unit.rs changed b".to_string()]);

    // Adding #[ignore] to test b changes its unit too.
    let s = setup("turned-off-ignore");
    prime_rust(&s.home);
    assert!(items(&s).is_empty());
    rewrite(&s, "unit.rs", &edit_of(&s, "unit.rs", "#[test]\nfn b()", "#[test]\n#[ignore]\nfn b()"));
    assert_eq!(weaken_for(&s, "tests/unit.rs"), ["test tests/unit.rs changed b".to_string()]);
}

#[test]
fn syntax_units_assert_shadowed_is_drift() {
    // A macro that shadows assert_eq is a new unit that is no test,
    // function or import, so it is drift even though every old unit
    // keeps its form.
    let s = setup("shadowed");
    prime_rust(&s.home);
    assert!(items(&s).is_empty());
    rewrite(&s, "unit.rs", &format!("{UNIT_RS}macro_rules! assert_eq {{ ($($t:tt)*) => {{}}; }}\n"));
    let got = weaken_for(&s, "tests/unit.rs");
    assert_eq!(got, ["protected file tests/unit.rs changed".to_string()], "the shadowing macro is drift");
}

#[test]
fn syntax_units_duplicate_removed_is_drift() {
    // Two identical tests count twice: deleting one is drift.
    let dup = "#[test]\nfn a() {\n    assert_eq!(1, 1);\n}\n\n#[test]\nfn a() {\n    assert_eq!(1, 1);\n}\n";
    let s = setup_with("duplicate", &[("tests/unit.rs", dup), ("tests/t.sh", T_SH)]);
    prime_rust(&s.home);
    assert!(items(&s).is_empty());
    rewrite(&s, "unit.rs", "#[test]\nfn a() {\n    assert_eq!(1, 1);\n}\n");
    assert_eq!(weaken_for(&s, "tests/unit.rs"), ["test tests/unit.rs changed a".to_string()]);
}

#[test]
fn syntax_units_test_added_inside_mod_is_no_drift() {
    // A mod splits into a header unit plus one unit per body item, so a
    // new test inside it is its own free unit.
    let s = setup_with("mod-added", &[("tests/unit.rs", MOD_RS), ("tests/t.sh", T_SH)]);
    prime_rust(&s.home);
    assert!(items(&s).is_empty());
    rewrite(
        &s,
        "unit.rs",
        "mod tests {\n    #[test]\n    fn a() {\n        assert_eq!(1, 1);\n    }\n\n    #[test]\n    fn b() {\n        assert_eq!(2, 2);\n    }\n}\n",
    );
    assert!(items(&s).is_empty(), "a new test inside the mod is free: {:?}", items(&s));
}

#[test]
fn syntax_units_js_test_added_inside_describe_is_no_drift() {
    // A describe the query marks @group splits into a header unit plus one
    // unit per body item, so a new it(...) inside it is its own free unit.
    let s = setup_with("describe", &[("tests/app.test.js", DESCRIBE_JS), ("tests/t.sh", T_SH)]);
    let entry = grammar::by_name("javascript").unwrap();
    grammar::acquire(&s.home, entry, Some(&fixtures())).unwrap();
    assert!(items(&s).is_empty());
    rewrite(
        &s,
        "app.test.js",
        "describe('group', () => {\n  it('a', () => {\n    expect(1).toBe(1);\n  });\n  it('b', () => {\n    expect(2).toBe(2);\n  });\n});\n",
    );
    assert!(items(&s).is_empty(), "a new test inside the describe is free: {:?}", items(&s));
}

#[test]
fn syntax_units_renamed_mod_header_is_drift() {
    // The mod header is its own unit, and its tests key on it, so renaming
    // the mod is drift and names the test inside it.
    let s = setup_with("mod-renamed", &[("tests/unit.rs", MOD_RS), ("tests/t.sh", T_SH)]);
    prime_rust(&s.home);
    assert!(items(&s).is_empty());
    rewrite(&s, "unit.rs", &edit_of(&s, "unit.rs", "mod tests {", "mod other {"));
    let got = weaken_for(&s, "tests/unit.rs");
    assert_eq!(got, ["test tests/unit.rs changed a".to_string()], "the renamed mod is drift");
}
