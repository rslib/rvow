use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use rvow_core::change::{self, Stage};
use rvow_core::diag::Severity;
use rvow_core::error::{Error, Result};
use rvow_core::gate::{self, Options};
use rvow_core::git;
use rvow_core::lint::Finding;
use rvow_core::proof::{self, State};
use rvow_core::store::{self, Mode, Project};
use rvow_core::vow;

/// Hold a coding agent to an accepted plan.
#[derive(Parser)]
#[command(name = "rvow", version)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create the rvow tree for this repository.
    Init {
        /// Keep the tree in rvow/ inside the repository instead of the home store.
        #[arg(long)]
        in_repo: bool,
    },
    /// Create a change in backlog/.
    New {
        /// The change title.
        #[arg(required = true, num_args = 1..)]
        title: Vec<String>,
        /// A small change: proposal and tasks, no spec deltas.
        #[arg(long)]
        small: bool,
    },
    /// Check the specs and every open change.
    Validate,
    /// Run the baseline and record the vow (user only).
    Accept {
        /// The change id or a unique prefix of it.
        change: String,
        /// Record the vow without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Run a task's stored verify command and record the proof.
    Prove {
        /// The task id or a unique prefix of it.
        task: String,
    },
    /// Show the state of each task on this branch.
    Status {
        /// Show every active change, not only this branch's.
        #[arg(long)]
        all: bool,
    },
    /// Check the active changes of this branch.
    Gate {
        /// Fail on stale and flaky proofs too.
        #[arg(long)]
        strict: bool,
        /// Run every verify command again in a clean worktree of HEAD.
        #[arg(long)]
        rerun: bool,
    },
}

fn main() -> ExitCode {
    #[cfg(unix)]
    // SAFETY: restores the default SIGPIPE action before any thread starts, so
    // `rvow ... | head` ends quietly instead of panicking on a closed pipe.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    let result = match cli.cmd {
        Cmd::Init { in_repo } => init(in_repo),
        Cmd::New { title, small } => new(&title.join(" "), small),
        Cmd::Validate => validate(),
        Cmd::Accept { change, yes } => accept(&change, yes),
        Cmd::Prove { task } => prove(&task),
        Cmd::Status { all } => status(all),
        Cmd::Gate { strict, rerun } => gate(Options { strict, rerun }),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            println!("error[{}]: {}", e.code, e.message);
            println!("  fix: {}", e.fix);
            ExitCode::FAILURE
        }
    }
}

fn cwd() -> Result<PathBuf> {
    std::env::current_dir().map_err(|e| Error::io(Path::new("."), e))
}

fn project(home: &Path) -> Result<Project> {
    store::find(&cwd()?, home)?.ok_or_else(|| {
        Error::new(
            "no-project",
            "this repository has no rvow tree",
            "run rvow init",
        )
    })
}

fn init(in_repo: bool) -> Result<bool> {
    let mode = if in_repo { Mode::Repo } else { Mode::Home };
    let p = store::init(&cwd()?, &store::home_dir()?, mode)?;
    println!("created {}", p.root.display());
    Ok(true)
}

fn new(title: &str, small: bool) -> Result<bool> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let branch = git::branch(&cwd()?)?.ok_or_else(|| {
        Error::new(
            "detached",
            "HEAD is detached, so the change has no branch",
            "check out a branch first",
        )
    })?;
    let (changes, _) = change::list(&p)?;
    let busy = changes
        .iter()
        .filter(|c| c.stage == Stage::Active)
        .any(|c| {
            std::fs::read_to_string(c.path.join(change::PROPOSAL))
                .is_ok_and(|t| t.lines().any(|l| l.trim() == format!("branch: {branch}")))
        });
    let c = change::new(&p, title, &branch, small)?;
    println!("created {} at {}", c.id, c.path.display());
    if busy {
        println!(
            "hint: {branch} already has an active change; one change per branch is the standard"
        );
    }
    Ok(true)
}

fn print_findings(prefix: &str, findings: &[Finding]) -> usize {
    let mut errors = 0;
    for (path, d) in findings {
        let level = match d.severity() {
            Severity::Error => {
                errors += 1;
                "error"
            }
            Severity::Warning => "warning",
        };
        println!(
            "{prefix}{path}:{}: {level}[{}]: {}",
            d.line,
            d.rule.code(),
            d.message
        );
        if d.severity() == Severity::Error {
            println!("  fix: {}", d.rule.fix());
        }
    }
    errors
}

