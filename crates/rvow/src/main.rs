use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{ArgGroup, CommandFactory, Parser, Subcommand, ValueEnum};
use rvow_core::amend;
use rvow_core::archive;
use rvow_core::brief;
use rvow_core::change::{self, Stage};
use rvow_core::diag::Severity;
use rvow_core::error::{Error, Result};
use rvow_core::gate::{self, Options};
use rvow_core::git;
use rvow_core::guide;
use rvow_core::hook;
use rvow_core::import;
use rvow_core::install;
use rvow_core::interview;
use rvow_core::lint::Finding;
use rvow_core::notes;
use rvow_core::plan;
use rvow_core::proof::{self, State};
use rvow_core::show;
use rvow_core::store::{self, Mode, Project};
use rvow_core::view;
use rvow_core::vow;

#[macro_use]
mod toon;
mod output;

use output::Reply;
use toon::{Node, Obj};

const DESCRIPTION: &str = "Hold a coding agent to an accepted plan.";

#[derive(Parser)]
#[command(name = "rvow", about = DESCRIPTION)]
struct Cli {
    /// Print TOON for agents; the default when stdout is not a terminal.
    #[arg(long, global = true, conflicts_with = "human")]
    toon: bool,
    /// Print human text; the default in a terminal.
    #[arg(long, global = true)]
    human: bool,
    /// Print the version.
    #[arg(short = 'V', long, short_alias = 'v')]
    version: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
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
        /// Add spec deltas; without it the change is small: proposal and tasks.
        #[arg(long)]
        spec: bool,
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
    /// Print one requirement, task or change in full.
    Show {
        /// A REQ, TSK or CHG id or a unique prefix of it.
        id: String,
        /// Print long text in full in TOON.
        #[arg(long)]
        full: bool,
    },
    /// Read the living specs.
    Spec {
        #[command(subcommand)]
        cmd: SpecCmd,
    },
    /// Convert an OpenSpec tree into specs and backlog changes.
    Import {
        #[command(subcommand)]
        cmd: ImportCmd,
    },
    /// Print the short guide for writing one plan artifact.
    Guide {
        #[arg(value_enum)]
        artifact: Artifact,
    },
    /// Mint real ids for the placeholders of an active change.
    Ids {
        /// The change id or a unique prefix of it, default every active change on this branch.
        change: Option<String>,
    },
    /// Print the short generated brief for this branch.
    Brief,
    /// Install the skill, the Claude Code hooks, the pi extension and the pre-push gate.
    Install {
        /// How rvow joins Claude Code: a mod (plugin), or hooks in a settings file.
        #[arg(long, value_enum, default_value = "mod")]
        claude: ClaudeMode,
        /// For --claude settings: your user settings, or this repository's .claude/settings.json.
        #[arg(long, value_enum, default_value = "user")]
        scope: Scope,
    },
    /// Report on a plan before accept: findings, checks that cannot run, open questions.
    Check {
        /// The change id; default: every open change on this branch.
        change: Option<String>,
    },
    /// List the open questions of a plan, each with its recommended answer.
    Clarify {
        /// The change id; default: every open change on this branch.
        change: Option<String>,
        /// Print the next round (at most 4) as JSON: question, options, recommended first.
        #[arg(long)]
        json: bool,
    },
    /// Record the user's answer to an open question and remove its marker.
    Answer {
        /// The question ref that rvow clarify printed, as CHG-xxxxxx:proposal.md:12.
        reference: String,
        /// The user's choice, as given.
        #[arg(required = true, num_args = 1..)]
        choice: Vec<String>,
        /// How the user answered: in a dialog, or in chat relayed by the agent.
        #[arg(long, value_enum, default_value = "chat")]
        via: ViaArg,
    },
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
        /// Ask to edit a protected test or setup file instead of recording drift.
        #[arg(long, value_name = "PATH")]
        unlock: Option<String>,
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
        /// Print this branch's view as JSON, the shape the harness UIs draw.
        #[arg(long, conflicts_with = "all")]
        json: bool,
    },
    /// Merge an active change into the living specs and move it to archive/done/.
    Archive {
        /// The change id or a unique prefix of it.
        change: String,
    },
    /// Give up a backlog or active change; the specs stay as they are.
    Drop {
        /// The change id or a unique prefix of it.
        change: String,
        /// Why the change is dropped.
        #[arg(long)]
        reason: String,
    },
    /// Replace a backlog or active change with another open change.
    Supersede {
        /// The change that is replaced.
        old: String,
        /// The open change that replaces it.
        #[arg(long)]
        by: String,
        /// Why the change is replaced.
        #[arg(long)]
        reason: String,
    },
    /// List every change that touched a requirement, oldest first.
    Trace {
        /// A REQ id or a unique prefix of it.
        id: String,
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

#[derive(Subcommand)]
enum SpecCmd {
    /// List each capability with its requirement ids and titles.
    List,
}

#[derive(Subcommand)]
enum ImportCmd {
    /// Import the living specs and open changes of an OpenSpec folder.
    Openspec {
        /// The OpenSpec folder; default: openspec/ at the repository top.
        #[arg(long)]
        from: Option<PathBuf>,
        /// Print the report and write nothing.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Artifact {
    Proposal,
    Spec,
    Tasks,
    Design,
}

fn main() -> ExitCode {
    #[cfg(unix)]
    // SAFETY: restores the default SIGPIPE action before any thread starts, so
    // `rvow ... | head` ends quietly instead of panicking on a closed pipe.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => return usage_error(e),
    };
    if cli.version {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let toon = cli.toon || !cli.human && !std::io::stdout().is_terminal();
    let Some(cmd) = cli.cmd else {
        return finish(home_view(), toon);
    };
    let toon = toon
        && !matches!(
            cmd,
            Cmd::Accept { .. } | Cmd::Confirm { .. } | Cmd::Answer { .. }
        );
    let result = match cmd {
        Cmd::Init { in_repo } => init(in_repo),
        Cmd::New { title, spec } => new(&title.join(" "), !spec),
        Cmd::Validate => validate(),
        Cmd::Accept { change, yes } => accept(&change, yes).map(Reply::done),
        Cmd::Ls => ls(),
        Cmd::Path { id } => path(&id),
        Cmd::Show { id, full } => show(&id, full),
        Cmd::Spec { cmd: SpecCmd::List } => spec_list(),
        Cmd::Import {
            cmd: ImportCmd::Openspec { from, dry_run },
        } => import_openspec(from, dry_run),
        Cmd::Guide { artifact } => Ok(guide(artifact)),
        Cmd::Ids { change } => ids(change.as_deref()),
        Cmd::Brief => brief(),
        Cmd::Hook { harness, event } => return hook(harness, event),
        Cmd::Install { claude, scope } => install(claude, scope),
        Cmd::Check { change } => check(change.as_deref()),
        Cmd::Clarify { change, json } => clarify(change.as_deref(), json),
        Cmd::Answer {
            reference,
            choice,
            via,
        } => answer(&reference, &choice.join(" "), via).map(Reply::done),
        Cmd::Block { task, reason } => block(&task, &reason),
        Cmd::Note { kind, text } => note(kind, &text.join(" ")),
        Cmd::Amend {
            reason,
            change,
            unlock,
        } => amend(&reason, change.as_deref(), unlock.as_deref()),
        Cmd::Confirm {
            ids,
            accept,
            reject,
            all,
        } => confirm(&ids, accept, reject, all.as_deref()).map(Reply::done),
        Cmd::Prove { task: Some(t), .. } => prove(&t),
        Cmd::Prove { .. } => prove_stale(),
        Cmd::Status { all, json } => status(all, json),
        Cmd::Gate { strict, rerun } => gate(Options { strict, rerun }),
        Cmd::Archive { change } => archive(&change),
        Cmd::Drop { change, reason } => drop_change(&change, &reason),
        Cmd::Supersede { old, by, reason } => supersede(&old, &by, &reason),
        Cmd::Trace { id } => trace(&id),
    };
    finish(result, toon)
}

/// Prints a result or an error and maps it to the exit code: 0 success, 1 error.
fn finish(result: Result<Reply>, toon: bool) -> ExitCode {
    match result {
        Ok(r) => {
            let ok = r.ok;
            r.print(toon);
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            if toon {
                println!("{}", output::error(&e.message, vec![e.fix]));
            } else {
                println!("error[{}]: {}", e.code, e.message);
                println!("  fix: {}", e.fix);
            }
            ExitCode::FAILURE
        }
    }
}

/// A clap parse error on stdout with exit 2, naming the valid flags or
/// subcommands of the command that failed. `--help` keeps clap's own output.
fn usage_error(e: clap::Error) -> ExitCode {
    use clap::error::ErrorKind;
    if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
        e.exit();
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let toon = has("--toon") || !has("--human") && !std::io::stdout().is_terminal();
    let mut cmd = Cli::command();
    cmd.build();
    let mut path = String::from("rvow");
    for arg in args.iter().filter(|a| !a.starts_with('-')) {
        let Some(sub) = cmd.find_subcommand(arg).cloned() else {
            break;
        };
        path.push(' ');
        path.push_str(sub.get_name());
        cmd = sub;
    }
    let no_sub = e.kind() == ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand;
    let valid = if (no_sub
        || matches!(
            e.kind(),
            ErrorKind::InvalidSubcommand | ErrorKind::MissingSubcommand
        ))
        && cmd.has_subcommands()
    {
        let subs: Vec<&str> = cmd
            .get_subcommands()
            .map(|c| c.get_name())
            .filter(|n| *n != "help")
            .collect();
        format!("valid subcommands for `{path}`: {}", subs.join(", "))
    } else {
        let flags: Vec<String> = cmd
            .get_arguments()
            .filter_map(|a| a.get_long())
            .filter(|l| !matches!(*l, "help" | "version" | "toon" | "human"))
            .map(|l| format!("--{l}"))
            .collect();
        let own = if flags.is_empty() {
            "none".to_owned()
        } else {
            flags.join(", ")
        };
        format!("valid flags for `{path}`: {own}; on every command: --toon, --human, --help")
    };
    let rendered = e.render().to_string();
    if toon {
        let head = rendered.split("\n\n").next().unwrap_or_default();
        let message = if no_sub {
            format!("`{path}` needs a subcommand")
        } else {
            let words: Vec<&str> = head.lines().map(str::trim).collect();
            words.join(" ").trim_start_matches("error: ").to_owned()
        };
        let mut help: Vec<String> = rendered
            .lines()
            .filter_map(|l| l.trim().strip_prefix("tip: "))
            .filter(|tip| !tip.starts_with("to pass "))
            .map(str::to_owned)
            .collect();
        help.push(cmd.render_usage().to_string().replace("Usage: ", "usage: "));
        help.push(valid);
        println!("{}", output::error(&message, help));
    } else {
        println!("{}", rendered.trim_end());
        println!("{valid}");
    }
    ExitCode::from(2)
}

/// The path with the home directory written as `~`.
fn tilde(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home.as_deref().and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) => Path::new("~").join(rest).display().to_string(),
        None => path.display().to_string(),
    }
}

