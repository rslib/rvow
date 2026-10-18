//! `rvow check`: a read-only report on a plan before the user accepts it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::change::{self, ChangeDir, Loaded, Stage};
use crate::config::{self, Config};
use crate::error::Result;
use crate::git;
use crate::ledger::{self, Kind};
use crate::lint::{Finding, Question, questions};
use crate::store::Project;
use crate::tasks::Task;

/// Shell builtins and keywords a verify command may start with.
const BUILTINS: &[&str] = &[
    "cd", "test", "[", "true", "false", "echo", "export", "set", "exit", ":", "printf", "read", "source", ".", "eval", "command", "type",
    "if", "for", "while", "!", "(", "{",
];

/// The program a shell command runs first, after `VAR=value` assignments and
/// `env`.
fn program(cmd: &str) -> Option<&str> {
    cmd.split_whitespace().find(|w| !(w.contains('=') && !w.starts_with('=')) && *w != "env")
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            let p = dir.join(name);
            p.is_file()
        })
    })
}

/// Tasks whose verify command starts with a program that neither exists on
/// PATH nor as a file in the repository. Such a command fails for the wrong
/// reason, so its failing baseline proves nothing.
/// The command each task runs, with a `@runner` verify filled from the
/// project config. A task whose fill fails keeps its raw verify text;
/// `runner_checks` reports the fill error itself.
pub fn filled_commands(runners: &BTreeMap<String, crate::template::Runner>, tasks: &[Task]) -> BTreeMap<String, String> {
    tasks
        .iter()
        .map(|t| {
            let cmd = crate::template::plan(runners, t).map(|p| p.command.clone()).unwrap_or_else(|_| t.verify.clone().unwrap_or_default());
            (t.id.to_string(), cmd)
        })
        .collect()
}

pub fn missing_programs(top: &Path, tasks: &[Task]) -> Vec<(String, String)> {
    let raw: BTreeMap<String, String> = tasks.iter().map(|t| (t.id.to_string(), t.verify.clone().unwrap_or_default())).collect();
    missing_programs_filled(top, tasks, &raw)
}

/// Like `missing_programs`, with each task's command already filled from its
/// runner, when it names one.
pub fn missing_programs_filled(top: &Path, tasks: &[Task], filled: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for t in tasks {
        let Some(cmd) = filled.get(&t.id.to_string()).map(String::as_str) else {
            continue;
        };
        let Some(prog) = program(cmd) else {
            continue;
        };
        let found = BUILTINS.contains(&prog)
            || if prog.contains('/') {
                let p = Path::new(prog);
                if p.is_absolute() { p.exists() } else { top.join(p).exists() }
            } else {
                on_path(prog)
            };
        if !found {
            out.push((t.id.to_string(), prog.to_string()));
        }
    }
    out
}

/// One full-suite verify warning: the task and why its verify slows accept.
/// The gate's `checks` already run the whole suite, so a task verify that
/// repeats them makes the accept baseline as slow as the gate.
#[derive(Debug)]
pub struct SlowVerify {
    pub task: String,
    pub message: String,
}

/// A verify that chains this many commands with `&&` or `;` is a suite of its own.
pub const CHAINED_COMMANDS: usize = 3;
/// A verify whose last recorded run took longer than this is a full suite.
pub const SLOW_PROOF_SECS: u64 = 60;

/// Programs that only look at files, so chaining them does not make a suite.
const FILE_CHECKS: [&str; 6] = ["grep", "rg", "test", "[", "diff", "cmp"];

/// The program of a segment, after any leading `!` words.
fn segment_program(segment: &str) -> Option<&str> {
    program(segment.trim_start().trim_start_matches(['!', ' ']))
}

/// The commands a verify chains with `&&` or `;`, not counting file checks.
pub fn chained_commands(cmd: &str) -> usize {
    cmd.replace("&&", ";").split(';').filter_map(segment_program).filter(|p| !FILE_CHECKS.contains(p)).count()
}

/// The duration of the newest recorded proof of `cmd` in this project, in ms.
fn last_run_ms(project: &Project, cmd: &str) -> Option<u64> {
    let dirs = change::list(project).ok()?.0;
    let mut newest: Option<(u64, u64)> = None;
    for d in &dirs {
        let Ok((events, _)) = ledger::read(&d.path.join(change::LEDGER)) else { continue };
        for e in events.iter().filter(|e| e.kind == Kind::Prove) {
            if e.data.get("verify").and_then(Value::as_str) != Some(cmd) {
                continue;
            }
            let Some(ms) = e.data.get("duration_ms").and_then(Value::as_u64) else { continue };
            if newest.is_none_or(|(time, _)| e.time > time) {
                newest = Some((e.time, ms));
            }
        }
    }
    newest.map(|(_, ms)| ms)
}

