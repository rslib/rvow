use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rvow_core::error::{Error, Result};
use rvow_core::git;
use rvow_core::hook as hooks;
use rvow_core::install;
use rvow_core::stats;
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
        // Timed from here, so a hook's duration includes finding the project.
        let start = std::time::Instant::now();
        let home = store::home_dir()?;
        let cwd = input.get("cwd").and_then(|v| v.as_str()).map(PathBuf::from).map_or_else(cwd, Ok)?;
        if matches!((harness, event), (Harness::Claude, HookEvent::SessionStart)) {
            let warnings = skill_warnings()?;
            return match store::find(&cwd, &home) {
                Ok(Some(p)) => {
                    let out = hooks::claude_session_start(Some(&p), &home, &input, &warnings);
                    count_duration(&p, stats::Hook::SessionStart, &input, start);
                    out
                }
                _ => hooks::claude_session_start(None, &home, &input, &warnings),
            };
        }
        let Ok(Some(p)) = store::find(&cwd, &home) else {
            return Ok(None);
        };
        let (counted, out) = match (harness, event) {
            (Harness::Claude, HookEvent::PreToolUse) => (Some(stats::Hook::PreToolUse), hooks::claude_pre_tool_use(&p, &input)),
            (Harness::Claude, HookEvent::PostToolUse) => (Some(stats::Hook::PostToolUse), hooks::claude_post_tool_use(&p, &input)),
            (Harness::Claude, HookEvent::SessionStart) => (None, hooks::claude_session_start(Some(&p), &home, &input, &[])),
            (Harness::Claude, HookEvent::Stop) => (Some(stats::Hook::Stop), hooks::claude_stop(&p, &home, &input)),
            (Harness::Claude, HookEvent::LeaseBind) => (Some(stats::Hook::LeaseBind), hooks::claude_lease_bind(&p, &input)),
            (Harness::Claude, HookEvent::LeaseEnd) => (Some(stats::Hook::LeaseEnd), hooks::claude_lease_end(&p, &input)),
            (Harness::Claude, HookEvent::View) => (None, view::view(&p, &home, &cwd)),
            (Harness::Claude, HookEvent::Status) => {
                (None, hooks::status_line(&p, &home, &cwd).map(|s| s.map(|s| serde_json::json!({ "status": s }))))
            }
        };
        if let Some(hook) = counted {
            count_duration(&p, hook, &input, start);
        }
        out
    };
    match run() {
        Ok(Some(out)) => println!("{out}"),
        Ok(None) => {}
        Err(e) => eprintln!("rvow hook: {}: {}", e.code, e.message),
    }
    ExitCode::SUCCESS
}

/// Counts one hook call with its duration, the session id and nothing else.
fn count_duration(p: &store::Project, hook: stats::Hook, input: &serde_json::Value, start: std::time::Instant) {
    let ms = start.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    stats::record_duration(p, hook, input.get("session_id").and_then(|v| v.as_str()), ms);
}

/// One warning per stale or edited file that `rvow install` manages.
fn skill_warnings() -> Result<Vec<String>> {
    let (Some(home), Some(mod_dir)) = (env_dir("HOME"), install::mod_dir()) else {
        return Ok(Vec::new());
    };
    let claude = env_dir("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude"));
    let rendered = crate::skill::render()?;
    let pi = install::pi_dir(Some(&home));
    Ok(install::stale_managed_files(&claude, &mod_dir, pi.as_deref(), &rendered)
        .into_iter()
        .map(|path| {
            let what = if path.ends_with("SKILL.md") { "skill" } else { "file" };
            format!("rvow: stale or edited managed {what}. Please run rvow install. Your edits are kept. Path: {}", path.display())
        })
        .collect())
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
    let dir = install::mod_dir().ok_or_else(|| Error::new("no-home", "HOME is not set", "set HOME"))?;
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
    if let Some(old) = install::remove_old_mod(&store::home_dir()?.join(install::MARKETPLACE_DIR), &dir)? {
        r.line(format!("old Claude Code mod: removed {}", old.display()));
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