/// The current branch for empty-state text, or `detached HEAD`.
fn branch_label() -> String {
    cwd()
        .and_then(|c| git::branch(&c))
        .ok()
        .flatten()
        .unwrap_or_else(|| "detached HEAD".to_owned())
}

fn home_view() -> Result<Reply> {
    let mut r = Reply::new();
    let bin = std::env::current_exe().map_or_else(|_| "rvow".to_owned(), |p| tilde(&p));
    r.set("bin", bin.as_str());
    r.set("description", DESCRIPTION);
    r.line(format!("rvow: {DESCRIPTION}"));
    r.line(format!("bin: {bin}"));
    let home = store::home_dir()?;
    let cwd = cwd()?;
    let Ok(Some(p)) = store::find(&cwd, &home) else {
        r.set("state", "no rvow project in this repository");
        r.line("no rvow project in this repository; run rvow init (it writes to the rvow home, not the repository)");
        r.help("Run `rvow init` to create this repository's rvow tree in the rvow home; the repository stays untouched");
        r.help("Run `rvow new \"<title>\"` after init to plan a change");
        return Ok(r);
    };
    let Some(v) = view::view(&p, &home, &cwd)? else {
        let b = branch_label();
        r.set("changes", format!("0 active or waiting on branch {b}"));
        r.line(format!("no active change or plan on {b}"));
        r.help("Run `rvow new \"<title>\"` to plan a change");
        r.help("Run `rvow ls` to list every change");
        return Ok(r);
    };
    let s = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_owned();
    r.set("branch", s(&v["branch"]));
    r.line(format!("branch {}", s(&v["branch"])));
    let empty = Vec::new();
    let list = |v: &serde_json::Value| v.as_array().unwrap_or(&empty).clone();
    let mut rows = Vec::new();
    for c in list(&v["changes"]) {
        let tasks = list(&c["tasks"]);
        let proved = tasks.iter().filter(|t| t["state"] == "proved").count();
        let count = format!("{proved}/{} proved", tasks.len());
        r.line(format!(
            "  {} {} {count}: {}",
            s(&c["id"]),
            s(&c["stage"]),
            s(&c["title"])
        ));
        rows.push(obj! {
            "id" => s(&c["id"]),
            "title" => s(&c["title"]),
            "stage" => s(&c["stage"]),
            "tasks" => count,
        });
    }
    r.table("changes", rows, "0 active");
    let plans = list(&v["plans"]);
    for p in &plans {
        r.line(format!(
            "  {} waits for accept: {}",
            s(&p["id"]),
            s(&p["title"])
        ));
        r.help(format!(
            "Run `rvow check {}` to see if the plan is ready",
            s(&p["id"])
        ));
    }
    if !plans.is_empty() {
        r.set("plans", Node::from(serde_json::Value::Array(plans)));
    }
    let n = &v["needs_you"];
    r.set("needs_you", Node::from(n.clone()));
    r.line(format!(
        "needs you: {} amendments, {} questions, {} blocked, {} plans",
        n["amendments"], n["questions"], n["blocked"], n["plans"]
    ));
    let next = &v["next"];
    if next.is_object() {
        r.set(
            "next",
            Node::Obj(obj! {
                "task" => s(&next["task"]),
                "title" => s(&next["title"]),
                "state" => s(&next["state"]),
            }),
        );
        r.line(format!(
            "next: {} {} ({})",
            s(&next["task"]),
            s(&next["title"]),
            s(&next["state"])
        ));
        r.help(format!(
            "Run `rvow prove {}` after the work to record its proof",
            s(&next["task"])
        ));
    }
    r.help("Run `rvow status` for drift and pending amendments");
    Ok(r)
}