/// A warning for each task whose verify runs the full suite instead of one
/// target: it chains commands, repeats a configured check, or ran slowly the
/// last time the same command was proved in this project.
pub fn full_suite(project: &Project, config: &Config, tasks: &[Task], filled: &BTreeMap<String, String>) -> Vec<SlowVerify> {
    let mut out = Vec::new();
    for t in tasks {
        let Some(cmd) = filled.get(&t.id.to_string()).map(|s| s.trim()) else { continue };
        let mut why = Vec::new();
        let chained = chained_commands(cmd);
        if chained >= CHAINED_COMMANDS {
            why.push(format!("chains {chained} commands"));
        }
        if config.checks.iter().any(|s| s.run().trim() == cmd) {
            why.push("repeats a configured `checks` entry".to_string());
        }
        if let Some(ms) = last_run_ms(project, cmd)
            && ms > SLOW_PROOF_SECS * 1000
        {
            why.push(format!("took {} s on its last recorded run", ms / 1000));
        }
        if why.is_empty() {
            continue;
        }
        out.push(SlowVerify {
            task: t.id.to_string(),
            message: format!(
                "{} verify ({cmd}) {}; put the full suite in the project `checks` and keep this verify targeted",
                t.id,
                why.join(", ")
            ),
        });
    }
    out
}

#[derive(Debug)]
pub struct Report {
    pub change: ChangeDir,
    pub findings: Vec<Finding>,
    pub missing: Vec<(String, String)>,
    pub questions: Vec<Question>,
    pub no_tasks: bool,
    pub slow: Vec<SlowVerify>,
}

impl Report {
    pub fn blocking(&self) -> bool {
        self.findings.iter().any(|(_, d)| d.severity() == crate::diag::Severity::Error)
            || !self.missing.is_empty()
            || !self.questions.is_empty()
            || self.no_tasks
    }
}

/// A task whose `files:` match more than this many tracked files goes stale on most changes.
const WIDE_FILES: usize = 40;

/// A warning for each task whose scope covers a large part of the repository: its proof goes
/// stale whenever any of those files changes, so the plan should name what the task changes.
fn wide_scopes(top: &Path, tasks: &[Task]) -> Vec<Finding> {
    let Ok(listing) = git::run(top, &["ls-tree", "-r", "--name-only", "HEAD"]) else {
        return Vec::new();
    };
    let files: Vec<&str> = listing.lines().collect();
    tasks
        .iter()
        .filter_map(|t| {
            let set = crate::fingerprint::globset(&t.files).ok()?;
            let n = files.iter().filter(|f| set.is_match(f)).count();
            (n > WIDE_FILES).then(|| {
                let message =
                    format!("{} covers {n} tracked files ({}); its proof goes stale when any of them changes", t.id, t.files.join(", "));
                (change::TASKS.to_string(), crate::diag::Diagnostic::new(crate::diag::Rule::WideScope, t.line, message))
            })
        })
        .collect()
}

/// The most shared files a `shared-scope` warning lists by name.
const SHARED_LISTED: usize = 3;

/// A warning for each pair of tasks whose `files:` match a common tracked file: the proof
/// of both goes stale when that file changes.
fn shared_scopes(top: &Path, tasks: &[Task]) -> Vec<Finding> {
    if tasks.len() < 2 {
        return Vec::new();
    }
    let Ok(listing) = git::run(top, &["ls-tree", "-r", "--name-only", "HEAD"]) else {
        return Vec::new();
    };
    let sets: Vec<_> = tasks.iter().map(|t| crate::fingerprint::globset(&t.files).ok()).collect();
    let mut shared: BTreeMap<(usize, usize), Vec<&str>> = BTreeMap::new();
    for file in listing.lines() {
        let hits: Vec<usize> =
            sets.iter().enumerate().filter(|(_, s)| s.as_ref().is_some_and(|s| s.is_match(file))).map(|(i, _)| i).collect();
        for (n, &a) in hits.iter().enumerate() {
            for &b in &hits[n + 1..] {
                shared.entry((a, b)).or_default().push(file);
            }
        }
    }
    shared
        .into_iter()
        .map(|((a, b), files)| {
            let listed = files.iter().take(SHARED_LISTED).copied().collect::<Vec<_>>().join(", ");
            let rest = files.len().saturating_sub(SHARED_LISTED);
            let more = if rest > 0 { format!(" and {rest} more") } else { String::new() };
            let noun = if files.len() == 1 { "file" } else { "files" };
            let message =
                format!("{} and {} share {} {noun}: {listed}{more}; their proofs go stale together", tasks[a].id, tasks[b].id, files.len());
            (change::TASKS.to_string(), crate::diag::Diagnostic::new(crate::diag::Rule::SharedScope, tasks[b].line, message))
        })
        .collect()
}

