use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rvow_core::change::{self, Stage};
use rvow_core::diag::Severity;
use rvow_core::error::{Error, Result};
use rvow_core::git;
use rvow_core::lint::Finding;
use rvow_core::store::{self, Mode, Project};
use rvow_core::vow;

const USAGE: &str = "\
rvow: hold a coding agent to an accepted plan

usage:
  rvow init [--in-repo]          create the rvow tree for this repository
  rvow new <title> [--small]     create a change in backlog/
  rvow validate                  check specs and changes
  rvow accept <change> [--yes]   run the baseline and record the vow
";

enum Exit {
    Ok,
    Failed,
}

fn main() -> ExitCode {
    #[cfg(unix)]
    // SAFETY: restores the default SIGPIPE action before any thread starts, so
    // `rvow ... | head` ends quietly instead of panicking on a closed pipe.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((cmd, rest)) = args.split_first() else {
        print!("{USAGE}");
        return ExitCode::from(2);
    };
    let result = match cmd.as_str() {
        "init" => init(rest),
        "new" => new(rest),
        "validate" => validate(rest),
        "accept" => accept(rest),
        "-h" | "--help" | "help" => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("error: unknown command {cmd:?}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(Exit::Ok) => ExitCode::SUCCESS,
        Ok(Exit::Failed) => ExitCode::FAILURE,
        Err(e) if e.code == "usage" => {
            eprintln!("error: {}\n\n{USAGE}", e.message);
            ExitCode::from(2)
        }
        Err(e) => {
            println!("error[{}]: {}", e.code, e.message);
            println!("  fix: {}", e.fix);
            ExitCode::FAILURE
        }
    }
}

fn usage(msg: &str) -> Error {
    Error::new("usage", msg, "")
}

/// Splits arguments into flags from `allowed` and positional words.
fn parse<'a>(args: &'a [String], allowed: &[&str]) -> Result<(Vec<&'a str>, Vec<&'a str>)> {
    let mut flags = Vec::new();
    let mut words = Vec::new();
    for a in args {
        if a.starts_with("--") {
            if !allowed.contains(&a.as_str()) {
                return Err(usage(&format!("unknown flag {a}")));
            }
            flags.push(a.as_str());
        } else {
            words.push(a.as_str());
        }
    }
    Ok((flags, words))
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

fn init(args: &[String]) -> Result<Exit> {
    let (flags, words) = parse(args, &["--in-repo"])?;
    if !words.is_empty() {
        return Err(usage("init takes no arguments"));
    }
    let mode = if flags.contains(&"--in-repo") {
        Mode::Repo
    } else {
        Mode::Home
    };
    let p = store::init(&cwd()?, &store::home_dir()?, mode)?;
    println!("created {}", p.root.display());
    Ok(Exit::Ok)
}

fn new(args: &[String]) -> Result<Exit> {
    let (flags, words) = parse(args, &["--small"])?;
    if words.is_empty() {
        return Err(usage("new needs a title"));
    }
    let title = words.join(" ");
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
    let c = change::new(&p, &title, &branch, flags.contains(&"--small"))?;
    println!("created {} at {}", c.id, c.path.display());
    if busy {
        println!(
            "hint: {branch} already has an active change; one change per branch is the standard"
        );
    }
    Ok(Exit::Ok)
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

fn validate(args: &[String]) -> Result<Exit> {
    let (_, words) = parse(args, &[])?;
    if !words.is_empty() {
        return Err(usage("validate takes no arguments"));
    }
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
    let open = changes
        .iter()
        .filter(|c| matches!(c.stage, Stage::Backlog | Stage::Active));
    let mut count = 0;
    for c in open {
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
    Ok(if errors == 0 { Exit::Ok } else { Exit::Failed })
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

fn accept(args: &[String]) -> Result<Exit> {
    let (flags, words) = parse(args, &["--yes"])?;
    let [target] = words[..] else {
        return Err(usage("accept needs one change id"));
    };
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
    if !flags.contains(&"--yes") && !confirm("record the vow?")? {
        println!("not accepted");
        return Ok(Exit::Failed);
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
    Ok(Exit::Ok)
}