fn cwd() -> Result<PathBuf> {
    std::env::current_dir().map_err(|e| Error::io(Path::new("."), e))
}

fn project(home: &Path) -> Result<Project> {
    store::find(&cwd()?, home)?.ok_or_else(|| {
        Error::new(
            "no-project",
            "this repository has no rvow tree",
            "run rvow init; it writes to the rvow home, not the repository",
        )
    })
}

fn init(in_repo: bool) -> Result<Reply> {
    let mode = if in_repo { Mode::Repo } else { Mode::Home };
    let p = store::init(&cwd()?, &store::home_dir()?, mode)?;
    let mut r = Reply::new();
    r.line(format!("created {}", p.root.display()));
    r.set("created", p.root.display().to_string());
    r.help("Run `rvow new \"<title>\"` to plan a change");
    Ok(r)
}

fn current_branch() -> Result<String> {
    git::branch(&cwd()?)?.ok_or_else(|| {
        Error::new(
            "detached",
            "HEAD is detached, so the change has no branch",
            "check out a branch first",
        )
    })
}

fn import_openspec(from: Option<PathBuf>, dry_run: bool) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let from = match from {
        Some(dir) => dir,
        None => git::toplevel(&cwd()?)?.join("openspec"),
    };
    let report = import::openspec(&p, &from, &current_branch()?, dry_run)?;
    Ok(Reply::lines(report.to_string()))
}

