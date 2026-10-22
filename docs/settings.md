# rvow settings

rvow reads settings from a global file and a project file.
The global file lives inside the rvow home (the directory named in `RVOW_HOME`).
The project file is `config.toml` in the rvow tree root of the project.
That root is the project folder of the home store in home mode, and the `rvow/` folder of the repository in repository mode.

The project value wins when both set a key, and a project list replaces the global list instead of extending it.

In repository mode the shared settings come from the project file only, so one contributor's settings cannot change a check that the project owns.
The global-only keys `machine`, `signing_key` and `agent` still come from the global file in both modes, and the project file refuses them.
A clean checkout never contains the user's `machine`, `signing_key` or `agent`.
Send those through the environment or a separate global file instead.

Most keys are part of the vow, so changing them after the plan is accepted is drift until the user confirms it.
`machine`, `signing_key` and `agent` are exceptions.
They belong in the global file only, and their values never enter the proof key.

### What `rvow init` ignores

`rvow init` adds the files of one machine in that root to `.gitignore`.
These are `.lock`, `hook-state/`, `provenance/`, `hook-stats.jsonl`, `leases.jsonl`, `yolo.json` and `rerun-cache.jsonl`.
In home mode it also ignores these in the rvow home:

- `claude-plugin/`
- the seal key
- `project-index.json`
- `worktrees/`

A store made before this change needs these lines added by hand.

`project-index.json` is a local cache from a repository to its project folder, so rvow walks the rvow home only for a repository it has no valid entry for.
rvow trusts an entry only for a project folder inside the rvow home whose root commit the repository still holds, and the hooks deny agent writes to the file.
Delete it at any time.

## An example

The full example below is a project file in home mode.
It parses as valid rvow settings.
It sets three interview rounds, unsigned confirmations by default and a ten minute test timeout.

```toml
base = "main"
timeout = "10m"
interview_cap = 3
checks = ["cargo test --locked"]
```

| Key | Meaning | Global only |
|---|---|---|
| `base` | The branch that becomes the `branch:` of a new plan | no |
| `timeout` | The default time one verify command may run | no |
| `inputs` | Extra globs whose content is part of a proof's input identity | no |
| `test_globs` | Globs that mark a file as a test, so editing it counts as weakening a proof | no |
| `protect_infra` | Globs of test infrastructure that the guard protects, such as snapshots, runner configs and `package.json` | no |
| `checks` | Commands the gate runs after task verification; an entry is a string or `{ run, files }` | no |
| `setup` | Commands the gate rerun runs before verification; an entry is a string or `{ run, files }` | no |
| `rerun_env` | Environment pairs applied when the gate reruns a task | no |
| `rerun_links` | Git-ignored directories of the repository, by repo-relative name, that the rerun worktree links in (default none) | no |
| `rerun_proven` | Rerun a task in a clean checkout even when its proof is current and its files are committed (default false) | no |
| `tools` | Executables whose content is part of the gate cache key, by name on `PATH` or by path | no |
| `rerun_cache` | How long a cached rerun pass stays valid, such as `24h`, or `off`; default `168h` | no |
| `cache_with_links` | Keep the rerun cache on when `rerun_links` is set; default false | no |
| `env` / `env_vars` | Extra commands and variables captured in the proof environment | no |
| `interview_cap` | The most questions one clarify round asks, 1 to 3, default 3 | no |
| `require_signature` | When true, a confirm only decides when its ssh signature is valid | no |
| `allowed_signers` | The allowed-signers file for signed confirmations | no |
| `context` | Project context text the `rvow guide` command appends | no |
| `rules` | Per-artifact authoring rules loaded by `rvow guide` | no |
| `machine` | A name for this computer | yes |
| `signing_key` | The ssh key for `rvow confirm --sign` | yes |
| `agent` | A name for the agent that runs proofs | yes |

The project `context` and each artifact `rules.<artifact>` value are capped at 2000 UTF-8 bytes.
Values above the cap are refused at load.