fn validate() -> Result<bool> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let (living, findings) = change::load_living(&p)?;
    let mut errors = print_findings("", &findings);
    let (changes, stray) = change::list(&p)?;
    for path in stray {
        errors += 1;
        println!(
            "{}: error[stray]: not a change folder name, expected CHG-xxxxxx-<slug>",
            path.strip_prefix(&p.root).unwrap_or(&path).display()
        );
    }
    let mut count = 0;
    for c in changes
        .iter()
        .filter(|c| matches!(c.stage, Stage::Backlog | Stage::Active))
    {
        count += 1;
        let loaded = change::load(&c.path, &living)?;
        let prefix = format!(
            "{}/",
            c.path.strip_prefix(&p.root).unwrap_or(&c.path).display()
        );
        errors += print_findings(&prefix, &loaded.findings);
    }
    println!(
        "checked {} specs and {count} open changes: {errors} errors",
        living.len()
    );
    Ok(errors == 0)
}

fn confirm(question: &str) -> Result<bool> {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        return Err(Error::new(
            "no-tty",
            "accept needs a confirmation and stdin is not a terminal",
            "run it again with --yes after reading the summary",
        ));
    }
    print!("{question} [y/N] ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    stdin
        .lock()
        .read_line(&mut line)
        .map_err(|e| Error::io(Path::new("stdin"), e))?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes"))
}

fn accept(target: &str, yes: bool) -> Result<bool> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let c = change::resolve(&p, target)?;
    let prepared = vow::prepare(&p, &home, c, &cwd()?, &mut std::io::stderr())?;
    println!(
        "{} on {}: {} tasks, base {}",
        prepared.change.id,
        prepared.branch,
        prepared.loaded.tasks.len(),
        &prepared.base_commit[..12.min(prepared.base_commit.len())]
    );
    for b in &prepared.baseline {
        let tests = b
            .outcome
            .tests_run
            .map_or("unknown".to_string(), |n| n.to_string());
        let exit = b
            .outcome
            .exit
            .map_or("signal".to_string(), |n| n.to_string());
        println!("  {}: baseline exit {exit}, {tests} tests", b.task);
    }
    if !yes && !confirm("record the vow?")? {
        println!("not accepted");
        return Ok(false);
    }
    let vowed = vow::commit(&p, prepared)?;
    for (from, to) in &vowed.minted {
        println!("  {from} -> {to}");
    }
    println!(
        "accepted {} at {} (vow {})",
        vowed.id,
        vowed.path.display(),
        vowed.event
    );
    Ok(true)
}

fn prove(task: &str) -> Result<bool> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let r = proof::prove(&p, &home, &cwd()?, task, &mut std::io::stderr())?;
    let o = &r.outcome;
    let tests = o.tests_run.map_or("unknown".to_string(), |n| n.to_string());
    let verdict = if r.proved {
        "proved".to_string()
    } else if o.timed_out {
        "failed: timed out".to_string()
    } else if o.passed() {
        "failed: the command passed but ran 0 tests".to_string()
    } else {
        format!(
            "failed: exit {}",
            o.exit.map_or("signal".into(), |e| e.to_string())
        )
    };
    println!(
        "{} {verdict} ({tests} tests, {} ms, event {})",
        r.task, o.duration_ms, r.event
    );
    for f in &r.lfs_pointers {
        println!("warning: {f} is a Git LFS pointer, run git lfs pull");
    }
    if r.looping {
        println!(
            "hint: the same failure {} times in a row; if the task cannot pass as written, run rvow block {} --reason \"...\"",
            proof::LOOP_RUNS,
            r.task
        );
    }
    match &r.next {
        Some(next) => print!("next: {next}"),
        None => println!("next: every task on this branch is proved or blocked"),
    }
    Ok(r.proved)
}

fn status(all: bool) -> Result<bool> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let changes = proof::status(&p, &home, &cwd()?, all)?;
    if changes.is_empty() {
        println!("no active change on this branch");
        return Ok(true);
    }
    for c in &changes {
        println!("{} on {}", c.id, c.branch);
        for t in &c.tasks {
            let state = match &t.state {
                State::Stale(why) => format!("stale ({})", why.join(", ")),
                s => s.name().to_string(),
            };
            println!("  {:<10} {:<20} {}", t.task, state, t.title);
        }
        for problem in &c.problems {
            println!("  error: {problem}");
        }
    }
    Ok(true)
}

fn gate(opts: Options) -> Result<bool> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let r = gate::gate(&p, &home, &cwd()?, opts, &mut std::io::stderr())?;
    for (task, ms) in &r.timings {
        println!("rerun {task}: {ms} ms");
    }
    for w in &r.warnings {
        println!("warning: {w}");
    }
    for f in &r.failures {
        println!("fail: {f}");
    }
    if r.passed() {
        println!("gate passed ({} active changes on this branch)", r.changes);
    } else {
        println!("gate failed: {} problems", r.failures.len());
    }
    Ok(r.passed())
}