/// Shell operators that make a verify command more than one program, so the
/// single-program rules below do not apply.
const OPERATORS: &[&str] = &["&&", "||", ";", "|", "&", ">", "<", "`", "$(", "\\", "\n"];

/// The whole verify command is a stub that runs no check: `true`, `:`, `exit 0`,
/// one `echo` or one `test`.
fn trivial_verify(cmd: &str) -> bool {
    let cmd = cmd.trim();
    if cmd.is_empty() || OPERATORS.iter().any(|op| cmd.contains(op)) {
        return false;
    }
    let mut words = cmd.split_whitespace();
    match words.next().unwrap_or("") {
        "true" | ":" | "echo" | "test" | "[" => true,
        "exit" => words.next() == Some("0") && words.next().is_none(),
        _ => false,
    }
}

/// The verify command runs a test runner whose output counts tests, so
/// `baseline: pass` says more than "the command exited 0".
fn counted_runner(cmd: &str) -> bool {
    let words: Vec<&str> = cmd.split_whitespace().collect();
    let pair = |a: &str, b: &str| words.windows(2).any(|w| w == [a, b]);
    pair("cargo", "test")
        || pair("cargo", "nextest")
        || pair("go", "test")
        || words.contains(&"pytest")
        || words.contains(&"jest")
        || words.contains(&"vitest")
        || words.contains(&"mocha")
        || words.contains(&"ctest")
        || (words.contains(&"node") && words.contains(&"--test"))
}

/// A warning for each task whose check proves nothing: a stub command, or a
/// `baseline: pass` on a command with no counted test runner.
fn weak_checks(tasks: &[Task], filled: &BTreeMap<String, String>) -> Vec<Finding> {
    let mut out = Vec::new();
    for t in tasks {
        let Some(cmd) = filled.get(&t.id.to_string()) else {
            continue;
        };
        let cmd = cmd.as_str();
        let trivial = trivial_verify(cmd);
        if !trivial && !(t.baseline_pass && !counted_runner(cmd)) {
            continue;
        }
        let message = if trivial {
            format!(
                "{} has a trivial verify ({cmd}); replace it with the command that fails today, so the check cannot pass while the work is missing",
                t.id
            )
        } else {
            format!(
                "{} sets baseline: pass but its verify ({cmd}) runs no counted test runner; use cargo test, cargo nextest, pytest, jest, vitest, mocha, node --test, go test or ctest",
                t.id
            )
        };
        out.push((change::TASKS.to_string(), crate::diag::Diagnostic::new(crate::diag::Rule::WeakVerify, t.line, message)));
    }
    out
}

/// Whether the missing-program rule may blame `task`: a task whose runner
/// fails to fill already gets the runner error from `runner_checks`, so the
/// raw `@text` must not count as a missing program too.
fn fill_ok(runners: &BTreeMap<String, crate::template::Runner>, task: &Task) -> bool {
    crate::template::reference(task.verify.as_deref().unwrap_or("")).is_none() || crate::template::plan(runners, task).is_ok()
}

/// The missing programs that `check` blames on a task: those of tasks whose
/// runner fills, since a failed fill already gets the runner error.
pub fn blamed_programs(
    top: &Path,
    tasks: &[Task],
    runners: &BTreeMap<String, crate::template::Runner>,
    filled: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    missing_programs_filled(top, tasks, filled)
        .into_iter()
        .filter(|(task, _)| tasks.iter().find(|t| t.id.to_string() == *task).is_none_or(|t| fill_ok(runners, t)))
        .collect()
}

const REQUIRED_SECTIONS: [&str; 3] = ["Why", "What changes", "Out of scope"];