fn new(title: &str, small: bool) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let branch = current_branch()?;
    let (changes, _) = change::list(&p)?;
    let busy = changes
        .iter()
        .filter(|c| c.stage == Stage::Active)
        .any(|c| {
            std::fs::read_to_string(c.path.join(change::PROPOSAL))
                .is_ok_and(|t| t.lines().any(|l| l.trim() == format!("branch: {branch}")))
        });
    let c = change::new(&p, title, &branch, small)?;
    let mut r = Reply::new();
    r.line(format!("created {} at {}", c.id, c.path.display()));
    r.set("created", c.id.to_string());
    r.set("path", c.path.display().to_string());
    if busy {
        let hint =
            format!("{branch} already has an active change; one change per branch is the standard");
        r.line(format!("hint: {hint}"));
        r.set("warning", hint);
    }
    r.help("Run `rvow guide proposal` for how to write the proposal");
    r.help(format!(
        "Run `rvow check {}` when the plan is written",
        c.id
    ));
    Ok(r)
}

/// One finding, stray folder or missing program, in the same shape for
/// `validate` and `check`.
struct Problem {
    at: String,
    level: &'static str,
    code: &'static str,
    message: String,
    fix: &'static str,
}

impl Problem {
    fn add(&self, r: &mut Reply, indent: &str) {
        r.line(format!(
            "{indent}{}: {}[{}]: {}",
            self.at, self.level, self.code, self.message
        ));
        if !self.fix.is_empty() {
            r.line(format!("{indent}  fix: {}", self.fix));
        }
    }

    fn row(&self) -> Obj {
        obj! {
            "at" => self.at,
            "level" => self.level,
            "code" => self.code,
            "message" => self.message,
            "fix" => self.fix,
        }
    }
}

fn problems(prefix: &str, findings: &[Finding]) -> Vec<Problem> {
    findings
        .iter()
        .map(|(path, d)| {
            let error = d.severity() == Severity::Error;
            Problem {
                at: format!("{prefix}{path}:{}", d.line),
                level: if error { "error" } else { "warning" },
                code: d.rule.code(),
                message: d.message.clone(),
                fix: if error { d.rule.fix() } else { "" },
            }
        })
        .collect()
}

fn validate() -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let (living, findings) = change::load_living(&p)?;
    let mut all = problems("", &findings);
    let (changes, stray) = change::list(&p)?;
    for path in stray {
        all.push(Problem {
            at: path
                .strip_prefix(&p.root)
                .unwrap_or(&path)
                .display()
                .to_string(),
            level: "error",
            code: "stray",
            message: "not a change folder name, expected CHG-xxxxxx-<slug>".to_owned(),
            fix: "",
        });
    }
    let mut count = 0usize;
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
        all.extend(problems(&prefix, &loaded.findings));
    }
    let errors = all.iter().filter(|f| f.level == "error").count();
    let mut r = Reply::new();
    for f in &all {
        f.add(&mut r, "");
    }
    r.line(format!(
        "checked {} specs and {count} open changes: {errors} errors",
        living.len()
    ));
    r.set("specs", living.len());
    r.set("changes", count);
    r.set("errors", errors);
    r.table(
        "findings",
        all.iter().map(Problem::row).collect(),
        "0 in the specs and open changes",
    );
    r.ok = errors == 0;
    Ok(r)
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
    println!("{REVIEW}");
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

/// Adds one proof run's human line and returns its row and warnings.
fn proof_run(r: &mut Reply, p: &proof::Proved) -> (Obj, Vec<String>) {
    let o = &p.outcome;
    let tests = o.tests_run.map_or("unknown".to_string(), |n| n.to_string());
    let verdict = if p.proved {
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
    r.line(format!(
        "{} {verdict} ({tests} tests, {} ms, event {})",
        p.task, o.duration_ms, p.event
    ));
    let mut warnings: Vec<String> = p
        .lfs_pointers
        .iter()
        .map(|f| format!("{f} is a Git LFS pointer, run git lfs pull"))
        .collect();
    for w in &warnings {
        r.line(format!("warning: {w}"));
    }
    if p.looping {
        let hint = format!(
            "the same failure {} times in a row; if the task cannot pass as written, run rvow block {} --reason \"...\"",
            proof::LOOP_RUNS,
            p.task
        );
        r.line(format!("hint: {hint}"));
        warnings.push(hint);
    }
    let row = obj! {
        "task" => p.task.to_string(),
        "result" => verdict,
        "tests" => o.tests_run,
        "ms" => o.duration_ms,
        "event" => p.event,
    };
    (row, warnings)
}

fn prove(task: &str) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let proved = proof::prove(&p, &home, &cwd()?, task, &mut std::io::stderr())?;
    let mut r = Reply::new();
    let (row, warnings) = proof_run(&mut r, &proved);
    r.data = row;
    if !warnings.is_empty() {
        r.set("warnings", warnings);
    }
    match &proved.next {
        Some(next) => {
            r.text.push_str(&format!("next: {next}"));
            r.set("next", next.lines().collect::<Vec<_>>());
        }
        None => {
            let done = "every task on this branch is proved or blocked";
            r.line(format!("next: {done}"));
            r.set("next", done);
            r.help("Run `rvow gate` to check the branch before push");
        }
    }
    r.ok = proved.proved;
    Ok(r)
}

