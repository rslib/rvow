use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rvow_core::error::{Error, Result};
use rvow_core::git;
use rvow_core::hook as hooks;
use rvow_core::install;
use rvow_core::store::{self, Mode, Rebind};
use rvow_core::view;

use super::{cwd, which};
use crate::output::Reply;
use crate::toon::Node;
use crate::{ClaudeMode, Harness, HookEvent, Scope};

pub fn init(in_repo: bool) -> Result<Reply> {
    let mode = if in_repo { Mode::Repo } else { Mode::Home };
    let home = store::home_dir()?;
    let p = store::init(&cwd()?, &home, mode)?;
    store::record_home(&home)?;
    let mut r = Reply::new();
    r.line(format!("created {}", p.root.display()));
    r.set("created", p.root.display().to_string());
    r.help("Run `rvow new \"<title>\"` to plan a change");
    if mode == Mode::Home && git::toplevel(&home).is_err() {
        r.help(format!("Run `git -C {} init` to version your plans; init wrote its .gitignore", home.display()));
    }
    Ok(r)
}

pub fn rebind(commit_map: Option<&Path>) -> Result<Reply> {
    let home = store::home_dir()?;
    let mut r = Reply::new();
    let result = match commit_map {
        Some(path) => store::rebind_with_map(&cwd()?, &home, path)?,
        None => store::rebind(&cwd()?, &home)?,
    };
    match result {
        Rebind::Unchanged(root) => {
            r.line(format!("up to date at {}", root.display()));
            r.set("unchanged", root.display().to_string());
        }
        Rebind::Updated(root) => {
            r.line(format!("wrote project.toml at {}", root.display()));
            r.set("updated", root.display().to_string());
        }
        Rebind::Moved { from, to } => {
            r.line(format!("moved {} to {}", from.display(), to.display()));
            r.set("moved", Node::Obj(obj! { "from" => from.display().to_string(), "to" => to.display().to_string() }));
        }
    }
    Ok(r)
}

/// Hooks never fail the harness: outside an rvow project they print nothing,
/// and an internal error allows the call and goes to stderr, so a bug in rvow
/// cannot lock the agent out of every repository.
pub fn hook(harness: Harness, event: HookEvent) -> ExitCode {
    let mut text = String::new();
    if std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).is_err() {
        return ExitCode::SUCCESS;
    }
    let input: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let run = || -> Result<Option<serde_json::Value>> {
        let home = store::home_dir()?;
        let cwd = input.get("cwd").and_then(|v| v.as_str()).map(PathBuf::from).map_or_else(cwd, Ok)?;
        if matches!((harness, event), (Harness::Claude, HookEvent::SessionStart)) {
            let out = match store::find(&cwd, &home) {
                Ok(Some(p)) => hooks::claude_session_start(&p, &home, &input)?,
                _ => None,
            };
            return session_start_skill_warning(out, &home);
        }
        let Ok(Some(p)) = store::find(&cwd, &home) else {
            return Ok(None);
        };
        match (harness, event) {
            (Harness::Claude, HookEvent::PreToolUse) => hooks::claude_pre_tool_use(&p, &input),
            (Harness::Claude, HookEvent::SessionStart) => hooks::claude_session_start(&p, &home, &input),
            (Harness::Claude, HookEvent::Stop) => hooks::claude_stop(&p, &home, &input),
            (Harness::Claude, HookEvent::LeaseBind) => hooks::claude_lease_bind(&p, &input),
            (Harness::Claude, HookEvent::LeaseEnd) => hooks::claude_lease_end(&p, &input),
            (Harness::Claude, HookEvent::View) => view::view(&p, &home, &cwd),
            (Harness::Claude, HookEvent::Status) => Ok(hooks::status_line(&p, &home, &cwd)?.map(|s| serde_json::json!({ "status": s }))),
        }
    };
    match run() {
        Ok(Some(out)) => println!("{out}"),
        Ok(None) => {}
        Err(e) => eprintln!("rvow hook: {}: {}", e.code, e.message),
    }
    ExitCode::SUCCESS
}

