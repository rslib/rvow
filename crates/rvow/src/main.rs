use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use rvow_core::amend;
use rvow_core::brief;
use rvow_core::change::{self, Stage};
use rvow_core::diag::Severity;
use rvow_core::error::{Error, Result};
use rvow_core::gate::{self, Options};
use rvow_core::git;
use rvow_core::hook;
use rvow_core::lint::Finding;
use rvow_core::notes;
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
    /// List every change with its stage, branch and title.
    Ls,
    /// Print where a change, task or requirement lives.
    Path {
        /// A CHG, TSK or REQ id or a unique prefix of it.
        id: String,
    },
    /// Mint real ids for the placeholders of an active change.
    Ids {
        /// The change id or a unique prefix of it, default every active change on this branch.
        change: Option<String>,
    },
    /// Print the short generated brief for this branch.
    Brief,
    /// Answer a harness hook: reads the hook JSON on stdin, prints the response.
    Hook {
        #[arg(value_enum)]
        harness: Harness,
        #[arg(value_enum)]
        event: HookEvent,
    },
    /// Record that a task cannot be finished as written.
    Block {
        /// The task id or a unique prefix of it.
        task: String,
        /// What stops the task.
        #[arg(long)]
        reason: String,
    },
    /// Append a decision, surprise or handoff note to this branch's change.
    Note {
        #[arg(value_enum)]
        kind: NoteArg,
        /// The note text.
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
    },
    /// Record the current plan edits as an amendment for the user to confirm.
    Amend {
        /// Why the plan changed.
        #[arg(long)]
        reason: String,
        /// The change id; default: every active change on this branch.
        #[arg(long)]
        change: Option<String>,
    },
    /// List pending amendments, or accept or reject them (user only).
    #[command(group(ArgGroup::new("decision").args(["accept", "reject"])))]
    Confirm {
        /// Amendment ids or unique prefixes.
        ids: Vec<String>,
        /// Accept the named amendments.
        #[arg(long)]
        accept: bool,
        /// Reject the named amendments.
        #[arg(long)]
        reject: bool,
        /// Decide every pending amendment; pass the digest that the list printed.
        #[arg(long, value_name = "DIGEST", conflicts_with = "ids")]
        all: Option<String>,
    },
    /// Run a task's stored verify command and record the proof.
    #[command(group(ArgGroup::new("what").required(true).args(["task", "stale"])))]
    Prove {
        /// The task id or a unique prefix of it.
        task: Option<String>,
        /// Prove every stale task on this branch.
        #[arg(long)]
        stale: bool,
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
        Cmd::Ls => ls(),
        Cmd::Path { id } => path(&id),
        Cmd::Ids { change } => ids(change.as_deref()),
        Cmd::Brief => brief(),
        Cmd::Hook { harness, event } => return hook(harness, event),
        Cmd::Block { task, reason } => block(&task, &reason),
        Cmd::Note { kind, text } => note(kind, &text.join(" ")),
        Cmd::Amend { reason, change } => amend(&reason, change.as_deref()),
        Cmd::Confirm {
            ids,
            accept,
            reject,
            all,
        } => confirm(&ids, accept, reject, all.as_deref()),
        Cmd::Prove { task: Some(t), .. } => prove(&t),
        Cmd::Prove { .. } => prove_stale(),
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

fn ask_yes(question: &str) -> Result<bool> {
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
    if !yes && !ask_yes("record the vow?")? {
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

fn print_result(r: &proof::Proved) {
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
}

fn prove(task: &str) -> Result<bool> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let r = proof::prove(&p, &home, &cwd()?, task, &mut std::io::stderr())?;
    print_result(&r);
    match &r.next {
        Some(next) => print!("next: {next}"),
        None => println!("next: every task on this branch is proved or blocked"),
    }
    Ok(r.proved)
}

fn prove_stale() -> Result<bool> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let cwd = cwd()?;
    let stale = proof::stale_tasks(&p, &home, &cwd)?;
    if stale.is_empty() {
        println!("no stale tasks");
        return Ok(true);
    }
    let mut all = true;
    for task in &stale {
        let r = proof::prove(&p, &home, &cwd, task, &mut std::io::stderr())?;
        print_result(&r);
        all &= r.proved;
    }
    Ok(all)
}

fn ls() -> Result<bool> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    for s in change::summaries(&p)? {
        println!("{} {:<10} {} {}", s.id, s.stage.name(), s.branch, s.title);
    }
    Ok(true)
}

fn path(id: &str) -> Result<bool> {
    let home = store::home_dir()?;
    println!("{}", change::locate(&project(&home)?, id)?);
    Ok(true)
}

fn ids(target: Option<&str>) -> Result<bool> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let minted = vow::mint_active(&p, &cwd()?, target)?;
    for (from, to) in &minted {
        println!("{from} -> {to}");
    }
    if minted.is_empty() {
        println!("no placeholders");
    }
    Ok(true)
}

