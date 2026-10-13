use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{ArgGroup, CommandFactory, Parser, Subcommand, ValueEnum};
use rvow_core::error::Result;
use rvow_core::gate::Options;

#[macro_use]
mod toon;
mod cmd;
#[cfg(test)]
mod doc_commands;
mod output;
mod skill;
mod w18;

use cmd::{attest, export, lifecycle, plan, prove, read, setup};
use output::Reply;

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
    /// Repair this repository's rvow tree: record its roots and remote, and move a home tree to its remote path.
    Rebind {
        /// Read a git filter-repo commit map to find a rewritten repository.
        #[arg(long, value_name = "FILE")]
        commit_map: Option<PathBuf>,
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
        /// Kept for old callers; accept never asks.
        #[arg(long, hide = true)]
        yes: bool,
    },
    /// Print a plan and the accept checklist; runs no verify command.
    Review {
        /// The change id or a unique prefix of it.
        change: String,
        /// Build fresh-context advisory evidence without running checks.
        #[arg(long, conflicts_with = "record")]
        packet: bool,
        /// Record advisory JSON findings against the current packet identity.
        #[arg(long)]
        record: Option<PathBuf>,
        /// Record as a judge note with harness and model.
        #[arg(long, requires = "record")]
        judge: bool,
        /// The harness that ran the judge.
        #[arg(long, requires = "judge", required_if_eq("judge", "true"))]
        harness: Option<String>,
        /// The model the judge ran on.
        #[arg(long, requires = "judge", required_if_eq("judge", "true"))]
        model: Option<String>,
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
        /// One part of a change: `proposal`, `tasks` or `spec:<name>`.
        part: Option<String>,
        /// Print long text in full in TOON.
        #[arg(long)]
        full: bool,
    },
    /// Replace one part of a change, or one task or requirement block, with the text on stdin.
    Put {
        /// A CHG id with `part`, or a TSK or REQ id whose block the stdin replaces.
        target: String,
        /// One part of a change: `proposal`, `tasks` or `spec:<name>`.
        part: Option<String>,
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
    /// Write rvow's specs and changes as an OpenSpec tree, one way.
    Export {
        #[command(subcommand)]
        cmd: ExportCmd,
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
    /// Print the shared dashboard with living specs and branch tasks.
    View,
    /// Print the short generated brief for this branch.
    Brief,
    /// Print the next task of this branch.
    Next,
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
    /// Move a backlog plan to the current branch, so it can be accepted here.
    Claim {
        /// The change id or a unique prefix of it.
        change: String,
    },
    /// List the open questions of a plan, each with its recommended answer.
    Clarify {
        /// The change id; default: every open change on this branch.
        change: Option<String>,
        /// Print the next round (at most interview_cap questions, default 3) as JSON: question, options, recommended first.
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
        /// The neutral event, or claude followed by its event for older callers.
        #[arg(value_enum)]
        target: HookTarget,
        #[arg(value_enum)]
        event: Option<HookEvent>,
    },
    /// Show how often the hooks denied or blocked in the last days.
    Stats {
        /// How many days back to count.
        #[arg(long, default_value_t = 7)]
        days: u64,
    },
    /// Relax the hooks for a while: audit, auto or bypass. Without a tier it shows what is on.
    Yolo {
        #[arg(value_enum)]
        tier: Option<YoloTier>,
        /// How long it stays on, such as 30m or 2h (default 2h, at most 24h).
        #[arg(long = "for")]
        length: Option<String>,
    },
    /// Lease one task to a subagent and print the brief to give it.
    Delegate {
        /// The task id or a unique prefix of it.
        task: String,
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
        /// Sign the decision with the global signing_key.
        #[arg(long, requires = "signer", requires = "decision")]
        sign: bool,
        /// Explicit principal in the allowed-signers file.
        #[arg(long, requires = "sign", value_name = "IDENTITY")]
        signer: Option<String>,
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
    /// Export accepted verification data for a CI checkout without the home store.
    Bundle {
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
    },
    /// Check the active changes of this branch.
    #[command(group(clap::ArgGroup::new("runner").args(["rerun", "bundle"])))]
    Gate {
        /// Fail on stale and flaky proofs too.
        #[arg(long)]
        strict: bool,
        /// Run every verify command again in a clean worktree of HEAD.
        #[arg(long)]
        rerun: bool,
        /// With --rerun, run every task even when an earlier clean pass has the same key.
        #[arg(long, requires = "rerun")]
        no_cache: bool,
        /// Write each rerun result to this file, for `rvow ingest` to read.
        #[arg(long, value_name = "FILE", requires = "runner")]
        out: Option<PathBuf>,
        /// Execute an accepted verification bundle without accessing the home store.
        #[arg(long, value_name = "FILE", conflicts_with = "rerun")]
        bundle: Option<PathBuf>,
    },
    /// Add proofs that a `gate --out` file carried to the ledger of the
    /// project each run names.
    Ingest {
        /// The run file that `gate --out` wrote.
        file: PathBuf,
    },
    /// Export the current proofs of a change as in-toto attestations.
    Attest {
        /// The change id or a unique prefix of it.
        change: String,
        /// Write the JSON lines to this file.
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
        /// Sign the file with the configured signing key.
        #[arg(long)]
        sign: bool,
    },
    /// Fill the grammar cache, or show the pinned languages and their cache state.
    Grammar {
        #[command(subcommand)]
        cmd: GrammarCmd,
    },
}

#[derive(Subcommand)]
enum GrammarCmd {
    /// Download pinned grammars into the cache; without names, every pinned language.
    Fetch {
        /// Language names from `rvow grammar list`.
        #[arg(num_args = 0..)]
        langs: Vec<String>,
    },
    /// Show each pinned language and whether the cache holds its grammar.
    List,
}
/// The change id or a unique prefix of it.

#[derive(Subcommand)]
enum SpecCmd {
    /// List each capability with its requirement ids and titles.
    List,
}

#[derive(Subcommand)]
enum ExportCmd {
    /// Write an OpenSpec tree under <dir>/openspec; never overwrites.
    Openspec {
        /// The target folder; default: ./openspec-export.
        #[arg(long)]
        to: Option<PathBuf>,
        /// Print what would be written and write nothing.
        #[arg(long)]
        dry_run: bool,
    },
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
    Judge,
}

fn main() -> ExitCode {
    reset_sigpipe();
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
        return finish(read::home_view(), toon);
    };
    let toon = toon && !matches!(cmd, Cmd::Accept { .. } | Cmd::Confirm { .. } | Cmd::Answer { .. });
    dispatch(cmd, toon)
}

fn reset_sigpipe() {
    #[cfg(unix)]
    // SAFETY: restores the default SIGPIPE action before any thread starts, so
    // `rvow ... | head` ends quietly instead of panicking on a closed pipe.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

fn dispatch(cmd: Cmd, toon: bool) -> ExitCode {
    let result = match cmd {
        Cmd::Init { in_repo } => setup::init(in_repo),
        Cmd::Rebind { commit_map } => setup::rebind(commit_map.as_deref()),
        Cmd::New { title, spec } => plan::new(&title.join(" "), !spec),
        Cmd::Validate => plan::validate(),
        Cmd::Accept { change, .. } => lifecycle::accept(&change).map(Reply::done),
        Cmd::Review { change, packet, record, judge, harness, model } => {
            plan::review(&change, packet, record.as_deref(), judge, harness.as_deref(), model.as_deref())
        }
        Cmd::Ls => read::ls(),
        Cmd::Path { id } => read::path(&id),
        Cmd::Show { id, part, full } => match part {
            Some(part) => match read::show_part(&id, part.as_str(), full) {
                Ok(r) => Ok(r),
                Err(e) if e.code == "part" => return part_usage_error(&e, toon),
                Err(e) => Err(e),
            },
            None => read::show(&id, full),
        },
        Cmd::Put { target, part } => match plan::put(&target, part.as_deref()) {
            Ok(r) => Ok(r),
            Err(e) if e.code == "part" => return part_usage_error(&e, toon),
            Err(e) => Err(e),
        },
        Cmd::Spec { cmd: SpecCmd::List } => read::spec_list(),
        Cmd::Import { cmd: ImportCmd::Openspec { from, dry_run } } => plan::import_openspec(from, dry_run),
        Cmd::Export { cmd: ExportCmd::Openspec { to, dry_run } } => export::export_openspec(to, dry_run),
        Cmd::Guide { artifact } => read::guide(artifact),
        Cmd::Ids { change } => plan::ids(change.as_deref()),
        Cmd::View => read::dashboard(),
        Cmd::Brief => read::brief(),
        Cmd::Next => read::next(),
        Cmd::Hook { target, event } => {
            let event = match (target, event) {
                (HookTarget::Claude, Some(event)) => Some(event),
                (target, None) => target.event(),
                _ => None,
            };
            return match event {
                Some(event) => setup::hook(Harness::Claude, event),
                None => {
                    usage_error(clap::Error::raw(clap::error::ErrorKind::InvalidValue, "use rvow hook <event> or rvow hook claude <event>"))
                }
            };
        }
        Cmd::Install { claude, scope } => setup::install(claude, scope),
        Cmd::Check { change } => plan::check(change.as_deref()),
        Cmd::Clarify { change, json } => plan::clarify(change.as_deref(), json),
        Cmd::Claim { change } => plan::claim(&change),
        Cmd::Answer { reference, choice, via } => plan::answer(&reference, &choice.join(" "), via).map(Reply::done),
        Cmd::Stats { days } => cmd::stats::stats(days),
        Cmd::Delegate { task } => cmd::delegate::delegate(&task),
        Cmd::Yolo { tier, length } => cmd::yolo::yolo(
            tier.map(|t| match t {
                YoloTier::Audit => Some(rvow_core::yolo::Tier::Audit),
                YoloTier::Auto => Some(rvow_core::yolo::Tier::Auto),
                YoloTier::Bypass => Some(rvow_core::yolo::Tier::Bypass),
                YoloTier::Off => None,
            }),
            length.as_deref(),
        ),
        Cmd::Block { task, reason } => lifecycle::block(&task, &reason),
        Cmd::Note { kind, text } => lifecycle::note(kind, &text.join(" ")),
        Cmd::Amend { reason, change, unlock } => lifecycle::amend(&reason, change.as_deref(), unlock.as_deref()),
        Cmd::Confirm { ids, accept, reject, all, sign, signer } => {
            lifecycle::confirm(&ids, accept, reject, all.as_deref(), sign, signer.as_deref()).map(Reply::done)
        }
        Cmd::Prove { task: Some(t), .. } => prove::prove(&t),
        Cmd::Prove { .. } => prove::prove_stale(),
        Cmd::Status { all, json } => read::status(all, json),
        Cmd::Bundle { out } => prove::bundle(&out),
        Cmd::Gate { strict: _, rerun: _, no_cache: _, out, bundle: Some(bundle) } => prove::gate_bundle(&bundle, out.as_deref()),
        Cmd::Gate { strict, rerun, no_cache, out, bundle: None } => prove::gate(Options { strict, rerun, out }, no_cache),
        Cmd::Ingest { file } => cmd::ingest::ingest(&file),
        Cmd::Attest { change, out, sign } => attest::attest(&change, &out, sign),
        Cmd::Grammar { cmd: GrammarCmd::Fetch { langs } } => cmd::grammar::fetch(&langs),
        Cmd::Grammar { cmd: GrammarCmd::List } => cmd::grammar::list(),
        Cmd::Archive { change } => lifecycle::archive(&change),
        Cmd::Drop { change, reason } => lifecycle::drop_change(&change, &reason),
        Cmd::Supersede { old, by, reason } => lifecycle::supersede(&old, &by, &reason),
        Cmd::Trace { id } => read::trace(&id),
    };
    finish(result, toon)
}

/// Prints a result or an error and maps it to the exit code: 0 success, 1 error.
fn finish(result: Result<Reply>, toon: bool) -> ExitCode {
    match result {
        Ok(r) => {
            let ok = r.ok;
            r.print(toon);
            if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        Err(e) => {
            if toon {
                println!("{}", output::error(&e.message, vec![e.fix], e.code));
            } else {
                println!("error[{}]: {}", e.code, e.message);
                println!("  fix: {}", e.fix);
            }
            ExitCode::FAILURE
        }
    }
}

/// A wrong part of a change on stdout with exit 2, listing the parts the
/// change does have.
fn part_usage_error(e: &rvow_core::error::Error, toon: bool) -> ExitCode {
    if toon {
        println!("{}", output::error(&e.message, vec![e.fix.clone()], e.code));
    } else {
        println!("error[{}]: {}", e.code, e.message);
        println!("  fix: {}", e.fix);
    }
    ExitCode::from(2)
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
    let (path, mut cmd) = failed_command(&args);
    let no_sub = e.kind() == ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand;
    let wants_sub = no_sub || matches!(e.kind(), ErrorKind::InvalidSubcommand | ErrorKind::MissingSubcommand);
    let valid = if wants_sub && cmd.has_subcommands() { valid_subcommands(&path, &cmd) } else { valid_flags(&path, &cmd) };
    let rendered = e.render().to_string();
    if toon {
        let message = usage_message(&rendered, &path, no_sub);
        let mut help = usage_tips(&rendered);
        help.push(cmd.render_usage().to_string().replace("Usage: ", "usage: "));
        help.push(valid);
        println!("{}", output::error(&message, help, "usage"));
    } else {
        println!("{}", rendered.trim_end());
        println!("{valid}");
    }
    ExitCode::from(2)
}

/// The deepest subcommand that the arguments name, and its path as `rvow a b`.
fn failed_command(args: &[String]) -> (String, clap::Command) {
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
    (path, cmd)
}

fn valid_subcommands(path: &str, cmd: &clap::Command) -> String {
    let subs: Vec<&str> = cmd.get_subcommands().map(|c| c.get_name()).filter(|n| *n != "help").collect();
    format!("valid subcommands for `{path}`: {}", subs.join(", "))
}

fn valid_flags(path: &str, cmd: &clap::Command) -> String {
    let flags: Vec<String> = cmd
        .get_arguments()
        .filter_map(|a| a.get_long())
        .filter(|l| !matches!(*l, "help" | "version" | "toon" | "human"))
        .map(|l| format!("--{l}"))
        .collect();
    let own = if flags.is_empty() { "none".to_owned() } else { flags.join(", ") };
    format!("valid flags for `{path}`: {own}; on every command: --toon, --human, --help")
}

fn usage_message(rendered: &str, path: &str, no_sub: bool) -> String {
    if no_sub {
        return format!("`{path}` needs a subcommand");
    }
    let head = rendered.split("\n\n").next().unwrap_or_default();
    let words: Vec<&str> = head.lines().map(str::trim).collect();
    words.join(" ").trim_start_matches("error: ").to_owned()
}

fn usage_tips(rendered: &str) -> Vec<String> {
    rendered.lines().filter_map(|l| l.trim().strip_prefix("tip: ")).filter(|tip| !tip.starts_with("to pass ")).map(str::to_owned).collect()
}

#[derive(Clone, Copy, ValueEnum)]
enum NoteArg {
    Decision,
    Surprise,
    Handoff,
}

#[derive(Clone, Copy, ValueEnum)]
enum Harness {
    Claude,
}

#[derive(Clone, Copy, ValueEnum)]
enum YoloTier {
    /// Every rule except tampering only audits.
    Audit,
    /// Also lets the agent run rvow accept and rvow confirm.
    Auto,
    /// Every hook only audits and a stop is never held back.
    Bypass,
    /// Switch it off.
    Off,
}

#[derive(Clone, Copy, ValueEnum)]
enum HookTarget {
    Claude,
    PreToolUse,
    SessionStart,
    Stop,
    Status,
    View,
    LeaseBind,
    LeaseEnd,
}

impl HookTarget {
    fn event(self) -> Option<HookEvent> {
        match self {
            Self::Claude => None,
            Self::PreToolUse => Some(HookEvent::PreToolUse),
            Self::SessionStart => Some(HookEvent::SessionStart),
            Self::Stop => Some(HookEvent::Stop),
            Self::Status => Some(HookEvent::Status),
            Self::View => Some(HookEvent::View),
            Self::LeaseBind => Some(HookEvent::LeaseBind),
            Self::LeaseEnd => Some(HookEvent::LeaseEnd),
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum HookEvent {
    PreToolUse,
    SessionStart,
    Stop,
    Status,
    View,
    LeaseBind,
    LeaseEnd,
}

#[derive(Clone, Copy, ValueEnum)]
enum Scope {
    User,
    Project,
}

#[derive(Clone, Copy, ValueEnum)]
enum ClaudeMode {
    Mod,
    Settings,
}

#[derive(Clone, Copy, ValueEnum)]
enum ViaArg {
    Dialog,
    Chat,
}