fn prove_stale() -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let cwd = cwd()?;
    let stale = proof::stale_tasks(&p, &home, &cwd)?;
    let mut r = Reply::new();
    if stale.is_empty() {
        r.line("no stale tasks");
        r.set(
            "proofs",
            format!("0 stale tasks on branch {}", branch_label()),
        );
        return Ok(r);
    }
    let mut rows = Vec::new();
    let mut all_warnings = Vec::new();
    for task in &stale {
        let proved = proof::prove(&p, &home, &cwd, task, &mut std::io::stderr())?;
        let (row, warnings) = proof_run(&mut r, &proved);
        rows.push(row);
        all_warnings.extend(warnings);
        r.ok &= proved.proved;
    }
    r.set("count", format!("{} stale tasks", rows.len()));
    r.table("proofs", rows, "");
    if !all_warnings.is_empty() {
        r.set("warnings", all_warnings);
    }
    Ok(r)
}

fn ls() -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let mut r = Reply::new();
    let mut rows = Vec::new();
    for s in change::summaries(&p)? {
        r.line(format!(
            "{} {:<10} {} {}",
            s.id,
            s.stage.name(),
            s.branch,
            s.title
        ));
        rows.push(obj! {
            "id" => s.id.to_string(),
            "stage" => s.stage.name(),
            "branch" => s.branch,
            "title" => s.title,
        });
    }
    if rows.is_empty() {
        r.line("no changes");
        r.help("Run `rvow new \"<title>\"` to plan a change");
    } else {
        r.set("count", rows.len());
        r.help("Run `rvow show <id>` to see a change in full");
        r.help("Run `rvow status` for the tasks on this branch");
    }
    r.table("changes", rows, "0 in this project");
    Ok(r)
}

fn path(id: &str) -> Result<Reply> {
    let home = store::home_dir()?;
    let found = change::locate(&project(&home)?, id)?.to_string();
    let mut r = Reply::new();
    r.line(&found);
    r.set("path", found);
    Ok(r)
}

fn show(id: &str, full: bool) -> Result<Reply> {
    let home = store::home_dir()?;
    Ok(Reply::long(
        show::show(&project(&home)?, id)?,
        full,
        &format!("rvow show {id}"),
    ))
}

fn spec_list() -> Result<Reply> {
    let home = store::home_dir()?;
    let (living, _) = change::load_living(&project(&home)?)?;
    let mut r = Reply::new();
    let mut rows = Vec::new();
    for (cap, lines) in show::overview(&living) {
        r.line(&cap);
        for line in lines {
            r.line(format!("  {line}"));
            let (id, title) = line.split_once(' ').unwrap_or((&line, ""));
            rows.push(obj! { "id" => id, "capability" => cap, "title" => title });
        }
    }
    if rows.is_empty() {
        r.line("no living specs");
        r.help("Run `rvow new --spec \"<title>\"` to plan a change with spec deltas");
    } else {
        r.set(
            "count",
            format!(
                "{} requirements in {} capabilities",
                rows.len(),
                living.len()
            ),
        );
        r.help("Run `rvow show <REQ-id>` to read a requirement");
        r.help("Run `rvow trace <REQ-id>` to list the changes that touched it");
    }
    r.table("requirements", rows, "0 in the living specs");
    Ok(r)
}

fn guide(artifact: Artifact) -> Reply {
    Reply::lines(
        match artifact {
            Artifact::Proposal => guide::PROPOSAL,
            Artifact::Spec => guide::SPEC,
            Artifact::Tasks => guide::TASKS,
            Artifact::Design => guide::DESIGN,
        }
        .to_owned(),
    )
}

fn ids(target: Option<&str>) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let minted = vow::mint_active(&p, &cwd()?, target)?;
    let mut r = Reply::new();
    let mut rows = Vec::new();
    for (from, to) in &minted {
        r.line(format!("{from} -> {to}"));
        rows.push(obj! { "from" => from.to_string(), "to" => to.to_string() });
    }
    if minted.is_empty() {
        r.line("no placeholders");
    }
    r.table(
        "minted",
        rows,
        format!("0 placeholders on branch {}", branch_label()),
    );
    Ok(r)
}

fn brief() -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    Ok(Reply::lines(brief::brief(&p, &home, &cwd()?)?))
}

