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
    _tmp: crate::temp::TempDir,
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
    let guard = crate::temp::TempDir::new(&format!("syntax-units-{name}"));
    let tmp = guard.path().to_path_buf();
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
    Setup { _tmp: guard, home, repo, project }
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
fn syntax_units_untrusted_project_query_does_not_make_an_assignment_free() {
    // The project query marks every assignment as a test, but its file is
    // not there at accept, so it is not protected and only names tests:
    // global.expect = ... is still drift.
    let cfg = "test_queries = { javascript = \"assign.scm\" }\n";
    let s = setup_cfg("project-assign", &[("tests/app.test.js", APP_JS_UNITS), ("tests/t.sh", T_SH)], cfg);
    prime(&s.home, "javascript");
    std::fs::write(s.repo.join("assign.scm"), "(expression_statement (assignment_expression) @test)\n").unwrap();
    assert!(items(&s).is_empty());
    rewrite_at(&s, "tests/app.test.js", &format!("{APP_JS_UNITS}global.expect = jest.fn();\n"));
    assert_eq!(weaken_for(&s, "app.test.js"), ["protected file tests/app.test.js changed".to_string()]);
}

#[test]
fn syntax_units_trusted_project_query_frees_an_assignment_it_names() {
    // The same query, committed before accept, is protected and trusted. A
    // mark with no @name never frees a unit; a mark that names the unit frees
    // it under the free rules of a shipped mark.
    let cfg = "test_queries = { javascript = \"assign.scm\" }\n";
    let unnamed = "(expression_statement (assignment_expression) @test)\n";
    let s = setup_cfg("trusted-assign", &[("tests/app.test.js", APP_JS_UNITS), ("tests/t.sh", T_SH), ("assign.scm", unnamed)], cfg);
    prime(&s.home, "javascript");
    assert!(items(&s).is_empty());
    rewrite_at(&s, "tests/app.test.js", &format!("{APP_JS_UNITS}global.expect = jest.fn();\n"));
    assert_eq!(
        weaken_for(&s, "app.test.js"),
        ["protected file tests/app.test.js changed".to_string()],
        "a mark with no name frees nothing"
    );

    let named = "(expression_statement (assignment_expression left: (member_expression property: (property_identifier) @name)) @test)\n";
    let s = setup_cfg("trusted-assign-named", &[("tests/app.test.js", APP_JS_UNITS), ("tests/t.sh", T_SH), ("assign.scm", named)], cfg);
    prime(&s.home, "javascript");
    rewrite_at(&s, "tests/app.test.js", &format!("{APP_JS_UNITS}global.mocked = jest.fn();\n"));
    assert!(items(&s).is_empty(), "a trusted mark that names the unit frees it: {:?}", items(&s));
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

fn exit_drift(name: &str, file: &str, lang: &str, old: &str, new: &str) {
    let s = setup_with(name, &[(file, old), ("tests/t.sh", T_SH)]);
    prime(&s.home, lang);
    assert!(items(&s).is_empty());
    rewrite_at(&s, file, new);
    assert_eq!(weaken_for(&s, file), [format!("protected file {file} changed")], "a new test that exits is drift");
}

#[test]
fn syntax_units_exit_rust_test_is_drift() {
    for (i, call) in ["std::process::exit(0)", "process::exit(0)", "std::process::abort()", "::std::process::exit(1)"].iter().enumerate() {
        let new = format!("{UNIT_RS}\n#[test]\nfn c() {{\n    {call};\n}}\n");
        exit_drift(&format!("exit-rs-{i}"), "tests/unit.rs", "rust", UNIT_RS, &new);
    }
}

#[test]
fn syntax_units_exit_python_test_is_drift() {
    for (i, call) in ["sys.exit(0)", "os._exit(1)", "os.abort()", "exit()", "quit()"].iter().enumerate() {
        let new = format!("{PY_TEST}\ndef test_b():\n    {call}\n");
        exit_drift(&format!("exit-py-{i}"), "tests/test_app.py", "python", PY_TEST, &new);
    }
}

#[test]
fn syntax_units_exit_js_test_is_drift() {
    for (i, call) in ["process.exit(1)", "process.abort()", "process.reallyExit(1)"].iter().enumerate() {
        let new = format!("{DESCRIBE_JS}it('b', () => {{\n  {call};\n}});\n");
        exit_drift(&format!("exit-js-{i}"), "tests/app.test.js", "javascript", DESCRIBE_JS, &new);
    }
}

#[test]
fn syntax_units_exit_js_test_inside_describe_is_drift() {
    let new = "describe('group', () => {\n  it('a', () => {\n    expect(1).toBe(1);\n  });\n  it('b', () => {\n    process.exit(1);\n  });\n});\n";
    exit_drift("exit-js-group", "tests/app.test.js", "javascript", DESCRIBE_JS, new);
}

#[test]
fn syntax_units_exit_cpp_test_is_drift() {
    let old = "TEST(Calc, Adds) {\n  EXPECT_EQ(1, 1);\n}\n";
    for (i, call) in ["exit(1)", "_exit(1)", "_Exit(1)", "quick_exit(1)", "abort()", "std::exit(1)", "std::abort()"].iter().enumerate() {
        let new = format!("{old}\nTEST(Calc, Subtracts) {{\n  {call};\n}}\n");
        exit_drift(&format!("exit-cpp-{i}"), "tests/calc_test.cpp", "cpp", old, &new);
    }
}

#[test]
fn syntax_units_exit_only_named_is_no_drift() {
    let s = setup("exit-named");
    prime_rust(&s.home);
    assert!(items(&s).is_empty());
    let added = format!("{UNIT_RS}\n#[test]\nfn c() {{\n    assert_eq!(code(\"exit\"), 1);\n}}\n");
    rewrite(&s, "unit.rs", &added);
    assert!(items(&s).is_empty(), "a test that only names exit is free: {:?}", items(&s));
}

const SPEC_RB: &str = "describe('group') do\n  it('a') do\n    expect(1).to eq(1)\n  end\nend\n";

#[test]
fn syntax_units_exit_by_name_python_getattr_is_drift() {
    for (i, call) in
        ["getattr(sys, \"exit\")(0)", "getattr(os, \"_exit\")(1)", "getattr(__import__(\"os\"), \"abort\")()"].iter().enumerate()
    {
        let new = format!("{PY_TEST}def test_b():\n    {call}\n");
        exit_drift(&format!("exit-name-py-{i}"), "tests/test_app.py", "python", PY_TEST, &new);
    }
}

#[test]
fn syntax_units_exit_by_name_javascript_subscript_and_reflect_are_drift() {
    for (i, call) in ["process[\"exit\"](0)", "Reflect.get(process, \"exit\")(0)", "process[\"reallyExit\"](0)"].iter().enumerate() {
        let new = format!("{DESCRIBE_JS}it('b', () => {{\n  {call};\n}});\n");
        exit_drift(&format!("exit-name-js-{i}"), "tests/app.test.js", "javascript", DESCRIBE_JS, &new);
    }
}

#[test]
fn syntax_units_exit_by_name_ruby_symbol_is_drift() {
    let new = format!("{SPEC_RB}it('b') do\n  Kernel.send(:exit)\nend\n");
    exit_drift("exit-name-rb", "tests/app_spec.rb", "ruby", SPEC_RB, &new);
}

#[test]
fn syntax_units_exit_by_name_a_string_that_only_names_exit_is_free() {
    let s = setup_with("exit-name-free", &[("tests/app.test.js", DESCRIBE_JS), ("tests/t.sh", T_SH)]);
    prime(&s.home, "javascript");
    assert!(items(&s).is_empty());
    let new = format!(
        "{DESCRIBE_JS}it('b', () => {{\n  expect(code(\"exit\")).toBe(1);\n  const key = \"exit\";\n  expect(key).toBe('exit');\n}});\n"
    );
    rewrite_at(&s, "tests/app.test.js", &new);
    assert!(items(&s).is_empty(), "a string that only names exit is free: {:?}", items(&s));
}

#[test]
fn syntax_units_exit_variable_named_exit_is_no_drift() {
    let s = setup("exit-variable");
    prime_rust(&s.home);
    let added = format!("{UNIT_RS}\n#[test]\nfn c() {{\n    let exit = 1;\n    assert_eq!(exit, 1);\n}}\n");
    rewrite(&s, "unit.rs", &added);
    assert!(items(&s).is_empty(), "a variable named exit is no call: {:?}", items(&s));
}

#[test]
fn syntax_units_exit_other_spellings_are_drift() {
    let rust =
        ["dbg!(std::process::exit(0))", "println!(\"{}\", std::process::exit(0))", "(std::process::exit)(0)", "unsafe { libc::_exit(0) }"];
    for (i, call) in rust.iter().enumerate() {
        let new = format!("{UNIT_RS}\n#[test]\nfn c() {{\n    {call};\n}}\n");
        exit_drift(&format!("exit-rs-more-{i}"), "tests/unit.rs", "rust", UNIT_RS, &new);
    }
    for (i, call) in ["pytest.exit('x', returncode=0)", "__import__('os')._exit(0)"].iter().enumerate() {
        let new = format!("{PY_TEST}\ndef test_b():\n    {call}\n");
        exit_drift(&format!("exit-py-more-{i}"), "tests/test_app.py", "python", PY_TEST, &new);
    }
    let js = [
        "globalThis.process.exit(0)",
        "require('process').exit(0)",
        "process.exit?.(0)",
        "process.exit.call(process, 0)",
        "(process.exit)(0)",
    ];
    for (i, call) in js.iter().enumerate() {
        let new = format!("{DESCRIBE_JS}it('b', () => {{\n  {call};\n}});\n");
        exit_drift(&format!("exit-js-more-{i}"), "tests/app.test.js", "javascript", DESCRIBE_JS, &new);
    }
}

const MOD_TEST_RS: &str = "#[test]\nfn w() {\n    assert_eq!(1, 1);\n}\n";

const MOD_MAIN_RS: &str = "mod status;\n";

fn mod_setup(name: &str, main: &str, extra: &[(&str, &str)]) -> Setup {
    let mut files = vec![(main, "mod status;\n"), ("tests/it/status.rs", MOD_TEST_RS), ("tests/t.sh", T_SH)];
    files.extend_from_slice(extra);
    let s = setup_with(name, &files);
    prime_rust(&s.home);
    assert!(items(&s).is_empty());
    s
}

fn write_new(s: &Setup, rel: &str, text: &str) {
    if let Some(dir) = Path::new(rel).parent() {
        std::fs::create_dir_all(s.repo.join(dir)).unwrap();
    }
    rewrite_at(s, rel, text);
}

#[test]
fn syntax_units_mod_new_module_is_no_drift() {
    let s = mod_setup("mod-new", "tests/it/main.rs", &[]);
    write_new(&s, "tests/it/main.rs", &format!("{MOD_MAIN_RS}mod walk;\n"));
    write_new(&s, "tests/it/walk.rs", MOD_TEST_RS);
    assert!(items(&s).is_empty(), "a new module with a test is free: {:?}", items(&s));
}

#[test]
fn syntax_units_mod_nested_dir_module_is_no_drift() {
    let s = mod_setup("mod-dir", "tests/it/main.rs", &[]);
    write_new(&s, "tests/it/main.rs", &format!("{MOD_MAIN_RS}#[cfg(test)]\nmod walk;\n"));
    write_new(&s, "tests/it/walk/mod.rs", MOD_TEST_RS);
    assert!(items(&s).is_empty(), "a new walk/mod.rs is free: {:?}", items(&s));
}

#[test]
fn syntax_units_mod_path_attribute_is_no_drift() {
    let s = mod_setup("mod-path", "tests/main.rs", &[("tests/suite/a.rs", MOD_TEST_RS)]);
    write_new(&s, "tests/main.rs", "mod status;\n#[path = \"suite/b.rs\"]\nmod b;\n");
    write_new(&s, "tests/suite/b.rs", MOD_TEST_RS);
    assert_eq!(weaken_for(&s, "tests/main.rs"), Vec::<String>::new(), "a path module is free: {:?}", items(&s));
}

#[test]
fn syntax_units_mod_that_exits_is_drift() {
    let s = mod_setup("mod-exit", "tests/it/main.rs", &[]);
    write_new(&s, "tests/it/main.rs", &format!("{MOD_MAIN_RS}mod walk;\n"));
    write_new(&s, "tests/it/walk.rs", "#[test]\nfn w() {\n    std::process::exit(0);\n}\n");
    assert_eq!(weaken_for(&s, "tests/it/main.rs"), ["protected file tests/it/main.rs changed".to_string()]);
}

#[test]
fn syntax_units_mod_existing_file_is_drift() {
    let s = mod_setup("mod-old", "tests/it/main.rs", &[("tests/it/old.rs", MOD_TEST_RS)]);
    write_new(&s, "tests/it/main.rs", &format!("{MOD_MAIN_RS}mod old;\n"));
    assert_eq!(weaken_for(&s, "tests/it/main.rs"), ["protected file tests/it/main.rs changed".to_string()]);
}

#[test]
fn syntax_units_mod_other_attribute_is_drift() {
    let s = mod_setup("mod-attr", "tests/it/main.rs", &[]);
    write_new(&s, "tests/it/main.rs", &format!("{MOD_MAIN_RS}#[macro_use]\nmod walk;\n"));
    write_new(&s, "tests/it/walk.rs", MOD_TEST_RS);
    assert_eq!(weaken_for(&s, "tests/it/main.rs"), ["protected file tests/it/main.rs changed".to_string()]);
}

#[test]
fn syntax_units_mod_missing_file_is_drift() {
    let s = mod_setup("mod-missing", "tests/it/main.rs", &[]);
    write_new(&s, "tests/it/main.rs", &format!("{MOD_MAIN_RS}mod walk;\n"));
    assert_eq!(weaken_for(&s, "tests/it/main.rs"), ["protected file tests/it/main.rs changed".to_string()]);
}

#[test]
fn syntax_units_mod_that_reaches_other_code_is_drift() {
    let walks = [
        "mod sub;\n\n#[test]\nfn w() {\n    assert_eq!(1, 1);\n}\n",
        "include!(\"sub.rs\");\n\n#[test]\nfn w() {\n    assert_eq!(1, 1);\n}\n",
        "#[ctor::ctor]\nfn start() {}\n\n#[test]\nfn w() {\n    assert_eq!(1, 1);\n}\n",
    ];
    for (i, walk) in walks.iter().enumerate() {
        let s = mod_setup(&format!("mod-reach-{i}"), "tests/it/main.rs", &[]);
        write_new(&s, "tests/it/main.rs", &format!("{MOD_MAIN_RS}mod walk;\n"));
        write_new(&s, "tests/it/walk.rs", walk);
        write_new(&s, "tests/it/walk/sub.rs", "#[test]\nfn x() {\n    std::process::exit(0);\n}\n");
        write_new(&s, "tests/it/sub.rs", "#[test]\nfn x() {\n    std::process::exit(0);\n}\n");
        assert_eq!(weaken_for(&s, "tests/it/main.rs"), ["protected file tests/it/main.rs changed".to_string()], "{walk}");
    }
}

#[test]
fn syntax_units_exit_by_name_other_lookups_and_backticks_are_drift() {
    for (i, call) in ["Kernel.public_send(:exit)", "Kernel.__send__(:exit)", "Kernel.method(:exit).call"].iter().enumerate() {
        let new = format!("{SPEC_RB}it('b') do\n  {call}\nend\n");
        exit_drift(&format!("exit-name-rb-more-{i}"), "tests/app_spec.rb", "ruby", SPEC_RB, &new);
    }
    for (i, call) in ["process[`exit`](0)", "Reflect.get(process, `exit`)(0)"].iter().enumerate() {
        let new = format!("{DESCRIBE_JS}it('b', () => {{\n  {call};\n}});\n");
        exit_drift(&format!("exit-name-js-tick-{i}"), "tests/app.test.js", "javascript", DESCRIBE_JS, &new);
    }
}

#[test]
fn syntax_units_exit_by_name_on_other_objects_is_free() {
    let s = setup_with("exit-name-other", &[("tests/app.test.js", DESCRIBE_JS), ("tests/t.sh", T_SH)]);
    prime(&s.home, "javascript");
    assert!(items(&s).is_empty());
    let new = format!(
        "{DESCRIBE_JS}it('b', () => {{\n  ws.send('exit');\n  expect(cfg['exit']).toBe(1);\n  expect(settings.get('exit')).toBe(1);\n}});\n"
    );
    rewrite_at(&s, "tests/app.test.js", &new);
    assert!(items(&s).is_empty(), "a key named exit on another object is free: {:?}", items(&s));
}
