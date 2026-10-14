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
