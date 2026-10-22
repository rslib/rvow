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

/// The time and duration in ms of the newest recorded proof of each verify
/// command in this project, read once for a whole `check` call.
fn last_runs(project: &Project) -> BTreeMap<String, (u64, u64)> {
    let mut out: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let Ok((dirs, _)) = change::list(project) else { return out };
    for d in &dirs {
        let Ok((events, _)) = ledger::read(&d.path.join(change::LEDGER)) else { continue };
        for e in events.iter().filter(|e| e.kind == Kind::Prove) {
            let Some(cmd) = e.data.get("verify").and_then(Value::as_str) else { continue };
            let Some(ms) = e.data.get("duration_ms").and_then(Value::as_u64) else { continue };
            if out.get(cmd).is_none_or(|(time, _)| e.time > *time) {
                out.insert(cmd.to_string(), (e.time, ms));
            }
        }
    }
    out
}

/// The duration of the newest recorded proof of `cmd` in this project, in ms.
fn last_run_ms(runs: &BTreeMap<String, (u64, u64)>, cmd: &str) -> Option<u64> {
    runs.get(cmd).map(|(_, ms)| *ms)
}

/// A warning for each task whose verify runs the full suite instead of one
/// target: it chains commands, repeats a configured check, or ran slowly the
/// last time the same command was proved in this project.
pub fn full_suite(
    runs: &BTreeMap<String, (u64, u64)>,
    config: &Config,
    tasks: &[Task],
    filled: &BTreeMap<String, String>,
) -> Vec<SlowVerify> {
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
        if let Some(ms) = last_run_ms(runs, cmd)
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

/// The tracked files of each worktree a `check` call reads, listed at most
/// once per worktree for the whole call.
#[derive(Default)]
struct Trees {
    listed: Vec<(PathBuf, Option<String>)>,
}

impl Trees {
    fn get(&mut self, top: &Path) -> Option<&str> {
        let at = match self.listed.iter().position(|(p, _)| p == top) {
            Some(at) => at,
            None => {
                let listing = git::run(top, &["ls-tree", "-r", "--name-only", "HEAD"]).ok();
                self.listed.push((top.to_path_buf(), listing));
                self.listed.len() - 1
            }
        };
        self.listed[at].1.as_deref()
    }
}

/// A warning for each task whose scope covers a large part of the repository: its proof goes
/// stale whenever any of those files changes, so the plan should name what the task changes.
fn wide_scopes(trees: &mut Trees, top: &Path, tasks: &[Task]) -> Vec<Finding> {
    let Some(listing) = trees.get(top) else {
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

/// A task title longer than this many words is a spec, not a title.
pub const MAX_TITLE_WORDS: usize = 60;
/// A `depends_on` chain of this many tasks makes one edit stale for all of them.
pub const CHAIN_TASKS: usize = 3;
/// A file in the scope of this many tasks makes all of their proofs stale at once.
pub const HOT_TASKS: usize = 3;

const NINJA_VALUE_FLAGS: [&str; 5] = ["-C", "-j", "-k", "-l", "-f"];
const CTEST_SELECTORS: [&str; 6] = ["-R", "-L", "-E", "-I", "--tests-regex", "--label-regex"];

/// Whether a `ninja` or `make` segment names a target: a word that is not a flag, a flag value or a variable.
fn names_target(words: &[&str]) -> bool {
    let mut skip = false;
    words.iter().skip(1).any(|w| {
        if std::mem::take(&mut skip) {
            return false;
        }
        skip = NINJA_VALUE_FLAGS.contains(w);
        !w.starts_with('-') && !w.contains('=')
    })
}

/// Why a verify command builds or runs everything, or `None` when it names a target.
fn broad_verify(cmd: &str) -> Option<&'static str> {
    cmd.replace("&&", ";").split([';', '|']).find_map(|segment| {
        let words: Vec<&str> = segment.split_whitespace().collect();
        match *words.first()? {
            "cmake" if words.contains(&"--build") => {
                let targeted = words.iter().any(|w| matches!(*w, "--target" | "-t") || w.starts_with("--target="));
                (!targeted).then_some("builds every target")
            }
            "ninja" | "make" => (!names_target(&words)).then_some("builds every target"),
            "ctest" => {
                let selected = words.iter().any(|w| CTEST_SELECTORS.iter().any(|f| w.starts_with(f)) || *w == "--rerun-failed");
                (!selected).then_some("runs every test")
            }
            _ => None,
        }
    })
}

/// A warning for each task whose title is a spec, and for each whose verify builds or runs everything.
fn task_sizes(tasks: &[Task], filled: &BTreeMap<String, String>) -> Vec<Finding> {
    let mut out = Vec::new();
    for t in tasks {
        let words = t.title.split_whitespace().count();
        if words > MAX_TITLE_WORDS {
            let message = format!("{} has a title of {words} words, more than {MAX_TITLE_WORDS}", t.id);
            out.push((change::TASKS.to_string(), crate::diag::Diagnostic::new(crate::diag::Rule::LongTask, t.line, message)));
        }
        let Some(cmd) = filled.get(&t.id.to_string()) else { continue };
        if let Some(why) = broad_verify(cmd) {
            let message = format!("{} verify ({cmd}) {why}; every proof and every gate run pays for it", t.id);
            out.push((change::TASKS.to_string(), crate::diag::Diagnostic::new(crate::diag::Rule::BroadVerify, t.line, message)));
        }
    }
    out
}

/// The longest `depends_on` chain that ends at the task at `at`, oldest task first.
fn chain_to(tasks: &[Task], at: usize, seen: &mut Vec<usize>) -> Vec<usize> {
    seen.push(at);
    let mut best: Vec<usize> = Vec::new();
    for d in &tasks[at].depends_on {
        let Some(next) = tasks.iter().position(|t| t.id == *d) else { continue };
        if seen.contains(&next) {
            continue;
        }
        let chain = chain_to(tasks, next, seen);
        if chain.len() > best.len() {
            best = chain;
        }
    }
    seen.pop();
    best.push(at);
    best
}

/// A warning for each task that ends a `depends_on` chain of `CHAIN_TASKS` or more tasks.
fn depends_chains(tasks: &[Task]) -> Vec<Finding> {
    let mut out = Vec::new();
    for (at, t) in tasks.iter().enumerate().filter(|(_, t)| !tasks.iter().any(|o| o.depends_on.contains(&t.id))) {
        let chain = chain_to(tasks, at, &mut Vec::new());
        if chain.len() < CHAIN_TASKS {
            continue;
        }
        let path = chain.iter().map(|&i| tasks[i].id.to_string()).collect::<Vec<_>>().join(" -> ");
        let message = format!(
            "{} ends a depends_on chain of {} tasks ({path}); one edit in any of their files makes the chain stale",
            t.id,
            chain.len()
        );
        out.push((change::TASKS.to_string(), crate::diag::Diagnostic::new(crate::diag::Rule::DependsChain, t.line, message)));
    }
    out
}

/// One warning that names the files in the scope of `HOT_TASKS` or more tasks, most shared first.
fn hot_files(listing: &str, tasks: &[Task]) -> Vec<Finding> {
    let sets: Vec<_> = tasks.iter().map(|t| crate::fingerprint::globset(&t.files).ok()).collect();
    let mut hot: Vec<(usize, &str, usize)> = Vec::new();
    for file in listing.lines() {
        let hits: Vec<usize> =
            sets.iter().enumerate().filter(|(_, s)| s.as_ref().is_some_and(|s| s.is_match(file))).map(|(i, _)| i).collect();
        if hits.len() >= HOT_TASKS {
            hot.push((hits.len(), file, hits[0]));
        }
    }
    hot.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    let Some(&(_, _, first)) = hot.first() else { return Vec::new() };
    let listed = hot.iter().take(SHARED_LISTED).map(|(n, f, _)| format!("{f} ({n} tasks)")).collect::<Vec<_>>().join(", ");
    let rest = hot.len().saturating_sub(SHARED_LISTED);
    let more = if rest > 0 { format!(" and {rest} more") } else { String::new() };
    let noun = if hot.len() == 1 { "file is" } else { "files are" };
    let message = format!(
        "{} {noun} in the scope of {HOT_TASKS} or more tasks, such as {listed}{more}; one edit to one of them makes each of those proofs stale",
        hot.len()
    );
    vec![(change::TASKS.to_string(), crate::diag::Diagnostic::new(crate::diag::Rule::HotFile, tasks[first].line, message))]
}

/// `hot_files` over the tracked files of `top`.
fn hot_scopes(trees: &mut Trees, top: &Path, tasks: &[Task]) -> Vec<Finding> {
    match trees.get(top) {
        Some(listing) => hot_files(listing, tasks),
        None => Vec::new(),
    }
}

/// The most shared files a `shared-scope` warning lists by name.
const SHARED_LISTED: usize = 3;

/// A warning for each pair of tasks whose `files:` match a common tracked file: the proof
/// of both goes stale when that file changes.
fn shared_scopes(trees: &mut Trees, top: &Path, tasks: &[Task]) -> Vec<Finding> {
    if tasks.len() < 2 {
        return Vec::new();
    }
    let Some(listing) = trees.get(top) else {
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

/// A warning for each task whose check proves nothing: a stub command.
fn weak_checks(tasks: &[Task], filled: &BTreeMap<String, String>) -> Vec<Finding> {
    let mut out = Vec::new();
    for t in tasks {
        let Some(cmd) = filled.get(&t.id.to_string()) else {
            continue;
        };
        let cmd = cmd.as_str();
        if !trivial_verify(cmd) {
            continue;
        }
        let message = format!(
            "{} has a trivial verify ({cmd}); replace it with the command that fails today, so the check cannot pass while the work is missing",
            t.id
        );
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

fn report(
    project: &Project,
    config: &Config,
    runs: &BTreeMap<String, (u64, u64)>,
    trees: &mut Trees,
    change: ChangeDir,
    loaded: Loaded,
    top: &Path,
) -> Report {
    let runners = config::runners(project).unwrap_or_default();
    let filled = filled_commands(&runners, &loaded.tasks);
    let slow = full_suite(runs, config, &loaded.tasks, &filled);
    let blanks = proposal_blanks(&loaded);
    let mut findings = loaded.findings;
    findings.extend(blanks);
    findings.extend(crate::lint::runner_checks(&runners, &loaded.tasks));
    findings.extend(crate::lint::configure_warnings(&runners, &loaded.tasks));
    findings.extend(wide_scopes(trees, top, &loaded.tasks));
    findings.extend(shared_scopes(trees, top, &loaded.tasks));
    findings.extend(hot_scopes(trees, top, &loaded.tasks));
    findings.extend(task_sizes(&loaded.tasks, &filled));
    findings.extend(depends_chains(&loaded.tasks));
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
    let runs = last_runs(project);
    let mut trees = Trees::default();
    let mut out = Vec::new();
    for d in dirs {
        let loaded = change::load(&d.path, &living)?;
        let change_top = if target.is_some() { change_top(&top, &loaded) } else { top.clone() };
        let mut r = report(project, &config, &runs, &mut trees, d, loaded, &change_top);
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

    fn parsed(src: &str) -> Vec<Task> {
        let (tasks, diags) = crate::tasks::parse_tasks(src);
        assert!(diags.is_empty(), "{diags:?}");
        tasks
    }

    fn codes(found: &[Finding]) -> Vec<&'static str> {
        found.iter().map(|(_, d)| d.rule.code()).collect()
    }

    #[test]
    fn task_size_warns_about_a_title_over_the_word_limit() {
        let long = vec!["word"; MAX_TITLE_WORDS + 1].join(" ");
        let ok = vec!["word"; MAX_TITLE_WORDS].join(" ");
        let tasks = parsed(&format!(
            "- TSK-new1 {long}\n    verify: cargo test a\n    files: a\n- TSK-new2 {ok}\n    verify: cargo test b\n    files: b\n"
        ));
        let found = task_sizes(&tasks, &BTreeMap::new());
        assert_eq!(codes(&found), ["long-task"]);
        assert!(found[0].1.message.contains("TSK-new1"), "{}", found[0].1.message);
    }

    #[test]
    fn task_size_warns_about_a_verify_that_builds_or_runs_everything() {
        for cmd in [
            "cmake --build --preset tests && ctest --preset tests -R '^a/'",
            "cmake --build build && ctest --test-dir build -R '^a$'",
            "cmake --build build --target test_a && ctest --test-dir build",
            "ninja -C build && ctest --test-dir build -R a",
            "make -j8 && ctest -R a",
        ] {
            assert!(broad_verify(cmd).is_some(), "{cmd}");
        }
        for cmd in [
            "cmake --build build --target test_a && ctest --test-dir build -R '^a$'",
            "cmake --build --preset tests --target test_a && ctest --preset tests -R '^a$'",
            "ninja -C build test_a && ctest --test-dir build -R a",
            "make test_a && ctest -R a",
            "cargo nextest run --locked a",
            "python3 scripts/check.py --self-test",
        ] {
            assert_eq!(broad_verify(cmd), None, "{cmd}");
        }
    }

    #[test]
    fn task_size_reports_a_broad_verify_on_its_task() {
        let tasks = parsed("- TSK-new1 A\n    verify: cmake --build --preset tests && ctest --preset tests -R a\n    files: a\n");
        let filled = BTreeMap::from([("TSK-new1".to_string(), "cmake --build --preset tests && ctest --preset tests -R a".to_string())]);
        let found = task_sizes(&tasks, &filled);
        assert_eq!(codes(&found), ["broad-verify"]);
        assert!(found[0].1.message.contains("TSK-new1"), "{}", found[0].1.message);
    }

    #[test]
    fn task_size_warns_once_about_a_depends_on_chain() {
        let two =
            parsed("- TSK-new1 A\n    verify: x\n    files: a\n- TSK-new2 B\n    verify: x\n    files: b\n    depends_on: TSK-new1\n");
        assert!(depends_chains(&two).is_empty());
        let four = parsed(
            "- TSK-new1 A\n    verify: x\n    files: a\n- TSK-new2 B\n    verify: x\n    files: b\n    depends_on: TSK-new1\n- TSK-new3 C\n    verify: x\n    files: c\n    depends_on: TSK-new2\n- TSK-new4 D\n    verify: x\n    files: d\n    depends_on: TSK-new3\n",
        );
        let found = depends_chains(&four);
        assert_eq!(codes(&found), ["depends-chain"]);
        assert!(found[0].1.message.contains("TSK-new4") && found[0].1.message.contains("4 tasks"), "{}", found[0].1.message);
    }

    #[test]
    fn task_size_warns_about_a_file_in_the_scope_of_three_tasks() {
        let tasks = parsed(
            "- TSK-new1 A\n    verify: x\n    files: src/**\n- TSK-new2 B\n    verify: x\n    files: src/a.rs, tests/**\n- TSK-new3 C\n    verify: x\n    files: src/a.rs\n- TSK-new4 D\n    verify: x\n    files: docs/**\n",
        );
        let found = hot_files("src/a.rs\nsrc/b.rs\ntests/t.rs\ndocs/d.md\n", &tasks);
        assert_eq!(codes(&found), ["hot-file"]);
        let message = &found[0].1.message;
        assert!(message.contains("src/a.rs") && message.contains("3 tasks"), "{message}");
        assert!(!message.contains("src/b.rs"), "a file in two tasks is not hot: {message}");
        assert!(hot_files("src/b.rs\ntests/t.rs\n", &tasks).is_empty());
    }

    #[test]
    fn a_baseline_pass_flag_draws_no_weak_verify_warning() {
        let tasks = parsed("- TSK-new1 A\n    verify: ./build/tests/test_hex64\n    files: a\n    baseline: pass\n");
        let filled = BTreeMap::from([("TSK-new1".to_string(), "./build/tests/test_hex64".to_string())]);
        assert!(weak_checks(&tasks, &filled).is_empty(), "the flag is read and ignored");
        let stub = BTreeMap::from([("TSK-new1".to_string(), "true".to_string())]);
        assert_eq!(codes(&weak_checks(&tasks, &stub)), ["weak-verify"], "a stub command still draws the warning");
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