fn session_start_skill_warning(mut out: Option<serde_json::Value>, rvow_home: &Path) -> Result<Option<serde_json::Value>> {
    let Some(home) = env_dir("HOME") else {
        return Ok(out);
    };
    let claude = env_dir("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude"));
    let rendered = crate::skill::render()?;
    let pi = install::pi_dir(Some(&home));
    for path in install::stale_managed_files(&claude, rvow_home, pi.as_deref(), &rendered) {
        let what = if path.ends_with("SKILL.md") { "skill" } else { "file" };
        let warning =
            format!("rvow: stale or edited managed {what}. Please run rvow install. Your edits are kept. Path: {}", path.display());
        let value = out
            .get_or_insert_with(|| serde_json::json!({"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": ""}}));
        let context = &mut value["hookSpecificOutput"]["additionalContext"];
        let previous = context.as_str().unwrap_or("");
        *context = rvow_core::brief::with_warning(previous, &warning).into();
    }
    Ok(out)
}

fn report(r: &mut Reply, what: &str, o: install::Outcome) {
    r.line(match o {
        install::Outcome::Written(p) => format!("{what}: wrote {}", p.display()),
        install::Outcome::Unchanged(p) => format!("{what}: up to date at {}", p.display()),
        install::Outcome::KeptEdit(p) => format!("{what}: kept your edits in {}; delete it to get the new version", p.display()),
        install::Outcome::Chained { path, prev } => {
            format!("{what}: moved your hook to {} and wrote {}, which runs it before the gate", prev.display(), path.display())
        }
        install::Outcome::AddLine { path, line } => {
            format!("{what}: another hook manager owns {}; add this line as its first command:\n  {line}", path.display())
        }
    });
}

fn env_dir(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn install(mode: ClaudeMode, scope: Scope) -> Result<Reply> {
    let mut r = Reply::new();
    let rvow_home = store::home_dir()?;
    if let Some(pointer) = store::record_home(&rvow_home)? {
        r.line(format!("rvow home: {} (recorded in {}, so hooks find it without RVOW_HOME)", rvow_home.display(), pointer.display()));
    }
    let rvow = std::env::current_exe().and_then(|p| p.canonicalize()).map_err(|e| Error::io(Path::new("rvow"), e))?;
    let home = env_dir("HOME").ok_or_else(|| Error::new("no-home", "HOME is not set", "set HOME"))?;
    let claude = env_dir("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude"));
    let settings = match scope {
        Scope::User => claude.join("settings.json"),
        Scope::Project => git::toplevel(&cwd()?)?.join(".claude/settings.json"),
    };
    match mode {
        ClaudeMode::Mod => install_mod(&mut r, &settings, &rvow)?,
        ClaudeMode::Settings => {
            report(&mut r, "skill", install::skill(&claude, &crate::skill::render()?)?);
            report(&mut r, "Claude Code hooks", install::claude_settings(&settings, &rvow)?);
        }
    }
    install_pi(&mut r, &home)?;
    install_pre_push(&mut r, &rvow)?;
    let text = std::mem::take(&mut r.text);
    Ok(Reply::lines(text))
}

fn install_mod(r: &mut Reply, settings: &Path, rvow: &Path) -> Result<()> {
    let dir = store::home_dir()?.join(install::MARKETPLACE_DIR);
    report(r, "Claude Code mod", install::claude_mod(&dir, rvow, &crate::skill::render()?)?);
    report(r, "old settings hooks", install::remove_claude_settings(settings)?);
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
    Ok(())
}

fn install_pi(r: &mut Reply, home: &Path) -> Result<()> {
    let pi = install::pi_dir(Some(home)).unwrap_or_else(|| home.join(".pi/agent"));
    if pi.is_dir() {
        let rendered = crate::skill::render()?;
        report(r, "pi extension", install::pi_extension(&pi)?);
        report(r, "pi skill", install::pi_skill(&pi, &rendered)?);
    } else {
        r.line(format!("pi extension: skipped, {} does not exist", pi.display()));
    }
    Ok(())
}

fn install_pre_push(r: &mut Reply, rvow: &Path) -> Result<()> {
    match store::find(&cwd()?, &store::home_dir()?) {
        Ok(Some(_)) => report(r, "pre-push gate", install::pre_push(&git::toplevel(&cwd()?)?, rvow)?),
        _ => r.line("pre-push gate: skipped, run rvow install inside a repository with an rvow tree"),
    }
    Ok(())
}