## Default test globs

Without `test_globs`, a file counts as a test when it lives in a `tests/`, `__tests__/`, `spec/` or `src/test/` folder at any depth.
It also counts when its name matches `*_test.*`, `test_*.py`, `*.test.*`, `*.spec.*`, `*_spec.rb` or `*Test.java`.

Without `protect_infra`, the guard also protects snapshots, `conftest.py`, `pytest.ini`, `tox.ini`, `jest.config.*`, `vitest.config.*`, `.cargo/config.toml`, `Makefile`, `justfile` and `package.json`.
Setting either key replaces its defaults.

## Linking build output into the rerun

`rvow gate --rerun` runs every verify command in a clean worktree of HEAD, so a verify command that needs generated output such as `node_modules` fails there.
List such a directory in `rerun_links` to link it from the real repository into the worktree.
The default is an empty list, which keeps the worktree clean.

A name in `rerun_links` follows these rules:

- It must be a relative path inside the repository that git does not track.
- A tracked name, an absolute name or a name with `..` is refused, and the error names the key.
- A listed directory that does not exist is skipped.

> [!WARNING]
> A verify command can write through a link into the real directory, so list only directories that a verify command may change.

`rerun_links` is not part of the vow, so changing it is not drift.
The same holds for `rerun_proven`.

### Rerun of proven tasks

By default a push trusts a task whose proof is current and whose files are all committed, because it was proved on that very content.
A task of an archived change is never rerun, since archiving needs every task proved.
Set `rerun_proven = true` when you want a clean-checkout run of proven tasks too.

## Warm reruns

`rvow gate --rerun` keeps its promise of a clean checkout of HEAD.
It builds in a new temporary worktree each time.
For a Rust project, point that build at a directory that only reruns use, or link `target` in, so the reruns start warm.

```toml
rerun_env = { CARGO_TARGET_DIR = "/Users/me/.cache/rvow-target" }
rerun_links = ["target"]
```

Use a directory of your own for `CARGO_TARGET_DIR`.
`rvow accept` runs no verify command, and `rvow prove` runs in your own checkout, so these keys do not touch either.
`rerun_links` turns the rerun cache off, as the section on linked build output says.

> [!WARNING]
> Do not point two checkouts that both stay on disk, such as two git worktrees of one repository, at one cargo build folder.
> Each overwrites the other's binaries.

## Scoping setup and checks to files

A `setup` or `checks` entry can be a table with `run` and `files`, such as `{ run = "make -C build", files = ["src/**"] }`.
A plain string entry always runs.
A table entry runs in these cases:

- A task that runs shares a file with its globs.
- A file that the branch changed matches one of them.
- The base branch is unknown, so nothing is skipped by mistake.

With a C++ entry scoped to `src/**` and a Python entry scoped to `python/**`, a branch that changes only `python/a.py` runs the Python entry and skips the C++ one.

The gate makes its rerun worktree and runs `setup` only when something needs them.
When the cache or a current proof answers every task, it makes no worktree and runs no `setup`.
The `checks` that remain then run in the repository itself.

## The test count

A pass with 0 tests run is not a proof, so rvow reads the number of tests from the summary line that the runner prints.

| Runner | Summary line rvow reads |
|--------|-------------------------|
| `cargo test` | each `test result:` line, summed |
| `pytest` | the final `=== N passed ... ===` line |
| `cargo nextest` | each `Summary [` line, summed |
| `jest` | each `Tests:` line, the `total` count |
| `go test` | each package line, with `[no tests to run]` and `[no test files]` as 0 |
| `node --test` | the `tests N` line of the spec reporter (after the info glyph) or of the TAP reporter (`# tests N`); the last one wins. Node reports a file in which no test ran, such as when `--test-name-pattern` matches nothing in it, as one passing test named by the file; rvow does not count that result |
| `vitest` | the `Tests` line, the total in the parentheses: `Tests  1 failed | 3 passed (4)` is 4 |
| `mocha` | the `N passing` line plus the `N failing` line; `pending` tests do not count |
| `ctest` | the `N% tests passed, F tests failed out of M` line, the `M`; `No tests were found!!!` is 0 |
| doctest | each `[doctest] test cases: N | ...` line, summed; the `[doctest] assertions:` line does not count |
| GoogleTest | each `[==========] N tests from M test suites ran.` line, summed, and the singular `1 test from 1 test suite ran.`; the `Running` line does not count |
| Unity | each `N Tests F Failures I Ignored` line, `N` less the ignored `I`, summed |

