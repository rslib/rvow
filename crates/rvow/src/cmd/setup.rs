use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rvow_core::change;
use rvow_core::error::{Error, Result};
use rvow_core::git;
use rvow_core::hook as hooks;
use rvow_core::install;
use rvow_core::lease;
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

/// Hooks never fail the harness: outside an rvow project they print nothing.
/// An internal error goes to stderr; before a tool call it also denies a tool
/// that writes, so a broken store cannot turn the checks off. Reads go on.
/// Every harness runs the Claude Code handlers: its call is read into
/// Claude's names first, and the answer is put into its own shape last.
pub fn hook(harness: Harness, event: HookEvent) -> ExitCode {
    let mut text = String::new();
    if std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).is_err() {
        return ExitCode::SUCCESS;
    }
    git::remember_answers();
    let mut input: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    if matches!(harness, Harness::Json) {
        neutral_input(&mut input);
    }
    let mut run = || -> Result<Option<serde_json::Value>> {
        // Timed from here, so a hook's duration includes finding the project.
        let start = std::time::Instant::now();
        // Without a home or a working folder there is no store to find, so no project to check.
        let Ok(home) = store::home_dir() else { return Ok(None) };
        let Ok(cwd) = input.get("cwd").and_then(|v| v.as_str()).map(PathBuf::from).map_or_else(cwd, Ok) else { return Ok(None) };
        if matches!(event, HookEvent::SessionStart) {
            // The skill and mod warnings are about Claude Code's and pi's files.
            let warnings = if matches!(harness, Harness::Claude) { skill_warnings()? } else { Vec::new() };
            return match store::find(&cwd, &home) {
                Ok(Some(p)) => {
                    bind_session_lease(&p, &cwd, &mut input);
                    let out = hooks::claude_session_start(Some(&p), &home, &input, &warnings);
                    count_duration(&p, stats::Hook::SessionStart, &input, start);
                    out
                }
                _ => hooks::claude_session_start(None, &home, &input, &warnings),
            };
        }
        let p = match store::find(&cwd, &home) {
            Ok(Some(p)) => p,
            Ok(None) => return Ok(None),
            // Outside a git repository, or before its first commit, there is no project to check.
            Err(e) if e.code == "no-commit" || git::toplevel(&cwd).is_err() => return Ok(None),
            Err(e) => return Err(e),
        };
        bind_session_lease(&p, &cwd, &mut input);
        let (counted, out) = match (harness, event) {
            (_, HookEvent::PreToolUse) => (Some(stats::Hook::PreToolUse), hooks::claude_pre_tool_use(&p, &input)),
            (_, HookEvent::PostToolUse) => (Some(stats::Hook::PostToolUse), hooks::claude_post_tool_use(&p, &input)),
            (_, HookEvent::SessionStart) => (None, hooks::claude_session_start(Some(&p), &home, &input, &[])),
            (_, HookEvent::Stop) => {
                // Both the stop answer and the re-prove read the living specs, so the hook loads them once.
                let living = hooks::stop_needs_specs(&input).then(|| change::living(&p).ok()).flatten();
                let out = match &living {
                    Some(living) => hooks::claude_stop_over(&p, &home, &input, living),
                    None => hooks::claude_stop(&p, &home, &input),
                };
                if matches!(out, Ok(None)) && input.get("agent_id").is_none() {
                    start_reprove(&p, &home, &cwd, living.as_deref());
                }
                (Some(stats::Hook::Stop), out)
            }
            (_, HookEvent::LeaseBind) => (Some(stats::Hook::LeaseBind), hooks::claude_lease_bind(&p, &input)),
            (_, HookEvent::LeaseEnd) => (Some(stats::Hook::LeaseEnd), hooks::claude_lease_end(&p, &input)),
            (_, HookEvent::View) => (None, view::view(&p, &home, &cwd)),
            (_, HookEvent::Status) => (None, hooks::status_line(&p, &home, &cwd).map(|s| s.map(|s| serde_json::json!({ "status": s })))),
        };
        if let Some(hook) = counted {
            count_duration(&p, hook, &input, start);
        }
        out
    };
    let out = match run() {
        Ok(out) => out,
        Err(e) => {
            eprintln!("rvow hook: {}: {}", e.code, e.message);
            if matches!(event, HookEvent::PreToolUse) { hooks::claude_pre_tool_use_failure(&input, &e) } else { None }
        }
    };
    if let Some(out) = answer(harness, event, out) {
        println!("{out}");
    }
    ExitCode::SUCCESS
}

/// Binds the session of a call to the live lease of the worktree the call
/// comes from, so the handlers below judge the session and its subagents by
/// that lease. Only rvow sets `lease_holder`, so a value in the input is
/// dropped. A failure is ignored: a hook never fails over a lease.
fn bind_session_lease(project: &store::Project, cwd: &Path, input: &mut serde_json::Value) {
    if let Some(fields) = input.as_object_mut() {
        fields.remove("lease_holder");
    }
    let Some(session) = input.get("session_id").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(String::from) else {
        return;
    };
    let Ok(top) = git::toplevel(cwd) else { return };
    if lease::bind_worktree(project, &top, &session).is_ok_and(|found| found.is_some()) {
        input["lease_holder"] = serde_json::json!(session);
    }
}