fn status(all: bool, json: bool) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let mut r = Reply::new();
    if json {
        let v = view::view(&p, &home, &cwd()?)?.unwrap_or(serde_json::Value::Null);
        println!("{v}");
        return Ok(r);
    }
    let changes = proof::status(&p, &home, &cwd()?, all)?;
    if changes.is_empty() {
        r.line("no active change on this branch");
        let scope = if all {
            "in this project".to_owned()
        } else {
            format!("on branch {}", branch_label())
        };
        r.set("changes", format!("0 active {scope}"));
        r.help("Run `rvow ls` to list every change");
        return Ok(r);
    }
    let mut items = Vec::new();
    let mut tasks = 0;
    let (mut drift, mut pending) = (false, false);
    for c in &changes {
        r.line(format!("{} on {}", c.id, c.branch));
        let mut rows = Vec::new();
        for t in &c.tasks {
            let state = match &t.state {
                State::Stale(why) => format!("stale ({})", why.join(", ")),
                s => s.name().to_string(),
            };
            r.line(format!("  {:<10} {:<20} {}", t.task, state, t.title));
            rows.push(obj! { "id" => t.task.to_string(), "state" => state, "title" => t.title });
        }
        tasks += rows.len();
        let mut item = obj! { "id" => c.id.to_string(), "branch" => c.branch };
        item.push((
            "tasks".to_owned(),
            Node::from(rows.into_iter().map(Node::Obj).collect::<Vec<_>>()),
        ));
        for problem in &c.problems {
            r.line(format!("  error: {problem}"));
        }
        if !c.problems.is_empty() {
            item.push(("errors".to_owned(), Node::from(c.problems.clone())));
        }
        let mut rows = Vec::new();
        for d in &c.drift {
            r.line(format!("  drift {}: {}", d.class.name(), d.what));
            rows.push(Node::Obj(
                obj! { "class" => d.class.name(), "what" => d.what },
            ));
        }
        if !rows.is_empty() {
            drift = true;
            item.push(("drift".to_owned(), Node::List(rows)));
        }
        let mut rows = Vec::new();
        for a in &c.pending {
            r.line(format!("  pending {}: {}", a.id, a.reason));
            rows.push(Node::Obj(
                obj! { "id" => a.id.to_string(), "reason" => a.reason },
            ));
        }
        if !rows.is_empty() {
            pending = true;
            item.push(("pending".to_owned(), Node::List(rows)));
        }
        items.push(Node::Obj(item));
    }
    r.set(
        "count",
        format!("{tasks} tasks in {} changes", changes.len()),
    );
    r.set("changes", items);
    r.help("Run `rvow prove <task-id>` to run a task's check and record the proof");
    if drift {
        r.help("Run `rvow amend --reason \"<why>\"` to record the drift for the user to confirm");
    }
    if pending {
        r.help("Ask the user to run `rvow confirm` to decide the pending amendments");
    }
    Ok(r)
}

fn gate(opts: Options) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let g = gate::gate(&p, &home, &cwd()?, opts, &mut std::io::stderr())?;
    let mut r = Reply::new();
    let mut rows = Vec::new();
    for (task, ms) in &g.timings {
        r.line(format!("rerun {task}: {ms} ms"));
        rows.push(obj! { "task" => task, "ms" => ms });
    }
    for w in &g.warnings {
        r.line(format!("warning: {w}"));
    }
    for f in &g.failures {
        r.line(format!("fail: {f}"));
    }
    r.ok = g.passed();
    if r.ok {
        r.line(format!(
            "gate passed ({} active changes on this branch)",
            g.changes
        ));
    } else {
        r.line(format!("gate failed: {} problems", g.failures.len()));
        r.help("Run `rvow status` to see each task's state");
    }
    r.set("passed", r.ok);
    r.set("changes", g.changes);
    if !rows.is_empty() {
        r.table("reruns", rows, "");
    }
    if !g.warnings.is_empty() {
        r.set("warnings", g.warnings);
    }
    if !g.failures.is_empty() {
        r.set("failures", g.failures);
    }
    Ok(r)
}

fn archive(target: &str) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let a = archive::archive(&p, &home, &cwd()?, target)?;
    let mut r = Reply::new();
    let mut rows = Vec::new();
    for (op, id) in &a.merged {
        r.line(format!("  {} {id}", op.word()));
        rows.push(obj! { "op" => op.word(), "id" => id });
    }
    r.line(format!("archived {} to {}", a.id, a.path.display()));
    r.set("archived", a.id.to_string());
    r.set("path", a.path.display().to_string());
    r.table("merged", rows, "0 spec deltas");
    Ok(r)
}

fn drop_change(target: &str, reason: &str) -> Result<Reply> {
    let home = store::home_dir()?;
    let path = archive::drop_change(&project(&home)?, target, reason)?;
    let mut r = Reply::new();
    r.line(format!("dropped to {}", path.display()));
    r.set("dropped", path.display().to_string());
    Ok(r)
}

fn supersede(old: &str, by: &str, reason: &str) -> Result<Reply> {
    let home = store::home_dir()?;
    let path = archive::supersede(&project(&home)?, old, by, reason)?;
    let mut r = Reply::new();
    r.line(format!("superseded to {}", path.display()));
    r.set("superseded", path.display().to_string());
    Ok(r)
}

fn trace(id: &str) -> Result<Reply> {
    let home = store::home_dir()?;
    let (id, touches) = archive::trace(&project(&home)?, id)?;
    let mut r = Reply::new();
    if touches.is_empty() {
        r.line(format!("no change touched {id}"));
    }
    let mut rows = Vec::new();
    for t in &touches {
        let date = t.date.as_deref().unwrap_or("-");
        r.line(format!(
            "{} {:<10} {date:<10} {:<10} {}",
            t.change,
            t.stage.name(),
            t.ops.join(","),
            t.title
        ));
        rows.push(obj! {
            "change" => t.change.to_string(),
            "stage" => t.stage.name(),
            "date" => date,
            "ops" => t.ops.join(" "),
            "title" => t.title,
        });
    }
    r.set("id", id.to_string());
    if !rows.is_empty() {
        r.set("count", rows.len());
    }
    r.table("changes", rows, format!("0 touched {id}"));
    Ok(r)
}