rvow counts only a summary line at the start of a line, like the other runners, so a line that a test prints itself, indented or with a prefix, does not count.
A doctest or GoogleTest binary has no fixed name, so rvow reads its output wherever it appears.

rvow removes the color codes from a line before it reads the count, so a runner that prints its summary in color, such as vitest in a terminal, counts the same as one that does not.
The tail that `rvow show` prints for the run keeps the codes.

### More than one runner

When the output holds the summaries of more than one runner, rvow adds their counts.
A run in which one of the runners ran 0 tests is not a proof, so the run records 0 tests.
One runner keeps its own count.
The lines of one runner, such as the `test result:` lines of libtest or the summary lines of doctest, are added into a single count first, so a crate with 0 doc tests and 5 unit tests is 5.

`pytest` that exits with 5, no tests ran, is 0 as well.

## The full-suite warning

`rvow check` warns with `full-suite` about a task whose verify chains three or more commands with `&&` or `;`, repeats a `checks` entry, or took more than one minute on its last recorded run.
A file check does not count toward the chain: a segment whose first word is `grep`, `rg`, `test`, `[`, `diff` or `cmp`, after any leading `!` and `NAME=value` words.
So `grep -q x a.md && grep -q y b.md && cargo test --test main docs::` chains one command and does not warn.

## The task size warnings

A task that is wide makes the gate slow and makes other proofs go stale.
`rvow check` warns about these four shapes.

- `long-task`: a task title of more than 60 words. Keep the title to one line and put the detail in the spec or in the test.
- `broad-verify`: a verify that builds or runs everything, such as `cmake --build --preset tests` with no `--target`, `ninja` or `make` with no target, or `ctest` with no `-R`, `-L`, `-E` or `-I`. Build one target and run its tests.
- `depends-chain`: a task that ends a `depends_on` chain of three or more tasks. One edit in any of them makes the chain stale.
- `hot-file`: a tracked file in the `files:` of three or more tasks. One edit to it makes all of those proofs stale. Name the file in one task only.

Put the full build, the full suite, benchmarks and timing checks in one last gate task or in the project `checks`.
A timing check in a task verify makes the gate flaky.

## Running each check once per commit

A `checks` entry that passes records its result in the same gate cache as a rerun task, so a commit that already passed the check does not run it again.
The key is the command text, the id of the HEAD tree and the tool fingerprint, so a new commit or a changed tool runs the check again.
When the key holds a pass and the check's scope has no uncommitted change, the gate skips the check.
It prints `cached check: <cmd>` on the same stream as `check:` and `check done:`.

A tree with uncommitted changes caches a passed check by the content of every file of the work tree instead of the commit.
So a retry on the same files skips a check that passed, and any edit runs it again.
A check that failed is never cached.

The scope of a check is as follows:

- A plain check has the whole tree as its scope, new untracked files included.
- A `{ run, files }` entry has the files its globs match.

An uncommitted change in the check's scope runs the check and records nothing, so a pass never counts for content that is not committed.
`rerun_cache = "off"` turns the check cache off too, so every check runs.

## How long a cached pass lasts

An entry in the gate cache stops answering for a task after `max_age`, which a task sets with a `max_age:` line such as `max_age: 24h`.
A task without one uses `rerun_cache`, and a project without that uses 168 hours.
Set `rerun_cache = "off"` to run every task each time.
`max_age`, `rerun_cache` and `cache_with_links` tune the cache, so changing them is not drift.