/// A structure error for each required proposal section that holds no words.
/// Only `rvow check` runs it: the `rvow new` scaffold is blank by design.
fn blank_sections(src: &str) -> Vec<crate::diag::Diagnostic> {
    let mut sections: Vec<(&str, usize, usize)> = Vec::new();
    for (i, line) in src.lines().enumerate() {
        if let Some(name) = line.strip_prefix("## ") {
            sections.push((name.trim(), i + 1, 0));
        } else if !line.starts_with('#')
            && let Some(last) = sections.last_mut()
        {
            last.2 += line.split_whitespace().count();
        }
    }
    sections
        .into_iter()
        .filter(|(name, _, n)| *n == 0 && REQUIRED_SECTIONS.contains(name))
        .map(|(name, line, _)| {
            crate::diag::Diagnostic::new(crate::diag::Rule::Structure, line, format!("proposal section `## {name}` is empty"))
        })
        .collect()
}

/// The blank-section errors of a loaded change's proposal, keyed by the
/// proposal file. `rvow check` and the dashboard both count them.
pub fn proposal_blanks(loaded: &Loaded) -> Vec<(String, crate::diag::Diagnostic)> {
    loaded
        .texts
        .iter()
        .find(|(path, _)| path == change::PROPOSAL)
        .map(|(_, text)| blank_sections(text).into_iter().map(|d| (change::PROPOSAL.to_string(), d)).collect())
        .unwrap_or_default()
}

fn report(project: &Project, config: &Config, change: ChangeDir, loaded: Loaded, top: &Path) -> Report {
    let runners = config::runners(project).unwrap_or_default();
    let filled = filled_commands(&runners, &loaded.tasks);
    let slow = full_suite(project, config, &loaded.tasks, &filled);
    let blanks = proposal_blanks(&loaded);
    let mut findings = loaded.findings;
    findings.extend(blanks);
    findings.extend(crate::lint::runner_checks(&runners, &loaded.tasks));
    findings.extend(crate::lint::configure_warnings(&runners, &loaded.tasks));
    findings.extend(wide_scopes(top, &loaded.tasks));
    findings.extend(shared_scopes(top, &loaded.tasks));
    findings.extend(weak_checks(&loaded.tasks, &filled));
    let missing = blamed_programs(top, &loaded.tasks, &runners, &filled);
    Report { missing, questions: questions(&loaded.texts), no_tasks: loaded.tasks.is_empty(), findings, change, slow }
}

/// The top of the worktree that holds the change's branch: the worktree of
/// `top` when its branch is checked out there, else the worktree of the same
/// repository that has it, else `top`. The files a check reads, and the
/// programs it blames, belong to that worktree.
fn change_top(top: &Path, loaded: &Loaded) -> PathBuf {
    let branch = loaded.proposal.branch.as_str();
    if branch.is_empty() {
        return top.to_path_buf();
    }
    git::top_for_branch(top, branch).ok().flatten().filter(|other| other != top).unwrap_or_else(|| top.to_path_buf())
}

/// Checks `target`, or every open change whose proposal names the current
/// branch.
pub fn check(project: &Project, cwd: &Path, target: Option<&str>) -> Result<Vec<Report>> {
    let top = git::toplevel(cwd)?;
    let (living, _) = change::load_living(project)?;
    let dirs = match target {
        Some(t) => vec![change::resolve(project, t)?],
        None => {
            let branch = git::branch(&top)?.unwrap_or_default();
            let (all, _) = change::list(project)?;
            let mut out = Vec::new();
            for c in all.into_iter().filter(|c| matches!(c.stage, Stage::Backlog | Stage::Active)) {
                if change::load(&c.path, &living)?.proposal.branch == branch {
                    out.push(c);
                }
            }
            out
        }
    };
    let home = crate::hardening::project_home(project)?;
    let config = config::load(project, &home)?;
    let mut out = Vec::new();
    for d in dirs {
        let loaded = change::load(&d.path, &living)?;
        let change_top = if target.is_some() { change_top(&top, &loaded) } else { top.clone() };
        let mut r = report(project, &config, d, loaded, &change_top);
        r.findings.extend(crate::lint::test_query_checks(&home, &change_top, &config));
        out.push(r);
    }
    Ok(out)
}