fn amend(reason: &str, target: Option<&str>, unlock: Option<&str>) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let mut r = Reply::new();
    let mut items = Vec::new();
    for a in amend::amend(&p, &home, &cwd()?, target, reason, unlock)? {
        r.line(format!("{} for {}:", a.id, a.change));
        let mut rows = Vec::new();
        for i in &a.items {
            r.line(format!("  {}: {}", i.class.name(), i.what));
            rows.push(Node::Obj(
                obj! { "class" => i.class.name(), "what" => i.what },
            ));
        }
        let mut item = obj! { "id" => a.id.to_string(), "change" => a.change.to_string() };
        item.push(("items".to_owned(), Node::List(rows)));
        items.push(Node::Obj(item));
    }
    r.line("ask the user to run /rvow-confirm (or ! rvow confirm)");
    r.set("amendments", items);
    r.help("Ask the user to run /rvow-confirm (or ! rvow confirm) to decide them");
    Ok(r)
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

fn block(task: &str, reason: &str) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let (change, task) = notes::block(&p, task, reason)?;
    let mut r = Reply::new();
    r.line(format!("{task} blocked in {change}"));
    r.line("the gate refuses it until the user confirms an amendment that changes or drops it");
    r.set("blocked", task.as_str());
    r.set("change", change.to_string());
    r.help("Run `rvow amend --reason \"<why>\"` after editing the task, for the user to confirm");
    Ok(r)
}

fn note(kind: NoteArg, text: &str) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let kind = match kind {
        NoteArg::Decision => notes::NoteKind::Decision,
        NoteArg::Surprise => notes::NoteKind::Surprise,
        NoteArg::Handoff => notes::NoteKind::Handoff,
    };
    let (change, cut) = notes::note(&p, &cwd()?, kind, text)?;
    let mut r = Reply::new();
    r.line(format!("{} noted in {change}", kind.name()));
    r.set("noted", kind.name());
    r.set("change", change.to_string());
    if cut {
        let w = format!(
            "the handoff kept only its first {} lines",
            notes::HANDOFF_LINES
        );
        r.line(format!("warning: {w}"));
        r.set("warning", w);
    }
    Ok(r)
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
    Status,
    View,
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
            (Harness::Claude, HookEvent::View) => view::view(&p, &home, &cwd),
            (Harness::Claude, HookEvent::Status) => {
                Ok(hook::status_line(&p, &home, &cwd)?.map(|s| serde_json::json!({ "status": s })))
            }
        }
    };
    match run() {
        Ok(Some(out)) => println!("{out}"),
        Ok(None) => {}
        Err(e) => eprintln!("rvow hook: {}: {}", e.code, e.message),
    }
    ExitCode::SUCCESS
}

#[derive(Clone, Copy, ValueEnum)]
enum Scope {
    User,
    Project,
}

fn report(r: &mut Reply, what: &str, o: install::Outcome) {
    r.line(match o {
        install::Outcome::Written(p) => format!("{what}: wrote {}", p.display()),
        install::Outcome::Unchanged(p) => format!("{what}: up to date at {}", p.display()),
        install::Outcome::KeptEdit(p) => format!(
            "{what}: kept your edits in {}; delete it to get the new version",
            p.display()
        ),
        install::Outcome::AddLine { path, line } => format!(
            "{what}: another hook manager owns {}; add this line to it:\n  {line}",
            path.display()
        ),
    });
}

#[derive(Clone, Copy, ValueEnum)]
enum ClaudeMode {
    Mod,
    Settings,
}

fn install(mode: ClaudeMode, scope: Scope) -> Result<Reply> {
    let mut r = Reply::new();
    let rvow = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| Error::io(Path::new("rvow"), e))?;
    let env_dir = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let home =
        env_dir("HOME").ok_or_else(|| Error::new("no-home", "HOME is not set", "set HOME"))?;
    let claude = env_dir("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude"));
    let settings = match scope {
        Scope::User => claude.join("settings.json"),
        Scope::Project => git::toplevel(&cwd()?)?.join(".claude/settings.json"),
    };
    match mode {
        ClaudeMode::Mod => {
            let dir = store::home_dir()?.join(install::MARKETPLACE_DIR);
            report(&mut r, "Claude Code mod", install::claude_mod(&dir, &rvow)?);
            report(
                &mut r,
                "old settings hooks",
                install::remove_claude_settings(&settings)?,
            );
            match which("claude") {
                Some(claude) => match install::register_claude_mod(&claude, &dir) {
                    Ok(done) => {
                        for d in done {
                            r.line(format!("Claude Code: {d}"));
                        }
                    }
                    Err(e) => {
                        r.line(format!("Claude Code: {}", e.message));
                        r.line(format!("  {}", e.fix));
                    }
                },
                None => {
                    r.line("Claude Code: claude is not on PATH; to load the mod, run once:");
                    r.line(format!("  claude plugin marketplace add {}", dir.display()));
                    r.line("  claude plugin install rvow@rvow");
                }
            }
        }
        ClaudeMode::Settings => {
            report(&mut r, "skill", install::skill(&claude)?);
            report(
                &mut r,
                "Claude Code hooks",
                install::claude_settings(&settings, &rvow)?,
            );
        }
    }
    let pi = home.join(".pi/agent");
    if pi.is_dir() {
        report(&mut r, "pi extension", install::pi_extension(&pi)?);
    } else {
        r.line(format!(
            "pi extension: skipped, {} does not exist",
            pi.display()
        ));
    }
    match store::find(&cwd()?, &store::home_dir()?) {
        Ok(Some(_)) => report(
            &mut r,
            "pre-push gate",
            install::pre_push(&git::toplevel(&cwd()?)?, &rvow)?,
        ),
        _ => {
            r.line("pre-push gate: skipped, run rvow install inside a repository with an rvow tree")
        }
    }
    let text = std::mem::take(&mut r.text);
    Ok(Reply::lines(text))
}