## Linked build output and the cache

A task that runs against linked build output, listed in `rerun_links`, can pass or fail on files that git does not track.
For that reason the gate turns the cache off whenever `rerun_links` is set, and prints a warning that says so.
Set `cache_with_links = true` when you know the linked output does not change a result.

## Tools in the cache key

The gate cache skips a task whose inputs did not change.

A compiler or container runtime that changes under the same project files can change a result, so list it in `tools`.
Each entry is a name on `PATH` or a path.
The cache key holds the sha256 of the file after links are resolved.
A tool that is missing makes the task uncacheable, so the gate runs it.

## The grammar cache

rvow compares protected files with the syntax tree of their text, so a file whose only edit lost a blank line is not drift.
A comment counts as code, so adding, removing or changing a comment is drift.
Reindenting a comment or changing only the spaces inside it is not drift.
For that rvow needs a tree-sitter grammar for the file's language.

### Fetching grammars

rvow keeps prebuilt `.wasm` grammars in the grammar cache, the `grammars/` folder inside the rvow home.
The pinned list of languages and their download URLs and hashes ships inside the rvow binary.
These commands manage the cache:

- `rvow grammar fetch` fills the cache from the pinned list, checks every download's sha256 against the pin and writes the file only when it matches.
- `rvow grammar fetch json` fills one language.
- `rvow grammar list` shows each pinned language and whether the cache holds it.

Set `RVOW_GRAMMAR_URL` to move the download base from the pinned GitHub release URLs to a folder or a file:// URL or an HTTP(S) base.
The file name of the download stays the pinned one, so a mirror folder of `tree-sitter-<lang>.wasm` files is enough.

### When a grammar is missing

The drift check in a hook, the view or amend does not download at check time.
It compares only with a grammar the cache already holds.
`rvow prove` and `rvow gate` fetch first, so the prove and gate fetch fills the cache with the missing pinned grammars of the changed protected files, waiting about 10 s per grammar.
A failed fetch keeps the raw rules and names `rvow grammar fetch`, so a missing grammar never weakens drift.

For one hour after a fetch fails in prove or gate, neither tries that language again and both treat it as a language with no grammar.
The drift item then keeps the raw rule and the `rvow grammar fetch` hint.
`rvow grammar fetch` always tries, and a success clears the one-hour record.

### Compare results

Each compare result is saved in the `grammars/` folder, keyed by the hash of each text and the grammar hash.
The next compare of the same two texts reuses it without parsing.
When rvow saves a compare result, it removes the saved compare results older than 30 days.
It removes only the result files it wrote, and a file it cannot remove never fails the save.

## Syntax units and query files

A protected test file is not compared line by line when its grammar is in the cache.
rvow splits it into syntax units and compares units instead.

### What a unit is

A unit is an item together with the attribute, decorator and comment siblings right before it.
In Rust `#[test] fn a` is one unit, so removing `#[test]` or adding `#[ignore]` to it changes the unit.
A unit keeps its container, so its form starts with the headers of the containers around it.
Moving a test into another module, class or describe block is therefore drift.

A body container that is not a function splits into a header unit plus one unit per body item.
These are such containers:

- a Rust module
- a Python class
- a C++ namespace
- a Java, Kotlin, C# or PHP class or namespace
- a call a query marks `@group`, such as `describe(...)`

The header unit is the container without its body, so renaming the module is drift.
The body of a function never splits, so a statement added inside a test changes that test's unit.
The compare counts, so each old unit must keep its canonical form in the new text at least as many times as before.
Deleting one of two identical tests is drift.

### When a new unit is free

A new unit is free only when all of these hold:

- A shipped query marks it a test.
- Its name is one the old text does not use.
- Every attribute above it is a test attribute (`#[test]`, `#[tokio::test...]`, `#[should_panic...]` or `@pytest.mark.*`).

