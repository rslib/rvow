//! Framework queries name the changed test: a drift item for a marked test
//! unit says "test <file> changed <name>", a project query adds to the
//! shipped one, and a missing or bad query file never makes the check
//! weaker. One test per spec scenario, offline through the fixture folder.

use std::path::{Path, PathBuf};
use std::process::Command;

use rvow_gate::store::{self, Project};
use rvow_gate::syntax::grammar;
use rvow_gate::{change, config, drift, git, plan, proof, vow};

fn fixtures() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../rvow/tests/fixtures/grammars").display().to_string()
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

const APP_JS: &str = "function add(a, b) {\n  return a + b;\n}\n\ntest('adds two numbers', () => {\n  expect(add(1, 2)).toBe(3);\n});\n";

/// An accepted change in a fresh repository with one protected test file.
/// `config` is written into the project config before the vow, so a test
/// that names query files is not itself config drift.
fn setup(name: &str, file: &str, body: &str, config: &str) -> Setup {
    setup_with(name, file, body, config, &[])
}

/// Like `setup`, and `extra` files are written and committed before the
/// vow, so a project query file in it is protected at accept.
fn setup_with(name: &str, file: &str, body: &str, config: &str, extra: &[(&str, &str)]) -> Setup {
    let guard = crate::temp::TempDir::new(&format!("syntax-queries-{name}"));
    let tmp = guard.path().to_path_buf();
    let home = tmp.join("home");
    let repo = tmp.join("r");
    std::fs::create_dir_all(&repo).unwrap();
    git_cmd(&repo, &["init", "-q", "-b", "main"]);
    std::fs::create_dir_all(repo.join("tests")).unwrap();
    std::fs::write(repo.join(file), body).unwrap();
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    for (rel, text) in extra {
        std::fs::write(repo.join(rel), text).unwrap();
    }
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
    Setup { _tmp: guard, home, repo, project }
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

fn weaken_items(s: &Setup) -> Vec<String> {
    items(s).into_iter().filter(|(c, _)| *c == drift::Class::Weaken).map(|(_, w)| w).collect()
}

/// The weaken items after `file` changes from `old` to `new`, with the
/// grammar of `lang` primed and an unchanged file clean.
fn change_of(name: &str, file: &str, lang: &str, old: &str, new: &str) -> Vec<String> {
    let s = setup(name, file, old, "");
    prime(&s.home, lang);
    assert!(items(&s).is_empty());
    std::fs::write(s.repo.join(file), new).unwrap();
    weaken_items(&s)
}

fn drifts(name: &str, file: &str, lang: &str, old: &str, new: &str) {
    let got = change_of(name, file, lang, old, new);
    assert_eq!(got, [format!("protected file {file} changed")], "{name} is drift");
}

fn free(name: &str, file: &str, lang: &str, old: &str, new: &str) {
    let got = change_of(name, file, lang, old, new);
    assert!(got.is_empty(), "{name} is free: {got:?}");
}

const JAVA: &str = "class CalcTest {\n    @Test\n    void adds() {\n        assertEquals(3, add(1, 2));\n    }\n}\n";

fn java_with(extra: &str) -> String {
    JAVA.replace("    }\n}\n", &format!("    }}\n\n{extra}\n}}\n"))
}

#[test]
fn syntax_queries_more_junit_test_added_is_no_drift() {
    let new = java_with("    @Test\n    void subtracts() {\n        assertEquals(1, sub(2, 1));\n    }");
    free("more-junit-added", "tests/CalcTest.java", "java", JAVA, &new);
    let new = java_with(
        "    @ParameterizedTest\n    @ValueSource(ints = {1, 2})\n    void doubles(int n) {\n        assertEquals(n * 2, add(n, n));\n    }",
    );
    free("more-junit-param", "tests/CalcTest.java", "java", JAVA, &new);
}

#[test]
fn syntax_queries_more_junit_setup_added_is_drift() {
    for (i, hook) in ["@BeforeEach", "@BeforeAll", "@AfterEach", "@AfterAll", "@Before", "@After"].iter().enumerate() {
        let new = java_with(&format!("    {hook}\n    void init() {{\n    }}"));
        drifts(&format!("more-junit-hook-{i}"), "tests/CalcTest.java", "java", JAVA, &new);
    }
}

#[test]
fn syntax_queries_more_junit_extra_annotation_is_drift() {
    for (i, ann) in ["@Disabled", "@Timeout(1)", "@Tag(\"slow\")"].iter().enumerate() {
        let new = java_with(&format!("    @Test\n    {ann}\n    void subtracts() {{\n        assertEquals(1, sub(2, 1));\n    }}"));
        drifts(&format!("more-junit-extra-{i}"), "tests/CalcTest.java", "java", JAVA, &new);
    }
}

#[test]
fn syntax_queries_more_junit_test_changed_names_the_test() {
    let got = change_of("more-junit-changed", "tests/CalcTest.java", "java", JAVA, &JAVA.replace("assertEquals(3", "assertEquals(4"));
    assert_eq!(got, ["test tests/CalcTest.java changed adds".to_string()]);
}

#[test]
fn syntax_queries_more_junit_test_that_exits_is_drift() {
    for (i, call) in ["System.exit(1)", "Runtime.getRuntime().halt(1)", "Runtime.getRuntime().exit(1)"].iter().enumerate() {
        let new = java_with(&format!("    @Test\n    void subtracts() {{\n        {call};\n    }}"));
        drifts(&format!("more-junit-exit-{i}"), "tests/CalcTest.java", "java", JAVA, &new);
    }
}

const KOTLIN: &str = "class CalcTest {\n    @Test\n    fun adds() {\n        assertEquals(3, add(1, 2))\n    }\n}\n";

fn kotlin_with(extra: &str) -> String {
    KOTLIN.replace("    }\n}\n", &format!("    }}\n\n{extra}\n}}\n"))
}

#[test]
fn syntax_queries_more_kotlin_test_added_is_no_drift() {
    let new = kotlin_with("    @Test\n    fun subtracts() {\n        assertEquals(1, sub(2, 1))\n    }");
    free("more-kotlin-added", "tests/CalcTest.kt", "kotlin", KOTLIN, &new);
}

#[test]
fn syntax_queries_more_kotlin_setup_and_disabled_are_drift() {
    let new = kotlin_with("    @BeforeEach\n    fun init() {\n    }");
    drifts("more-kotlin-hook", "tests/CalcTest.kt", "kotlin", KOTLIN, &new);
    let new = kotlin_with("    @Test\n    @Disabled\n    fun subtracts() {\n    }");
    drifts("more-kotlin-disabled", "tests/CalcTest.kt", "kotlin", KOTLIN, &new);
}

#[test]
fn syntax_queries_more_kotlin_test_that_exits_is_drift() {
    for (i, call) in ["exitProcess(1)", "System.exit(1)"].iter().enumerate() {
        let new = kotlin_with(&format!("    @Test\n    fun subtracts() {{\n        {call}\n    }}"));
        drifts(&format!("more-kotlin-exit-{i}"), "tests/CalcTest.kt", "kotlin", KOTLIN, &new);
    }
}

const CSHARP: &str = "namespace Calc {\n    public class CalcTests {\n        [Fact]\n        public void Adds() {\n            Assert.Equal(3, Add(1, 2));\n        }\n    }\n}\n";

fn csharp_with(extra: &str) -> String {
    CSHARP.replace("        }\n    }\n}\n", &format!("        }}\n\n{extra}\n    }}\n}}\n"))
}

#[test]
fn syntax_queries_more_xunit_test_changed_names_the_test() {
    let got = change_of("more-xunit-changed", "tests/CalcTests.cs", "c_sharp", CSHARP, &CSHARP.replace("Equal(3", "Equal(4"));
    assert_eq!(got, ["test tests/CalcTests.cs changed Adds".to_string()]);
}

#[test]
fn syntax_queries_more_xunit_test_added_is_no_drift() {
    for (i, attr) in ["[Fact]", "[Test]", "[TestMethod]", "[TestCase(1, 2)]", "[Theory, InlineData(1)]"].iter().enumerate() {
        let new = csharp_with(&format!(
            "        {attr}\n        public void Subtracts() {{\n            Assert.Equal(1, Sub(2, 1));\n        }}"
        ));
        free(&format!("more-xunit-added-{i}"), "tests/CalcTests.cs", "c_sharp", CSHARP, &new);
    }
}

#[test]
fn syntax_queries_more_xunit_setup_and_skip_are_drift() {
    for (i, attr) in
        ["[SetUp]", "[TestInitialize]", "[ClassInitialize]", "[Fact(Skip = \"x\")]", "[Fact]\n        [Skip]"].iter().enumerate()
    {
        let new = csharp_with(&format!("        {attr}\n        public void Subtracts() {{\n        }}"));
        drifts(&format!("more-xunit-drift-{i}"), "tests/CalcTests.cs", "c_sharp", CSHARP, &new);
    }
}

#[test]
fn syntax_queries_more_xunit_test_that_exits_is_drift() {
    for (i, call) in ["Environment.Exit(1)", "Environment.FailFast(\"x\")"].iter().enumerate() {
        let new = csharp_with(&format!("        [Fact]\n        public void Subtracts() {{\n            {call};\n        }}"));
        drifts(&format!("more-xunit-exit-{i}"), "tests/CalcTests.cs", "c_sharp", CSHARP, &new);
    }
}

const RUBY: &str = "describe Calc do\n  it \"adds\" do\n    expect(add(1, 2)).to eq(3)\n  end\nend\n";

fn ruby_with(extra: &str) -> String {
    RUBY.replace("  end\nend\n", &format!("  end\n\n{extra}\nend\n"))
}

#[test]
fn syntax_queries_more_rspec_example_added_is_no_drift() {
    for (i, head) in ["it \"subtracts\" do", "specify \"subtracts\" do", "example \"subtracts\" do"].iter().enumerate() {
        let new = ruby_with(&format!("  {head}\n    expect(sub(2, 1)).to eq(1)\n  end"));
        free(&format!("more-rspec-added-{i}"), "tests/calc_spec.rb", "ruby", RUBY, &new);
    }
}

#[test]
fn syntax_queries_more_rspec_example_changed_names_the_test() {
    let got = change_of("more-rspec-changed", "tests/calc_spec.rb", "ruby", RUBY, &RUBY.replace("eq(3)", "eq(4)"));
    assert_eq!(got, ["test tests/calc_spec.rb changed adds".to_string()]);
}

#[test]
fn syntax_queries_more_rspec_hooks_added_are_drift() {
    for (i, hook) in [
        "before do\n    @x = 1\n  end",
        "after do\n    @x = 1\n  end",
        "let(:x) { 1 }",
        "subject { 1 }",
        "around do |ex|\n    ex.run\n  end",
    ]
    .iter()
    .enumerate()
    {
        let new = ruby_with(&format!("  {hook}"));
        drifts(&format!("more-rspec-hook-{i}"), "tests/calc_spec.rb", "ruby", RUBY, &new);
    }
}

#[test]
fn syntax_queries_more_rspec_example_that_exits_is_drift() {
    for (i, call) in ["exit", "exit 1", "abort \"x\"", "exit!", "Process.exit(1)"].iter().enumerate() {
        let new = ruby_with(&format!("  it \"subtracts\" do\n    {call}\n  end"));
        drifts(&format!("more-rspec-exit-{i}"), "tests/calc_spec.rb", "ruby", RUBY, &new);
    }
}

const PHP: &str =
    "<?php\nclass CalcTest extends TestCase {\n    public function testAdds() {\n        $this->assertEquals(3, add(1, 2));\n    }\n}\n";

fn php_with(extra: &str) -> String {
    PHP.replace("    }\n}\n", &format!("    }}\n\n{extra}\n}}\n"))
}

#[test]
fn syntax_queries_more_phpunit_test_added_is_no_drift() {
    let bodies = [
        "    public function testSubtracts() {\n        $this->assertEquals(1, sub(2, 1));\n    }",
        "    #[Test]\n    public function subtracts() {\n        $this->assertEquals(1, sub(2, 1));\n    }",
        "    /** @test */\n    public function subtracts() {\n        $this->assertEquals(1, sub(2, 1));\n    }",
    ];
    for (i, body) in bodies.iter().enumerate() {
        free(&format!("more-phpunit-added-{i}"), "tests/CalcTest.php", "php", PHP, &php_with(body));
    }
}

#[test]
fn syntax_queries_more_phpunit_test_changed_names_the_test() {
    let got = change_of("more-phpunit-changed", "tests/CalcTest.php", "php", PHP, &PHP.replace("assertEquals(3", "assertEquals(4"));
    assert_eq!(got, ["test tests/CalcTest.php changed testAdds".to_string()]);
}

#[test]
fn syntax_queries_more_phpunit_setup_and_helper_are_drift() {
    for (i, name) in ["setUp", "tearDown", "helper"].iter().enumerate() {
        let new = php_with(&format!("    public function {name}() {{\n    }}"));
        drifts(&format!("more-phpunit-hook-{i}"), "tests/CalcTest.php", "php", PHP, &new);
    }
    let new = php_with("    public static function setUpBeforeClass(): void {\n    }");
    drifts("more-phpunit-hook-static", "tests/CalcTest.php", "php", PHP, &new);
}

#[test]
fn syntax_queries_more_phpunit_test_that_exits_is_drift() {
    for (i, call) in ["exit(1);", "die(\"x\");", "exit;"].iter().enumerate() {
        let new = php_with(&format!("    public function testSubtracts() {{\n        {call}\n    }}"));
        drifts(&format!("more-phpunit-exit-{i}"), "tests/CalcTest.php", "php", PHP, &new);
    }
}

#[test]
fn syntax_queries_more_exit_spellings_are_drift() {
    for (i, call) in ["java.lang.System.exit(0);", "Runtime rt = Runtime.getRuntime(); rt.halt(1);"].iter().enumerate() {
        let new = java_with(&format!("    @Test\n    void subtracts() {{\n        {call}\n    }}"));
        drifts(&format!("more-exit-java-{i}"), "tests/CalcTest.java", "java", JAVA, &new);
    }
    for (i, call) in ["java.lang.System.exit(1)", "val r = Runtime.getRuntime(); r.halt(1)"].iter().enumerate() {
        let new = kotlin_with(&format!("    @Test\n    fun subtracts() {{\n        {call}\n    }}"));
        drifts(&format!("more-exit-kotlin-{i}"), "tests/CalcTest.kt", "kotlin", KOTLIN, &new);
    }
    let new = csharp_with("        [Fact]\n        public void Subtracts() {\n            global::System.Environment.Exit(1);\n        }");
    drifts("more-exit-csharp", "tests/CalcTests.cs", "c_sharp", CSHARP, &new);
    let ruby = [
        "exit if true",
        "exit unless false",
        "true && exit",
        "begin; exit; end",
        "exit rescue nil",
        "x = exit",
        "foo(exit)",
        "Kernel::exit(1)",
    ];
    for (i, call) in ruby.iter().enumerate() {
        let new = ruby_with(&format!("  it \"subtracts\" do\n    {call}\n  end"));
        drifts(&format!("more-exit-ruby-{i}"), "tests/calc_spec.rb", "ruby", RUBY, &new);
    }
    let new = php_with("    public function testSubtracts() {\n        die;\n    }");
    drifts("more-exit-php-die", "tests/CalcTest.php", "php", PHP, &new);
}

#[test]
fn syntax_queries_more_focus_and_skip_arguments_are_drift() {
    for (i, head) in ["it \"subtracts\", focus: true do", "it \"subtracts\", :focus do"].iter().enumerate() {
        let new = ruby_with(&format!("  {head}\n    expect(sub(2, 1)).to eq(1)\n  end"));
        drifts(&format!("more-focus-ruby-{i}"), "tests/calc_spec.rb", "ruby", RUBY, &new);
    }
    let new = format!("{APP_JS}\ntest('subtracts', {{ only: true }}, () => {{\n  expect(1).toBe(1);\n}});\n");
    drifts("more-focus-js", "tests/app.test.js", "javascript", APP_JS, &new);
    for (i, attr) in ["[Fact(SkipUnless = \"X\")]", "[TestCase(1, Ignore = \"x\")]"].iter().enumerate() {
        let new = csharp_with(&format!(
            "        {attr}\n        public void Subtracts() {{\n            Assert.Equal(1, Sub(2, 1));\n        }}"
        ));
        drifts(&format!("more-skip-csharp-{i}"), "tests/CalcTests.cs", "c_sharp", CSHARP, &new);
    }
}

#[test]
fn syntax_queries_more_abort_on_another_object_is_no_drift() {
    let new =
        format!("{APP_JS}\ntest('subtracts', () => {{\n  const c = new AbortController();\n  c.abort();\n  worker.terminate();\n}});\n");
    free("more-abort-js", "tests/app.test.js", "javascript", APP_JS, &new);
    let new = java_with("    @Test\n    void subtracts() {\n        session.exit();\n    }");
    free("more-exit-method-java", "tests/CalcTest.java", "java", JAVA, &new);
}

const DOCTEST_FILE: &str = "tests/calc_test.cpp";

const DOCTEST: &str = "TEST_CASE(\"adds\") { CHECK(add(1, 2) == 3); }\n";

const DOCTEST_SUITE: &str = "TEST_SUITE(\"calc\") {\n    TEST_CASE(\"adds\") {\n        CHECK(add(1, 2) == 3);\n    }\n}\n";

const CATCH2: &str = "TEST_CASE(\"calc\", \"[calc]\") {\n    SECTION(\"adds\") {\n        REQUIRE(add(1, 2) == 3);\n    }\n}\n";

/// A real file of dftracer-utils, `tests/test_hex64.cpp`.
const HEX64: &str = r#"#define DOCTEST_CONFIG_IMPLEMENT_WITH_MAIN
#include <dftracer/utils/core/common/hash/hex64.h>
#include <doctest/doctest.h>

#include <string>

using dftracer::utils::hash::format_hex64;
using dftracer::utils::hash::parse_hex64;

TEST_SUITE("hex64") {
    TEST_CASE("hex64 - round trips every value") {
        for (std::uint64_t v :
             {std::uint64_t{0}, std::uint64_t{1}, std::uint64_t{0xdef456},
              std::uint64_t{~0ull}, std::uint64_t{0xf07c4ebf132e3799ull}}) {
            auto text = format_hex64(v);
            CHECK(text.size() == 16);
            auto back = parse_hex64(text);
            REQUIRE(back.has_value());
            CHECK(*back == v);
        }
    }

    TEST_CASE("hex64 - matches what dftracer writes") {
        CHECK(*parse_hex64("f07c4ebf132e3799") == 0xf07c4ebf132e3799ull);
        CHECK(format_hex64(0xf07c4ebf132e3799ull) == "f07c4ebf132e3799");
        CHECK(format_hex64(0) == "0000000000000000");
    }
}
"#;

fn doctest_free(name: &str, old: &str, new: &str) {
    free(name, DOCTEST_FILE, "cpp", old, new);
}

fn doctest_drifts(name: &str, old: &str, new: &str) {
    drifts(name, DOCTEST_FILE, "cpp", old, new);
}

fn doctest_names(name: &str, old: &str, new: &str) -> Vec<String> {
    change_of(name, DOCTEST_FILE, "cpp", old, new)
}

#[test]
fn syntax_queries_doctest_case_added_is_no_drift() {
    let new = format!("{DOCTEST}\nTEST_CASE(\"subtracts\") {{ CHECK(sub(2, 1) == 1); }}\n");
    doctest_free("doctest-added", DOCTEST, &new);
}

#[test]
fn syntax_queries_doctest_case_changed_inside_a_suite_names_the_case() {
    let got = doctest_names("doctest-suite", DOCTEST_SUITE, &DOCTEST_SUITE.replace("== 3", "== 4"));
    assert_eq!(got, [format!("test {DOCTEST_FILE} changed adds")]);
}

#[test]
fn syntax_queries_doctest_case_added_inside_a_suite_and_a_new_suite_are_no_drift() {
    let inner = DOCTEST_SUITE.replace("    }\n}\n", "    }\n    TEST_CASE(\"subtracts\") {\n        CHECK(sub(2, 1) == 1);\n    }\n}\n");
    doctest_free("doctest-suite-added", DOCTEST_SUITE, &inner);
    let outer = format!("{DOCTEST_SUITE}\nTEST_SUITE(\"calc2\") {{\n    TEST_CASE(\"muls\") {{ CHECK(mul(2, 3) == 6); }}\n}}\n");
    doctest_free("doctest-suite-new", DOCTEST_SUITE, &outer);
}

#[test]
fn syntax_queries_doctest_removed_case_names_the_case() {
    let two = format!("{DOCTEST}TEST_CASE(\"subtracts\") {{ CHECK(sub(2, 1) == 1); }}\n");
    assert_eq!(doctest_names("doctest-removed", &two, DOCTEST), [format!("test {DOCTEST_FILE} changed subtracts")]);
}

#[test]
fn syntax_queries_doctest_fixture_case_added_is_no_drift() {
    let old = "TEST_CASE_FIXTURE(Calc, \"adds\") { CHECK(add(1, 2) == 3); }\n";
    let new = format!("{old}TEST_CASE_FIXTURE(Calc, \"subtracts\") {{ CHECK(sub(2, 1) == 1); }}\n");
    doctest_free("doctest-fixture", old, &new);
    let got = doctest_names("doctest-fixture-changed", old, &old.replace("== 3", "== 4"));
    assert_eq!(got, [format!("test {DOCTEST_FILE} changed adds")]);
}

#[test]
fn syntax_queries_doctest_template_and_decorated_cases_are_named() {
    let old = "TEST_CASE_TEMPLATE(\"sizes\", T, int, long) { CHECK(sizeof(T) > 0); }\nTEST_CASE(\"slow\" * doctest::timeout(5)) { CHECK(1 == 1); }\n";
    let new = format!("{old}TEST_CASE_TEMPLATE(\"widths\", T, char, short) {{ CHECK(sizeof(T) > 0); }}\n");
    doctest_free("doctest-template", old, &new);
    let got = doctest_names("doctest-decorated", old, &old.replace("== 1", "== 2"));
    assert_eq!(got, [format!("test {DOCTEST_FILE} changed slow")]);
}

#[test]
fn syntax_queries_doctest_catch2_section_change_names_the_case() {
    let added = CATCH2.replace("    }\n}\n", "    }\n    SECTION(\"subtracts\") {\n        REQUIRE(sub(2, 1) == 1);\n    }\n}\n");
    assert_eq!(doctest_names("doctest-section-added", CATCH2, &added), [format!("test {DOCTEST_FILE} changed calc")]);
    let got = doctest_names("doctest-section", CATCH2, &CATCH2.replace("        REQUIRE(add(1, 2) == 3);\n", ""));
    assert_eq!(got, [format!("test {DOCTEST_FILE} changed calc")]);
}

#[test]
fn syntax_queries_doctest_catch2_scenario_is_a_test() {
    let old = "SCENARIO(\"adding\") {\n    GIVEN(\"two numbers\") {\n        REQUIRE(add(1, 2) == 3);\n    }\n}\n";
    let new = format!("{old}SCENARIO(\"subtracting\") {{\n    GIVEN(\"two numbers\") {{\n        REQUIRE(sub(2, 1) == 1);\n    }}\n}}\n");
    doctest_free("doctest-scenario", old, &new);
    let got = doctest_names("doctest-scenario-changed", old, &old.replace("== 3", "== 4"));
    assert_eq!(got, [format!("test {DOCTEST_FILE} changed adding")]);
}

#[test]
fn syntax_queries_doctest_new_case_that_exits_is_drift() {
    let fine = format!("{DOCTEST}TEST_CASE(\"x\") {{ CHECK(1 == 1); }}\n");
    doctest_free("doctest-exit-control", DOCTEST, &fine);
    for (i, call) in ["std::exit(0);", "exit(1);", "abort();", "_exit(0);"].iter().enumerate() {
        let new = format!("{DOCTEST}TEST_CASE(\"x\") {{ {call} }}\n");
        doctest_drifts(&format!("doctest-exit-{i}"), DOCTEST, &new);
    }
    let inner = DOCTEST_SUITE.replace("    }\n}\n", "    }\n    TEST_CASE(\"x\") {\n        std::exit(0);\n    }\n}\n");
    doctest_drifts("doctest-exit-in-suite", DOCTEST_SUITE, &inner);
}

#[test]
fn syntax_queries_doctest_new_case_that_skips_is_drift() {
    for (i, head) in [
        "TEST_CASE(\"x\" * doctest::skip())",
        "TEST_CASE(\"x\" * doctest::may_fail())",
        "TEST_CASE(\"x\" * doctest::timeout(1) * doctest::skip(true))",
        "TEST_CASE(\"x\", \"[.]\")",
        "TEST_CASE(\"x\", \"[slow][!mayfail]\")",
    ]
    .iter()
    .enumerate()
    {
        let new = format!("{DOCTEST}{head} {{ CHECK(1 == 1); }}\n");
        doctest_drifts(&format!("doctest-skip-{i}"), DOCTEST, &new);
    }
    let new = format!("{DOCTEST}TEST_CASE(\"x\" * doctest::timeout(1)) {{ CHECK(1 == 1); }}\n");
    doctest_free("doctest-timeout-is-free", DOCTEST, &new);
}

#[test]
fn syntax_queries_doctest_gtest_case_with_a_tag_string_stays_free() {
    let old = "TEST(Calc, Adds) {\n    EXPECT_EQ(1, 1);\n}\n";
    let new = format!("{old}\nTEST(Calc, Tags) {{\n    EXPECT_EQ(tag(), \"[.]\");\n}}\n");
    free("doctest-gtest-tag", "tests/g.cpp", "cpp", old, &new);
}

#[test]
fn syntax_queries_doctest_real_dftracer_file_adds_and_names_cases() {
    let added = HEX64.replace(
        "        CHECK(format_hex64(0) == \"0000000000000000\");\n    }\n",
        "        CHECK(format_hex64(0) == \"0000000000000000\");\n    }\n\n    TEST_CASE(\"hex64 - zero parses\") {\n        CHECK(*parse_hex64(\"0000000000000000\") == 0);\n    }\n",
    );
    assert_ne!(added, HEX64);
    free("doctest-real-added", "tests/test_hex64.cpp", "cpp", HEX64, &added);
    let got = change_of(
        "doctest-real-changed",
        "tests/test_hex64.cpp",
        "cpp",
        HEX64,
        &HEX64.replace("CHECK(text.size() == 16);", "CHECK(text.size() == 15);"),
    );
    assert_eq!(got, ["test tests/test_hex64.cpp changed hex64 - round trips every value".to_string()]);
}

/// A doctest file whose helper has a default argument of `= {}`, a default
/// tree-sitter-cpp cannot parse. The units around the error region must still
/// be compared.
const CPP_ERROR: &str = r#"#include <doctest/doctest.h>

struct Opts {
    int n = 0;
};

void run(Opts o = {}) {
    add(o.n);
}

TEST_CASE("adds") {
    CHECK(add(1, 2) == 3);
}
"#;

const CPP_ERROR_FILE: &str = "tests/calc_test.cpp";

/// A real file of dftracer-utils, `tests/dataframe/test_cast_followups.cpp`,
/// whose template helper takes a default argument of `= {}`.
const DFTRACER_CAST: &str = r#"#define DOCTEST_CONFIG_IMPLEMENT_WITH_MAIN
#include <dftracer/utils/dataframe/dataframe.h>
#include <doctest/doctest.h>

#include <cmath>
#include <cstdint>
#include <limits>
#include <random>
#include <vector>

using dftracer::utils::dataframe::Series;
using dftracer::utils::dataframe::TypeId;

namespace {

bool bit(const Series& s, std::int64_t i) {
    return (s.data<std::uint8_t>()[i >> 3] >> (i & 7)) & 1;
}

template <class T>
void check_to_bool(const std::vector<T>& x, TypeId type,
                   const std::vector<std::uint8_t>& valid = {}) {
    const auto n = static_cast<std::int64_t>(x.size());
    Series s =
        Series::flat(type, x.data(), n, valid.empty() ? nullptr : valid.data());
    Series r = s.cast(TypeId::Bool);
    REQUIRE(r.valid());
    REQUIRE(r.length() == n);
    for (std::int64_t i = 0; i < n; ++i) {
        const T v = x[static_cast<std::size_t>(i)];
        bool is_null = !valid.empty() && !((valid[i >> 3] >> (i & 7)) & 1);
        if constexpr (std::is_floating_point_v<T>)
            is_null = is_null || std::isnan(v);
        if (is_null) {
            CHECK(r.is_null(i));
        } else {
            CHECK_FALSE(r.is_null(i));
            CHECK(bit(r, i) == (v != T{}));
        }
    }
}

}  // namespace

TEST_CASE("SIMD and scalar number-to-bool agree on every width and length") {
    std::mt19937_64 gen(12345);
    for (std::size_t n : {0, 1, 7, 8, 63, 64, 65, 127, 128, 129, 1000, 4097}) {
        std::vector<std::int32_t> a(n);
        check_to_bool(a, TypeId::Int32);
    }
}

TEST_CASE("a column without NaN or nulls has no validity after the cast") {
    std::vector<std::int64_t> x{1, 0, 3};
    Series r = Series::flat_i64(x.data(), 3).cast(TypeId::Bool);
    REQUIRE(r.valid());
    CHECK(r.null_count() == 0);
}
"#;

fn cpp_error_drifts(name: &str, old: &str, new: &str) {
    drifts(name, CPP_ERROR_FILE, "cpp", old, new);
}

#[test]
fn syntax_queries_cpp_errors_a_case_added_next_to_a_default_argument_is_no_drift() {
    let new = format!("{CPP_ERROR}\nTEST_CASE(\"subtracts\") {{ CHECK(sub(2, 1) == 1); }}\n");
    free("cpp-errors-added", CPP_ERROR_FILE, "cpp", CPP_ERROR, &new);
}

#[test]
fn syntax_queries_cpp_errors_a_change_inside_the_error_region_is_drift() {
    cpp_error_drifts("cpp-errors-default-changed", CPP_ERROR, &CPP_ERROR.replace("Opts o = {}", "Opts o = {1}"));
}

#[test]
fn syntax_queries_cpp_errors_an_assert_removed_next_to_the_error_names_the_case() {
    let old = CPP_ERROR.replace("    CHECK(add(1, 2) == 3);\n", "");
    let got = change_of("cpp-errors-assert", CPP_ERROR_FILE, "cpp", CPP_ERROR, &old);
    assert_eq!(got, [format!("test {CPP_ERROR_FILE} changed adds")]);
}

#[test]
fn syntax_queries_cpp_errors_a_new_case_that_holds_a_parse_error_is_drift() {
    let new = format!("{CPP_ERROR}\nTEST_CASE(\"broken\") {{ CHECK(add(1, 2) == 3 }}\n");
    cpp_error_drifts("cpp-errors-new-broken", CPP_ERROR, &new);
}

#[test]
fn syntax_queries_cpp_errors_a_real_dftracer_file_compares_by_unit() {
    const FILE: &str = "tests/test_cast_followups.cpp";
    let new = format!(
        "{DFTRACER_CAST}\nTEST_CASE(\"a null casts to null\") {{\n    CHECK(Series::flat_i64(nullptr, 0).cast(TypeId::Bool).null_count() == 0);\n}}\n"
    );
    free("cpp-errors-dftracer-added", FILE, "cpp", DFTRACER_CAST, &new);
    let got = change_of(
        "cpp-errors-dftracer-changed",
        FILE,
        "cpp",
        DFTRACER_CAST,
        &DFTRACER_CAST.replace("null_count() == 0", "null_count() == 1"),
    );
    assert_eq!(got, [format!("test {FILE} changed a column without NaN or nulls has no validity after the cast")]);
}

const UNITY_FILE: &str = "tests/calc_test.c";

const UNITY: &str = "#include <unity.h>\n\nstatic void test_add(void) {\n    TEST_ASSERT_EQUAL_INT(3, add(1, 2));\n}\n\nint main(void) {\n    UNITY_BEGIN();\n    RUN_TEST(test_add);\n    return UNITY_END();\n}\n";

fn unity_with(function: &str, run: &str) -> String {
    UNITY
        .replace("int main(void)", &format!("{function}\nint main(void)"))
        .replace("    return UNITY_END();", &format!("{run}    return UNITY_END();"))
}

#[test]
fn syntax_queries_unity_test_added_with_run_test_is_no_drift() {
    let new = unity_with("static void test_sub(void) {\n    TEST_ASSERT_EQUAL_INT(1, sub(2, 1));\n}\n", "    RUN_TEST(test_sub);\n");
    free("unity-added", UNITY_FILE, "c", UNITY, &new);
    let fn_only = unity_with("static void test_sub(void) {\n    TEST_ASSERT_EQUAL_INT(1, sub(2, 1));\n}\n", "");
    free("unity-fn-only", UNITY_FILE, "c", UNITY, &fn_only);
}

#[test]
fn syntax_queries_unity_run_test_line_removed_names_the_test() {
    let got = change_of("unity-removed", UNITY_FILE, "c", UNITY, &UNITY.replace("    RUN_TEST(test_add);\n", ""));
    assert_eq!(got, [format!("test {UNITY_FILE} changed test_add")]);
}

#[test]
fn syntax_queries_unity_test_changed_names_the_test() {
    let got = change_of("unity-changed", UNITY_FILE, "c", UNITY, &UNITY.replace("(3, add(1, 2))", "(4, add(1, 2))"));
    assert_eq!(got, [format!("test {UNITY_FILE} changed test_add")]);
}

#[test]
fn syntax_queries_unity_setup_added_is_drift() {
    let new = UNITY.replace("static void test_add", "void setUp(void) {\n    reset();\n}\n\nstatic void test_add");
    drifts("unity-setup", UNITY_FILE, "c", UNITY, &new);
    let new = unity_with("void tearDown(void) {\n    reset();\n}\n", "");
    drifts("unity-teardown", UNITY_FILE, "c", UNITY, &new);
}

#[test]
fn syntax_queries_unity_new_test_that_exits_is_drift() {
    for (i, call) in ["exit(0);", "abort();", "_exit(1);"].iter().enumerate() {
        let new = unity_with(&format!("static void test_sub(void) {{\n    {call}\n}}\n"), "    RUN_TEST(test_sub);\n");
        drifts(&format!("unity-exit-{i}"), UNITY_FILE, "c", UNITY, &new);
    }
}

#[test]
fn syntax_queries_unity_run_test_of_a_name_in_use_is_drift() {
    let new = unity_with("", "    RUN_TEST(test_add);\n");
    drifts("unity-run-twice", UNITY_FILE, "c", UNITY, &new);
    let new = unity_with("", "    RUN_TEST(setUp);\n");
    drifts("unity-run-helper", UNITY_FILE, "c", UNITY, &new);
}

#[test]
fn syntax_queries_unity_a_file_without_unity_is_not_a_test_file() {
    let old = "#include \"plugin.h\"\n\nint main(void) {\n    return run();\n}\n";
    let new = format!("static void test_helper(void) {{\n    run();\n}}\n\n{old}");
    drifts("unity-none", "tests/plugin_consumer.c", "c", old, &new);
}

#[test]
fn syntax_queries_unity_header_files_keep_the_raw_rule() {
    let old = "#ifndef CALC_H\n#define CALC_H\nint add(int a, int b);\n#endif\n";
    drifts("unity-header-decl", "tests/calc.h", "c", old, &old.replace("#endif\n", "int sub(int a, int b);\n#endif\n"));
    let helper = format!("{old}static inline void test_helper(void) {{ run(); }}\n");
    drifts("unity-header-helper", "tests/calc.h", "c", old, &helper);
    drifts("unity-header-define", "tests/calc.h", "c", old, &old.replace("CALC_H", "CALC2_H"));
}

/// A real file of dftracer-utils, `tests/index/test_indexer_abi.c`, cut to
/// its first test.
const INDEXER_ABI: &str = r#"#include <stdio.h>
#include <stdlib.h>
#include <unity.h>

#include "testing_utilities.h"

static test_environment_handle_t g_env = NULL;

void setUp(void) {
    g_env = test_environment_create();
    TEST_ASSERT_NOT_NULL(g_env);
}

void tearDown(void) {
    test_environment_destroy(g_env);
    g_env = NULL;
}

static void test_open_with_default_options(void) {
    char* trace = test_environment_create_dft_gzip_file(g_env, 10);
    dftu_indexer* ix = open_one(trace);
    dftu_indexer_free(ix);
    free(trace);
}

static void test_open_invalid_arguments(void) {
    const char* one[] = {"x.pfw.gz"};
    dftu_indexer_open_result b = dftu_indexer_open(one, 0, NULL);
    TEST_ASSERT_FALSE(DFTU_RESULT_OK(b));
    TEST_ASSERT_EQUAL_INT(DFTU_COND_INVALID_ARGUMENT,
                          DFTU_RESULT_ERROR(b).condition);
}

int main(void) {
    UNITY_BEGIN();
    RUN_TEST(test_open_with_default_options);
    RUN_TEST(test_open_invalid_arguments);
    return UNITY_END();
}
"#;

#[test]
fn syntax_queries_unity_real_dftracer_file_adds_and_names_tests() {
    let added = INDEXER_ABI
        .replace(
            "int main(void) {",
            "static void test_free_null(void) {\n    dftu_indexer_free(NULL);\n    TEST_PASS();\n}\n\nint main(void) {",
        )
        .replace("    return UNITY_END();", "    RUN_TEST(test_free_null);\n    return UNITY_END();");
    free("unity-real-added", "tests/test_indexer_abi.c", "c", INDEXER_ABI, &added);
    let got = change_of(
        "unity-real-changed",
        "tests/test_indexer_abi.c",
        "c",
        INDEXER_ABI,
        &INDEXER_ABI.replace("dftu_indexer_open(one, 0, NULL)", "dftu_indexer_open(one, 1, NULL)"),
    );
    assert_eq!(got, ["test tests/test_indexer_abi.c changed test_open_invalid_arguments".to_string()]);
    let setup_changed = change_of(
        "unity-real-setup",
        "tests/test_indexer_abi.c",
        "c",
        INDEXER_ABI,
        &INDEXER_ABI.replace("g_env = NULL;\n}", "g_env = NULL;\n    cleanup();\n}"),
    );
    assert_eq!(setup_changed, ["protected file tests/test_indexer_abi.c changed".to_string()]);
}

const CHECK_JS: &str = "check('adds', () => {\n  expect(add(1, 2)).toBe(3);\n});\n";

const CHECK_QUERY_CONFIG: &str = "test_queries = { javascript = \"checks.scm\" }\n";

fn check_with(extra: &str) -> String {
    format!("{CHECK_JS}\n{extra}")
}

#[test]
fn syntax_queries_project_trusted_query_frees_a_new_check() {
    let s = setup_with("trusted-free", "tests/app.check.js", CHECK_JS, CHECK_QUERY_CONFIG, &[("checks.scm", CHECKS_SCM)]);
    prime(&s.home, "javascript");
    assert!(items(&s).is_empty());

    let new = check_with("check('subtracts', () => {\n  expect(sub(2, 1)).toBe(1);\n});\n");
    std::fs::write(s.repo.join("tests/app.check.js"), &new).unwrap();
    assert!(items(&s).is_empty(), "a new check of a trusted query is free: {:?}", items(&s));

    let exits = check_with("check('leaves', () => {\n  process.exit(0);\n});\n");
    std::fs::write(s.repo.join("tests/app.check.js"), exits).unwrap();
    assert_eq!(weaken_items(&s), ["protected file tests/app.check.js changed".to_string()], "the free rules hold for a project mark");

    let reuse = check_with("check('adds', () => {\n  expect(1).toBe(1);\n});\n");
    std::fs::write(s.repo.join("tests/app.check.js"), reuse).unwrap();
    assert_eq!(weaken_items(&s), ["protected file tests/app.check.js changed".to_string()], "a used name is not fresh");
}

#[test]
fn syntax_queries_project_trusted_query_file_changed_is_drift() {
    let s = setup_with("trusted-edit", "tests/app.check.js", CHECK_JS, CHECK_QUERY_CONFIG, &[("checks.scm", CHECKS_SCM)]);
    prime(&s.home, "javascript");
    assert!(items(&s).is_empty());

    std::fs::write(s.repo.join("checks.scm"), format!("{CHECKS_SCM}; a comment\n")).unwrap();
    assert_eq!(weaken_items(&s), ["protected file checks.scm changed".to_string()]);

    let new = check_with("check('subtracts', () => {\n  expect(sub(2, 1)).toBe(1);\n});\n");
    std::fs::write(s.repo.join("tests/app.check.js"), new).unwrap();
    let got = weaken_items(&s);
    assert!(got.contains(&"protected file tests/app.check.js changed".to_string()), "an edited query frees nothing: {got:?}");
}

#[test]
fn syntax_queries_project_trusted_query_added_after_accept_frees_nothing() {
    let s = setup("trusted-late", "tests/app.check.js", CHECK_JS, "");
    prime(&s.home, "javascript");
    assert!(items(&s).is_empty());

    std::fs::write(s.repo.join("checks.scm"), CHECKS_SCM).unwrap();
    std::fs::write(s.project.root.join("config.toml"), CHECK_QUERY_CONFIG).unwrap();
    let new = check_with("check('subtracts', () => {\n  expect(sub(2, 1)).toBe(1);\n});\n");
    std::fs::write(s.repo.join("tests/app.check.js"), new).unwrap();
    let got = weaken_items(&s);
    assert!(got.contains(&"protected file tests/app.check.js changed".to_string()), "the new unit is not free: {got:?}");
    assert!(got.iter().any(|w| w.starts_with("config changed")), "the config change is drift: {got:?}");
}

#[test]
fn syntax_queries_project_trusted_query_named_at_accept_but_missing_frees_nothing() {
    let s = setup("trusted-missing", "tests/app.check.js", CHECK_JS, CHECK_QUERY_CONFIG);
    prime(&s.home, "javascript");
    std::fs::write(s.repo.join("checks.scm"), CHECKS_SCM).unwrap();
    let new = check_with("check('subtracts', () => {\n  expect(sub(2, 1)).toBe(1);\n});\n");
    std::fs::write(s.repo.join("tests/app.check.js"), new).unwrap();
    let got = weaken_items(&s);
    assert!(
        got.contains(&"protected file tests/app.check.js changed".to_string()),
        "a query file that was not there at accept is not trusted: {got:?}"
    );
}

#[test]
fn syntax_queries_doctest_new_case_with_a_directive_is_drift() {
    let cases = [
        "TEST_CASE(\"b\") {\n#define REQUIRE(x)\n}\n",
        "TEST_CASE(\"b\") {\n#undef CHECK\n}\n",
        "TEST_CASE(\"b\") {\n#include \"evil.h\"\n}\n",
    ];
    for (i, case) in cases.iter().enumerate() {
        doctest_drifts(&format!("doctest-directive-{i}"), DOCTEST, &format!("{case}{DOCTEST}"));
    }
    let new = UNITY.replace(
        "static void test_add",
        "static void test_sub(void) {\n#undef TEST_ASSERT_EQUAL_INT\n#define TEST_ASSERT_EQUAL_INT(a, b) ((void)0)\n}\n\nstatic void test_add",
    );
    drifts("unity-directive", UNITY_FILE, "c", UNITY, &new);
}

#[test]
fn syntax_queries_doctest_other_exit_spellings_are_drift() {
    for (i, call) in ["::exit(0);", "::abort();", "raise(9);", "auto f = &exit; f(0);"].iter().enumerate() {
        doctest_drifts(&format!("doctest-exit-more-{i}"), DOCTEST, &format!("{DOCTEST}TEST_CASE(\"x\") {{ {call} }}\n"));
    }
    let new = unity_with("static void test_sub(void) {\n    kill(getpid(), 9);\n}\n", "    RUN_TEST(test_sub);\n");
    drifts("unity-kill", UNITY_FILE, "c", UNITY, &new);
}

#[test]
fn syntax_queries_doctest_other_skips_are_drift() {
    for (i, head) in
        ["TEST_CASE(\"n\" * skip())", "TEST_CASE(\"n\", \"[hide]\")", "TEST_CASE(\"n\" * doctest::expected_failures(9))"].iter().enumerate()
    {
        doctest_drifts(&format!("doctest-skip-more-{i}"), DOCTEST, &format!("{DOCTEST}{head} {{ CHECK(1 == 2); }}\n"));
    }
    let new = unity_with("static void test_sub(void) {\n    TEST_IGNORE();\n}\n", "    RUN_TEST(test_sub);\n");
    drifts("unity-ignore", UNITY_FILE, "c", UNITY, &new);
}

#[test]
fn syntax_queries_unity_main_keeps_its_order() {
    let moved = UNITY.replace("    RUN_TEST(test_add);\n    return UNITY_END();", "    return UNITY_END();\n    RUN_TEST(test_add);");
    drifts("unity-return-first", UNITY_FILE, "c", UNITY, &moved);
    let added = unity_with("static void test_sub(void) {\n    TEST_ASSERT_EQUAL_INT(1, sub(2, 1));\n}\n", "    RUN_TEST(test_sub);\n");
    free("unity-order-added", UNITY_FILE, "c", UNITY, &added);
}

#[test]
fn syntax_queries_cpp_errors_a_directive_in_a_template_header_is_drift() {
    let new = format!("{DOCTEST}TEST_CASE_TEMPLATE(\"x\", T, int, float\n#define CHECK(x) (void)0\n) {{ CHECK(1); }}\n");
    doctest_drifts("cpp-errors-header-define", DOCTEST, &new);
}