const REVIEW: &str = "\
before you accept, check:
  1. Does each requirement say what the system does, not how?
  2. Does each task's check fail today for the reason the task fixes?
  3. Is anything in Out of scope that you need now?
  4. Is every Assumption one you agree with?
  5. Is the change small enough to review its diff in one sitting?";

fn check(target: Option<&str>) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let reports = plan::check(&p, &cwd()?, target)?;
    let mut r = Reply::new();
    if reports.is_empty() {
        r.line("no open change on this branch");
        r.set("changes", format!("0 open on branch {}", branch_label()));
        r.help("Run `rvow new \"<title>\"` to plan a change");
        return Ok(r);
    }
    let mut items = Vec::new();
    for c in &reports {
        let id = &c.change.id;
        r.line(format!("{id} ({})", c.change.stage.name()));
        let mut found = problems("", &c.findings);
        found.extend(c.missing.iter().map(|(task, prog)| Problem {
            at: task.clone(),
            level: "error",
            code: "missing-program",
            message: format!("{prog} is neither on PATH nor in the repository"),
            fix: "",
        }));
        for f in &found {
            f.add(&mut r, "  ");
        }
        let mut questions = Vec::new();
        for q in &c.questions {
            r.line(format!(
                "  {}:{}: open question: {}",
                q.path, q.line, q.question
            ));
            questions.push(Node::Obj(obj! {
                "at" => format!("{}:{}", q.path, q.line),
                "question" => q.question,
            }));
        }
        let ready = !c.blocking();
        let mut item = obj! {
            "id" => id.to_string(),
            "stage" => c.change.stage.name(),
            "ready" => ready,
        };
        if !found.is_empty() {
            let rows = found.iter().map(|f| Node::Obj(f.row())).collect();
            item.push(("findings".to_owned(), Node::List(rows)));
        }
        if !questions.is_empty() {
            item.push(("questions".to_owned(), Node::List(questions)));
            r.help(format!(
                "Run `rvow clarify {id}` for the questions with recommended answers"
            ));
        }
        items.push(Node::Obj(item));
        if ready {
            r.line(format!(
                "  ready to accept: ask the user to run /rvow-accept {id} (or ! rvow accept {id})"
            ));
            r.help(format!(
                "Ask the user to run /rvow-accept {id} (or ! rvow accept {id})"
            ));
        } else {
            r.ok = false;
            r.line("  not ready to accept");
            r.help(format!("Run `rvow check {id}` after fixing the plan"));
        }
    }
    r.set("changes", items);
    Ok(r)
}

fn clarify(target: Option<&str>, json: bool) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let mut r = Reply::new();
    if json {
        println!("{}", interview::round(&p, &cwd()?, target)?);
        return Ok(r);
    }
    let mut rows = Vec::new();
    for c in plan::check(&p, &cwd()?, target)? {
        for q in &c.questions {
            let rec = q.recommended.as_deref().unwrap_or("none given");
            r.line(format!(
                "{}:{}:{}: {} (recommended: {rec})",
                c.change.id, q.path, q.line, q.question
            ));
            rows.push(obj! {
                "ref" => format!("{}:{}:{}", c.change.id, q.path, q.line),
                "question" => q.question,
                "recommended" => rec,
            });
        }
    }
    if rows.is_empty() {
        r.line("no open questions");
    } else {
        let ask = "ask the user with the rvow_ask tool, or ask in chat and record each answer with rvow answer <ref> \"<choice>\"";
        r.line(ask);
        r.set("count", rows.len());
        r.help(ask);
    }
    r.table(
        "questions",
        rows,
        format!("0 open on branch {}", branch_label()),
    );
    Ok(r)
}

#[derive(Clone, Copy, ValueEnum)]
enum ViaArg {
    Dialog,
    Chat,
}

fn answer(reference: &str, choice: &str, via: ViaArg) -> Result<bool> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let via = match via {
        ViaArg::Dialog => interview::Via::Dialog,
        ViaArg::Chat => interview::Via::Chat,
    };
    let a = interview::answer(&p, reference, choice, via)?;
    let how = if a.recommended {
        "the recommended answer"
    } else {
        "an answer other than the recommended one"
    };
    println!(
        "Recorded for {}: {} -> {} ({how}).",
        a.change, a.question, a.answer
    );
    println!(
        "Now update the requirement or task this answer affects, and remove any text it contradicts."
    );
    Ok(true)
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
    })
}