/// Reads a neutral call (`tool`, `input`) into the names the handlers read
/// (`tool_name`, `tool_input`). A call that already has those names stays.
fn neutral_input(input: &mut serde_json::Value) {
    let Some(call) = input.as_object_mut() else { return };
    for (from, to) in [("tool", "tool_name"), ("input", "tool_input")] {
        if !call.contains_key(to)
            && let Some(v) = call.get(from).cloned()
        {
            call.insert(to.to_string(), v);
        }
    }
}

/// The answer of a Claude Code handler in the shape `harness` reads.
fn answer(harness: Harness, event: HookEvent, out: Option<serde_json::Value>) -> Option<serde_json::Value> {
    use serde_json::json;
    let at = |v: &Option<serde_json::Value>, path: &str| v.as_ref().and_then(|v| v.pointer(path)).cloned();
    match harness {
        Harness::Claude => out,
        // Codex cannot ask, so a question for the user is a denial with its reason.
        Harness::Codex => out.map(|mut v| {
            if let Some(d) = v.pointer_mut("/hookSpecificOutput/permissionDecision")
                && d == "ask"
            {
                *d = json!("deny");
            }
            v
        }),
        Harness::Json => Some(match event {
            HookEvent::PreToolUse => match at(&out, "/hookSpecificOutput/permissionDecision") {
                Some(d) => json!({"decision": d, "reason": at(&out, "/hookSpecificOutput/permissionDecisionReason")}),
                None => json!({"decision": "allow"}),
            },
            HookEvent::PostToolUse | HookEvent::SessionStart => match at(&out, "/hookSpecificOutput/additionalContext") {
                Some(text) => json!({"context": text}),
                None => json!({}),
            },
            HookEvent::Stop => match at(&out, "/decision") {
                Some(d) => json!({"decision": d, "reason": at(&out, "/reason")}),
                None => json!({}),
            },
            HookEvent::Status | HookEvent::View | HookEvent::LeaseBind | HookEvent::LeaseEnd => out.unwrap_or_else(|| json!({})),
        }),
    }
}

/// Starts `rvow reprove` detached when a finished change has stale tasks, so
/// it runs while the person reads the answer. A failure here only costs the
/// head start: `rvow archive` proves the stale tasks itself.
fn start_reprove(p: &store::Project, home: &Path, cwd: &Path, living: Option<&[change::CapSpec]>) {
    if rvow_core::reprove::running(p) {
        return;
    }
    let ready = match living {
        Some(living) => rvow_core::reprove::ready_over(p, home, cwd, living),
        None => rvow_core::reprove::ready(p, home, cwd),
    };
    if !ready.is_ok_and(|r| !r.is_empty()) {
        return;
    }
    let (Ok(exe), Ok(top)) = (std::env::current_exe(), git::toplevel(cwd)) else {
        return;
    };
    let path = rvow_core::reprove::log_path(p);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(log) = std::fs::File::create(&path) else {
        return;
    };
    let Ok(err) = log.try_clone() else {
        return;
    };
    let mut c = std::process::Command::new(exe);
    c.arg("reprove").current_dir(top).stdin(std::process::Stdio::null()).stdout(log).stderr(err);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut c, 0);
    let _ = c.spawn();
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
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("rvow"));
    let pi = install::pi_dir(Some(&home));
    let stale: Vec<String> =
        install::stale_managed_files(&claude, &mod_dir, pi.as_deref(), &rendered).into_iter().map(|path| stale_message(&path)).collect();
    Ok(if stale.is_empty() { stale } else { stale.into_iter().chain([stale_hint(&exe)]).collect() })
}

/// The warning for one managed file that differs from what the running rvow writes. The brief cuts a
/// line at 240 chars, so a long path leaves no room for more.
fn stale_message(path: &Path) -> String {
    let what = if path.ends_with("SKILL.md") { "skill" } else { "file" };
    format!("rvow: stale or edited managed {what}. Please run rvow install. Your edits are kept. Path: {}", path.display())
}

/// The one line after the file warnings that names the running binary and the other way out.
fn stale_hint(exe: &Path) -> String {
    format!("rvow: this rvow is {}; if a newer rvow wrote those files, run cargo install first.", exe.display())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_fresh_home_messages_name_the_file_the_binary_and_both_fixes() {
        let file = stale_message(Path::new("/h/plugins/rvow/skills/rvow/SKILL.md"));
        assert!(
            file.contains("/h/plugins/rvow/skills/rvow/SKILL.md") && file.contains("skill") && file.contains("run rvow install"),
            "{file}"
        );
        let hint = stale_hint(Path::new("/bin/rvow"));
        assert!(hint.contains("/bin/rvow") && hint.contains("cargo install"), "{hint}");
    }
}