A Java, Kotlin, C# or PHP test also needs every annotation or attribute it holds to be a test one (see the shipped queries below).
A comment above it is free only after the first old unit of the file.

### What is always drift

A new test that calls a function that ends the process is drift, even when a query marks it.
These are a process exit or abort such as `std::process::exit`, `sys.exit`, `os._exit`, `exit()`, `process.exit`, `os.Exit`, `log.Fatal*`, `System.exit`, `Environment.Exit` or PHP `exit`.
rvow reads the names in the syntax tree, so a string `"exit"` or a variable named `exit` is not a process exit.
A qualified or indirect call such as `java.lang.System.exit(0)`, `(process.exit)(0)`, `dbg!(std::process::exit(0))` or a bare Ruby `exit if done` is.

A string or a symbol that names one of those functions is a process exit where it is used to reach one.
These are the places where it counts:

- As the key of a subscript or element access on a process-like object, such as `process["exit"]` or ``process[`exit`]``.
- As an argument of a call that looks a function up by name on one, such as `getattr(sys, "exit")`, `Reflect.get(process, "exit")`, `Kernel.send`, `public_send`, `__send__`, `method`, `getMethod` and `getDeclaredMethod`.

A process-like object is `process`, `sys`, `os`, `System`, `Kernel`, `Runtime` and the like, or a module loaded by name such as `__import__("os")`.
A bare Ruby `send(:exit)` counts too, so `ws.send('exit')` or `cfg['exit']` on other objects stays free.
The same string in any other place is not an exit, so `assert_eq!(code("exit"), 1)` stays free.
A call made through a string, such as `getattr(sys, "exit")()`, is drift too, as a call that reaches an exit function by name is.

`exit`, `abort` and `terminate` count only when they are called bare or on a process-like object (`process`, `sys`, `os`, `System`, `Environment`, `Kernel`, `Runtime`, `std`, `libc`), so `controller.abort()` stays free.
A new test call that focuses the run on itself is drift too, because it makes the runner skip every other test.
That means RSpec `focus: true` or `:focus` or a node test `{ only: true }` option.

### New test modules in Rust

A new `mod NAME;` line with no body in a protected Rust test file (the mod line rule) is free too, when all of these hold:

- Its only attributes are `#[path = "..."]` and `#[cfg(test)]`, so `#[macro_use]` or any other `#[cfg(...)]` is drift.
- The module file is new since accept, so a `mod` line that names a file that was protected at accept is drift.
- The file exists now, found as rustc finds it (the `#[path]` value against the folder of the declaring file, else `NAME.rs` or `NAME/mod.rs` under the folder of `main.rs`, `lib.rs` or `mod.rs`, or under `<stem>/` for any other file).
- The file calls no process exit, focuses no run, and brings in no code the compare does not read.
  That means no `mod` line of its own, no `include!` and no start-up attribute such as `#[ctor]` or `#[link_section]`.

A `mod` line inside an inline module is drift.

### Other new items

A new file that matches `protect_infra` or the enforcement files after accept is a new protected infra file and is drift (`protected file <path> added`).
Examples are `tests/conftest.py` and `.cargo/config.toml`.
A new file that only matches `test_globs` stays free.

A new helper, hook, fixture, import or `macro_rules!` is drift, so record it with `rvow amend`.
Moving a unit inside its container is not drift.
Plain Rust `//` comments are left out of unit forms, and doc comments and comments in other languages are kept.

### When the grammar does not load

A file whose grammar does not load, or whose text does not parse cleanly, keeps the raw rules.
These are the line rule for most files and the Rust test-module rule for `.rs`.
A C or C++ file is the one exception, because it is compared by unit even when its text holds other parse errors, such as a default argument of `= {}` that tree-sitter cannot parse.
Each error region is then part of the unit around it, so a change inside one, such as `= {}` to `= {1}`, is drift, and a new unit that holds an ERROR node is never free.