fn brief() -> Result<bool> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    print!("{}", brief::brief(&p, &home, &cwd()?)?);
    Ok(true)
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
        for d in &c.drift {
            println!("  drift {}: {}", d.class.name(), d.what);
        }
        for a in &c.pending {
            println!("  pending {}: {}", a.id, a.reason);
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

fn amend(reason: &str, target: Option<&str>) -> Result<bool> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    for a in amend::amend(&p, &home, &cwd()?, target, reason)? {
        println!("{} for {}:", a.id, a.change);
        for i in &a.items {
            println!("  {}: {}", i.class.name(), i.what);
        }
    }
    println!("ask the user to run: ! rvow confirm");
    Ok(true)
}

fn confirm(ids: &[String], accept: bool, reject: bool, all: Option<&str>) -> Result<bool> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    if !accept && !reject {
        if !ids.is_empty() || all.is_some() {
            return Err(Error::new(
                "decision",
                "say what to do with the amendments",
                "add --accept or --reject",
            ));
        }
        let (living, _) = change::load_living(&p)?;
        let pending = amend::pending(&proof::active(&p, &living)?);
        if pending.is_empty() {
            println!("no pending amendments");
            return Ok(true);
        }
        for a in &pending {
            println!("{} for {}", a.id, a.change);
            for i in &a.items {
                let class = i.get("class").and_then(|v| v.as_str()).unwrap_or("");
                let what = i.get("what").and_then(|v| v.as_str()).unwrap_or("");
                println!("  {class}: {what}");
                for (label, key) in [("before", "before"), ("after", "after")] {
                    if let Some(v) = i.get(key).and_then(|v| v.as_str()) {
                        for line in v.lines() {
                            println!("    {label}: {line}");
                        }
                    }
                }
            }
            println!("  reason (from the agent): {}", a.reason);
        }
        println!("digest {}", amend::digest(&pending));
        println!("decide with: rvow confirm <id>... --accept|--reject, or --accept --all <digest>");
        return Ok(true);
    }
    let decision = if accept {
        amend::Decision::Accept
    } else {
        amend::Decision::Reject
    };
    for id in amend::confirm(&p, ids, decision, all)? {
        println!("{id}: {}", if accept { "accepted" } else { "rejected" });
    }
    Ok(true)
}

#[derive(Clone, Copy, ValueEnum)]
enum NoteArg {
    Decision,
    Surprise,
    Handoff,
}

fn block(task: &str, reason: &str) -> Result<bool> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let (change, task) = notes::block(&p, task, reason)?;
    println!("{task} blocked in {change}");
    println!("the gate refuses it until the user confirms an amendment that changes or drops it");
    Ok(true)
}

fn note(kind: NoteArg, text: &str) -> Result<bool> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let kind = match kind {
        NoteArg::Decision => notes::NoteKind::Decision,
        NoteArg::Surprise => notes::NoteKind::Surprise,
        NoteArg::Handoff => notes::NoteKind::Handoff,
    };
    let (change, cut) = notes::note(&p, &cwd()?, kind, text)?;
    println!("{} noted in {change}", kind.name());
    if cut {
        println!(
            "warning: the handoff kept only its first {} lines",
            notes::HANDOFF_LINES
        );
    }
    Ok(true)
}

#[derive(Clone, Copy, ValueEnum)]
enum Harness {
    Claude,
}

#[derive(Clone, Copy, ValueEnum)]
enum HookEvent {
    PreToolUse,
    SessionStart,
    Stop,
}

/// Hooks never fail the harness: outside an rvow project they print nothing,
/// and an internal error allows the call and goes to stderr, so a bug in rvow
/// cannot lock the agent out of every repository.
fn hook(harness: Harness, event: HookEvent) -> ExitCode {
    let mut text = String::new();
    if std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).is_err() {
        return ExitCode::SUCCESS;
    }
    let input: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let run = || -> Result<Option<serde_json::Value>> {
        let home = store::home_dir()?;
        let cwd = input
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .map_or_else(cwd, Ok)?;
        let Ok(Some(p)) = store::find(&cwd, &home) else {
            return Ok(None);
        };
        match (harness, event) {
            (Harness::Claude, HookEvent::PreToolUse) => hook::claude_pre_tool_use(&p, &input),
            (Harness::Claude, HookEvent::SessionStart) => {
                hook::claude_session_start(&p, &home, &input)
            }
            (Harness::Claude, HookEvent::Stop) => hook::claude_stop(&p, &home, &input),
        }
    };
    match run() {
        Ok(Some(out)) => println!("{out}"),
        Ok(None) => {}
        Err(e) => eprintln!("rvow hook: {}: {}", e.code, e.message),
    }
    ExitCode::SUCCESS
}