/// Moves a backlog plan to the current branch by rewriting its `branch:` line,
/// so it can be accepted here. Returns the branch it came from.
pub fn claim(project: &Project, cwd: &Path, input: &str) -> Result<String> {
    let c = change::resolve(project, input)?;
    if c.stage != Stage::Backlog {
        return Err(crate::error::Error::new(
            "stage",
            format!("{} is {}; only a backlog plan moves between branches", c.id, c.stage.name()),
            "an accepted change keeps its branch; amend it instead",
        ));
    }
    let branch = git::branch(&git::toplevel(cwd)?)?
        .ok_or_else(|| crate::error::Error::new("detached", "HEAD is detached", "check out the branch to claim the plan for"))?;
    let _lock = project.lock()?;
    let path = c.path.join(change::PROPOSAL);
    let text = std::fs::read_to_string(&path).map_err(|e| crate::error::Error::io(&path, e))?;
    let mut old = String::new();
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        match line.strip_prefix("branch:") {
            Some(rest) if old.is_empty() => {
                old = rest.trim().to_string();
                out.push_str(&format!("branch: {branch}\n"));
            }
            _ => out.push_str(line),
        }
    }
    if old.is_empty() {
        out = format!("branch: {branch}\n{text}");
    }
    std::fs::write(&path, out).map_err(|e| crate::error::Error::io(&path, e))?;
    Ok(old)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;
    use crate::store::{Mode, init};

    #[test]
    fn claim_moves_a_backlog_plan_to_this_branch() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &tmp.0.join("home"), Mode::Home).unwrap();
        let c = change::new(&p, "T", "elsewhere", true).unwrap();
        assert_eq!(claim(&p, &repo, &c.id.to_string()).unwrap(), "elsewhere");
        let text = std::fs::read_to_string(c.path.join(change::PROPOSAL)).unwrap();
        assert!(text.starts_with("branch: main\n"), "{text}");
        assert_eq!(check(&p, &repo, None).unwrap().len(), 1);
    }

    #[test]
    fn ctest_is_a_counted_runner_and_a_gtest_binary_is_not() {
        assert!(counted_runner("ctest --test-dir build -R hex64"));
        assert!(counted_runner("cmake --build build && ctest --output-on-failure"));
        assert!(!counted_runner("./build/tests/test_hex64"), "a doctest or gtest binary has no fixed name");
    }

    #[test]
    fn programs_skip_assignments_and_env() {
        assert_eq!(program("FOO=1 BAR=2 cargo test x"), Some("cargo"));
        assert_eq!(program("env RUST_LOG=x cargo test"), Some("cargo"));
        assert_eq!(program("./scripts/check.sh"), Some("./scripts/check.sh"));
        assert_eq!(program(""), None);
    }

    #[test]
    fn check_reports_missing_programs_and_questions() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        std::fs::create_dir_all(repo.join("scripts")).unwrap();
        std::fs::write(repo.join("scripts/ok.sh"), "true").unwrap();
        let p = init(&repo, &tmp.0.join("home"), Mode::Home).unwrap();
        let c = change::new(&p, "T", "main", true).unwrap();
        std::fs::write(
            c.path.join("tasks.md"),
            "- TSK-new1 A\n  verify: sh -c true\n  files: a\n- TSK-new2 B\n  verify: no-such-tool-xyz run\n  files: a\n- TSK-new3 C\n  verify: ./scripts/ok.sh\n  files: a\n- TSK-new4 D\n  verify: ./scripts/gone.sh\n  files: a\n",
        )
        .unwrap();
        let prop = std::fs::read_to_string(c.path.join("proposal.md")).unwrap();
        std::fs::write(c.path.join("proposal.md"), prop + "[NEEDS CLARIFICATION: Which? | recommended: this]\n").unwrap();
        let reports = check(&p, &repo, None).unwrap();
        assert_eq!(reports.len(), 1);
        let r = &reports[0];
        assert_eq!(
            r.missing,
            [("TSK-new2".to_string(), "no-such-tool-xyz".to_string()), ("TSK-new4".to_string(), "./scripts/gone.sh".to_string())]
        );
        assert_eq!(r.questions.len(), 1);
        assert!(r.blocking());

        git(&repo, &["checkout", "-q", "-b", "other"]);
        assert!(check(&p, &repo, None).unwrap().is_empty());
        assert_eq!(check(&p, &repo, Some(&c.id.to_string())).unwrap().len(), 1);
    }
    #[test]
    fn check_warns_about_a_scope_that_covers_much_of_the_repository() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        for i in 0..WIDE_FILES + 5 {
            std::fs::write(repo.join(format!("src/f{i}.txt")), "x").unwrap();
        }
        std::fs::write(repo.join("one.txt"), "x").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "files"]);
        let p = init(&repo, &tmp.0.join("home"), Mode::Home).unwrap();
        let c = change::new(&p, "T", "main", true).unwrap();
        std::fs::write(
            c.path.join("tasks.md"),
            "- TSK-new1 Wide\n  verify: true\n  files: src/**\n- TSK-new2 Narrow\n  verify: true\n  files: one.txt\n",
        )
        .unwrap();
        let r = &check(&p, &repo, None).unwrap()[0];
        let wide: Vec<_> = r.findings.iter().filter(|(_, d)| d.rule == crate::diag::Rule::WideScope).collect();
        assert_eq!(wide.len(), 1, "{:?}", r.findings);
        assert!(wide[0].1.message.contains("src/**") && wide[0].1.message.contains("goes stale"), "{}", wide[0].1.message);
        assert_eq!(wide[0].1.rule.severity(), crate::diag::Severity::Warning);
    }

    fn shared_scope_findings(tasks: &str, files: &[&str]) -> Vec<String> {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        for f in files {
            std::fs::write(repo.join(f), "x").unwrap();
        }
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "files"]);
        let p = init(&repo, &tmp.0.join("home"), Mode::Home).unwrap();
        let c = change::new(&p, "T", "main", true).unwrap();
        std::fs::write(c.path.join("tasks.md"), tasks).unwrap();
        let r = &check(&p, &repo, None).unwrap()[0];
        r.findings
            .iter()
            .filter(|(_, d)| d.rule == crate::diag::Rule::SharedScope)
            .map(|(_, d)| {
                assert_eq!(d.rule.severity(), crate::diag::Severity::Warning);
                assert_eq!(d.rule.code(), "shared-scope");
                d.message.clone()
            })
            .collect()
    }

    #[test]
    fn check_warns_about_a_shared_scope_names_both_tasks() {
        let tasks = "- TSK-new1 A\n  verify: true\n  files: src/a.rs\n- TSK-new2 B\n  verify: true\n  files: src/a.rs\n";
        let msgs = shared_scope_findings(tasks, &["src/a.rs"]);
        assert_eq!(msgs.len(), 1, "{msgs:?}");
        assert!(msgs[0].contains("TSK-new1") && msgs[0].contains("TSK-new2") && msgs[0].contains("src/a.rs"), "{}", msgs[0]);
    }

    #[test]
    fn check_warns_about_a_shared_scope_cuts_after_three_files() {
        let tasks = "- TSK-new1 A\n  verify: true\n  files: src/*.rs\n- TSK-new2 B\n  verify: true\n  files: src/**\n";
        let names = ["a", "b", "c", "d", "e"].map(|n| format!("src/{n}.rs"));
        let files: Vec<&str> = names.iter().map(String::as_str).collect();
        let msgs = shared_scope_findings(tasks, &files);
        assert_eq!(msgs.len(), 1, "{msgs:?}");
        assert!(msgs[0].contains("src/a.rs") && msgs[0].contains("src/c.rs"), "{}", msgs[0]);
        assert!(!msgs[0].contains("src/d.rs") && msgs[0].contains("2 more"), "{}", msgs[0]);
    }

    #[test]
    fn check_warns_about_a_shared_scope_stays_quiet_for_separate_scopes() {
        let tasks = "- TSK-new1 A\n  verify: true\n  files: src/a.rs\n- TSK-new2 B\n  verify: true\n  files: src/b.rs\n";
        let msgs = shared_scope_findings(tasks, &["src/a.rs", "src/b.rs"]);
        assert!(msgs.is_empty(), "{msgs:?}");
    }

    #[test]
    fn blank_why_is_an_error_that_names_why() {
        let diags = blank_sections("branch: b\n## Why\n## What changes\nx\n## Out of scope\nNone.\n");
        let msgs: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(msgs, ["proposal section `## Why` is empty"]);
        assert_eq!(diags[0].line, 2);
        assert_eq!(diags[0].rule, crate::diag::Rule::Structure);
    }

    #[test]
    fn blank_out_of_scope_with_one_word_is_no_error() {
        let diags = blank_sections("branch: b\n## Why\nx\n## What changes\nx\n## Out of scope\nNone.\n");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