### Shipped queries

The drift item names the test that changed when a query file marks its unit.
Each shipped query marks the test units of the common frameworks:

- `test(...)`, `it(...)` and `describe(...)` calls for JavaScript and TypeScript
- pytest and unittest `test_*` functions for Python
- `#[test]` functions for Rust
- GoogleTest `TEST`, `TEST_F` and `TEST_P` macros for C++, named `Suite.Name`
- `func TestX` for Go

The queries for Java, Kotlin, C#, Ruby, PHP, C++ (doctest and Catch2) and C (Unity) mark these as tests:

| Language | Marked as a test | Never free |
|----------|------------------|------------|
| Java and Kotlin (JUnit) | methods with `@Test`, `@ParameterizedTest`, `@RepeatedTest` or `@TestFactory`; classes are containers | `@BeforeEach`, `@BeforeAll`, `@AfterEach`, `@AfterAll`, `@Before`, `@After`, and a test with another annotation such as `@Disabled` or `@Timeout` |
| C# (xUnit, NUnit, MSTest) | methods with `[Fact]`, `[Theory]`, `[Test]`, `[TestCase(...)]` or `[TestMethod]`; classes and namespaces are containers | `[SetUp]`, `[TearDown]`, `[TestInitialize]`, `[ClassInitialize]`, and a test with another attribute such as `[Skip]` or `Skip = ...` |
| Ruby (RSpec, Minitest) | `it`, `specify`, `example` and `test "..."` calls; `describe` and `context` blocks are `@group` containers | `before`, `after`, `let`, `subject` and `around` |
| PHP (PHPUnit) | methods named `test*`, and methods with `#[Test]` or a `/** @test */` docblock; classes are containers | `setUp`, `tearDown`, `setUpBeforeClass` and every helper |
| C++ (doctest, Catch2) | `TEST_CASE`, `TEST_CASE_FIXTURE`, `TEST_CASE_METHOD`, `TEST_CASE_TEMPLATE`, `TEMPLATE_TEST_CASE` and `SCENARIO` blocks, named by their string; `TEST_SUITE` blocks are `@group` containers; `SUBCASE` and `SECTION` stay inside their case, so a change in one names the case | a case with a `skip()`, `may_fail()`, `should_fail()` or `expected_failures()` decorator or a Catch2 tag `[.]`, `[hide]`, `[!hide]`, `[!mayfail]` or `[!shouldfail]`, and every helper |
| C (Unity) | `test_*` functions of a file that includes `unity.h`, and the `RUN_TEST(test_name)` lines of a `main` that holds a `RUN_TEST` line | `setUp`, `tearDown`, every helper, a `RUN_TEST` of a name the old text already uses, and a test that calls `TEST_IGNORE` |

### C, C++ and the other languages

In C and C++ a new test that holds a preprocessor directive (`#define`, `#undef`, `#include`) is drift, since it can switch off the asserts of later tests.
Naming `exit`, `abort` or `quick_exit` counts as a process exit even without a call, as does `::exit`, `raise` or `kill`.
A new test in these languages that calls a process exit (`System.exit`, `Runtime...halt`, `exitProcess`, `Environment.Exit`, `exit`, `abort`, `Process.exit`, `die`) is drift too.

A doctest or Catch2 macro such as `TEST_CASE("adds") { ... }` has no `;`, so tree-sitter-cpp parses it as a header with a missing `;` followed by a block, and the parse holds an error.
rvow accepts that error in a C++ file and joins the header and its block into one unit.

The statements of a Unity `main` keep their order, so moving `return UNITY_END();` above a `RUN_TEST` line is drift.
A `RUN_TEST` rule applies to a `main` that holds at least one `RUN_TEST` line.
The header of `main` is one unit and each statement is its own unit, so a new `RUN_TEST(test_name);` line for a new free `test_` function is free and a removed line is drift.
A `main` of any other C file stays one unit, and a `.h` file frees no new unit unless it includes `unity.h`.

### Project query files

A project adds its own query file per language with `test_queries` in the project config:

```toml
test_queries = { javascript = "tests/queries/chai.scm" }
```

The path is repo-relative and the query adds to the shipped one for that language.
Inside the query, a `@test` capture marks a unit as a test when it spans the whole unit.
A `@name` capture holds the title or function name the item prints, and several `@name` captures join with `.`.
A `@group` capture marks a call as a container that splits into a header unit plus one unit per body item.

A project query file named in `test_queries` is a protected file, like a test file.
It is in the protected set at accept, and a later change to it is drift (`protected file <path> changed`).
A trusted query can split a `@group` container and make a new unit free under the same rules as a shipped mark.
The unit needs a `@name`, a name the old text does not use, test attributes only, no process exit and no focus.

A query is trusted when its file was protected at accept and is unchanged.
A query file that was not there at accept, or that was named in `test_queries` after accept, is not trusted.
The config change is drift already, and the query only names tests in drift items.

Here is an example query.

```scheme
(
  (call_expression
    function: (identifier) @fn
    arguments: (arguments (string (string_fragment) @name))) @test
  (#eq? @fn "check")
)
```

A unit no query marks keeps the unnamed item text.
A missing or broken query file never weakens the check.
The report just loses the name, and `rvow check` warns with `test-query` and names the file.
Without `test_queries`, only the shipped queries name tests.

## Choosing the git binary

rvow runs the git it finds on `PATH`.
On macOS, when that git is `/usr/bin/git`, rvow runs the real git behind it, which `xcrun --find git` names, since the shim adds about 6 ms to each git call.
It keeps that path in `hook-state/git-bin` in the rvow home and asks `xcrun` again only when the path is gone.
It runs a kept path only when root owns the file, so a path that an agent writes there is never run.

Set `RVOW_GIT` to the path of a git binary to run that one for every git call instead.
Set it when the git on `PATH` cannot run, such as a sandbox where `/usr/bin/git` is a launcher shim that the sandbox stops.
An empty value falls back to `PATH`.

The agent eval runner passes the same path as `EVAL_RVOW_GIT`, which is the only spelling that reaches inside the eval sandbox.

## Runners

The `[runners]` table of the project config holds a named command template, and a task verify names it with `verify: @<runner> key=value`.

```toml
[runners.cpp]
run     = "cmake --build --preset {preset} --target {target} && ctest --preset {preset} -R '{test}' --no-tests=error --output-junit {junit}"
params  = { preset = ["tests", "asan", "tsan"] }
default = { preset = "tests" }
needs   = ["build/build-{preset}"]
env     = { CPM_SOURCE_CACHE = "/tmp/cpm" }
timeout = "5m"
```

Every key except `run` is optional.

### Filling `run`

Each `{param}` of `run` gets its value from the task's `key=value` pairs or from `default`.
`{junit}` is filled by rvow itself with a fresh report path, quoted for the shell.
Write `{junit}` without quotes in `run`, because `rvow check` refuses a runner that quotes it.
The report goes in the system temp dir, or in the dir that the runner's `env` sets as `RVOW_JUNIT_DIR`, and rvow removes it after it reads it.

A task key that `run` does not use, a value outside `params`, and a param with no value are `rvow check` errors.

Each value lands inside a shell command, so it may hold only `A-Z a-z 0-9 _ . : / + = * ^ $` and `-`.
rvow refuses any other char, at check time and again before a run, and names the char.
A `default` value follows the same char rule and the same `params` list.
A task's own `timeout:` wins over the runner's `timeout`.

### The JUnit report

When `run` holds `{junit}`, prove reads the JUnit report.
A case is failed when it holds a `<failure>` or an `<error>` child.
Its name is the `name` attribute, or `classname.name` when both are there.
At prove and at the gate rerun the proof passes only when all of these hold:

- The command exits 0.
- The report exists.
- The report holds at least one passed case.
- No test case failed.

A vow from an older rvow may name failed cases, and the proof then needs each of those to be there and to pass.

### Build directories

Each path of `needs` is filled with the same params and must exist before anything runs.
Accept, prove and the gate rerun run no command and name the missing path, so configure that build dir first.

`rvow check` warns with `configure-in-verify` when a plain verify or a runner's `run` configures CMake.
That means the words `cmake --preset`, `cmake -S`, `cmake -B` or any `--fresh`.
`cmake --build --preset` only builds and does not warn.

When the output of a verify run holds `Re-running CMake`, the prove result notes that the build reconfigured.
When the runner has a `needs` path, it also names it: `run ninja -C <dir> -d explain to see why`.

### Keeping a proof when the artifacts are unchanged

A proof goes stale when a file in the task's `files:` changes, also when the change does not alter what the test runs.
A runner can set two more keys, and rvow then keeps such a proof without a verify run.
rvow knows no build system for this. The runner names the build and the files it makes.

```toml
[runners.cpp]
run = "cmake --build build --target {target} && ctest --test-dir build -R '^{target}$'"
build = "cmake --build build --target {target}"
artifacts = ["build/tests/{target}"]
```

- `build` is an incremental build command of what `run` executes, with the same `{param}` fill as `run`.
- `artifacts` lists the built files that `run` executes, relative to the work tree. A path may use the same params. A file that the test reads but the build does not make, such as a data file, goes in the list too.
- A runner that sets one of the two must set both. The two keys stay out of the runner hash, so adding them makes no proof stale.

A passing proof records the SHA-256 of each artifact.
`rvow prove --stale`, the background re-prove and `rvow archive` then try this before they run a verify.
The task must be stale only because its input files or a dependency changed, and its last proof must have passed with no `mutants:` and no private check.
rvow runs `build`, hashes the artifacts and compares them with the recorded hashes.
When every hash is the same, rvow records a proof that carries the old results and marks it `revalidated`, and the verify does not run.
When one differs, or the build fails, rvow runs the verify as before.

A build that is not reproducible, such as one that embeds a timestamp, gives a new hash each time.
That only costs a verify run.

On macOS a Debug link writes the modification time of each object file into the binary, so a comment-only edit changes every dylib and test binary.
Set `env = { ZERO_AR_DATE = "1" }` on the runner. The runner `env` applies to `run`, `build` and `always`, so it need not go in each command.

Three more keys keep a runner short and its kept proof honest.

- `always` lists cheap commands, with the same `{param}` fill. They run before each revalidation, and a failed one runs the verify instead. Use it for checks on source text that the artifacts do not cover, such as `test ! -e include/old.h` or `! git grep -q <pattern>`. It needs `build`.
- `extends = "<runner>"` starts from another runner. A set `run`, `build` or `timeout` wins. The maps `params`, `default` and `env` merge with the child's keys winning. The lists `needs`, `artifacts` and `always` add up. A runner that has no `run` is a base only, and a verify cannot name it.
- An `artifacts` entry with `*`, `?` or `[` is a glob, such as `build/lib/*.so`. rvow hashes every match and records the file names, so a new or a missing match runs the verify. A glob must match at least one file.
The gate in a clean worktree does not revalidate, since it builds from nothing.

### Runners and the vow

The vow records a hash of each runner a task uses (`run`, `env`, `timeout`, `params`, `default`, `needs`, `always`; not `build` and `artifacts`, and an `extends` is merged first).
A changed runner is weaken drift for each task that uses it, like a changed verify, and a runner that no task uses can change with no drift.
The runner hash is also part of the proof key and the gate cache key, so a confirmed runner change makes the old proofs stale.
`rvow review` and `rvow show` print the filled command as a `run:` line under the `verify:` line.

> [!NOTE]
> A bundle refuses a task with an `@<runner>` verify, so give a bundled task a plain verify.
