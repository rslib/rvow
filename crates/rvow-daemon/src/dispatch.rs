//! The hook body: one call in, the text `rvow hook` prints on stdout out.
//!
//! Every harness runs the Claude Code handlers: its call is read into Claude's
//! names first, and the answer is put into its own shape last.

use std::path::{Path, PathBuf};

use rvow_core::change;
use rvow_core::error::{Error, Result};
use rvow_core::install;
use rvow_core::lease;
use rvow_core::stats;
use rvow_core::store;
use rvow_core::view;
use rvow_git::git;
use rvow_hook::hook as hooks;
use rvow_proto::{Harness, HookEvent};

/// The answer to one hook call: the text to print, without its newline, or
/// `None` when the hook prints nothing. `text` is the raw stdin of the call.
///
/// Hooks never fail the harness: outside an rvow project they print nothing.
/// An internal error goes to stderr; before a tool call it also denies a tool
/// that writes, so a broken store cannot turn the checks off. Reads go on.
pub fn answer_hook(harness: Harness, event: HookEvent, text: &str) -> Option<String> {
    answer_hook_at(None, harness, event, text)
}

/// `answer_hook` for a caller that keeps an rvow home: `Some(home)` overrides
/// `store::home_dir()`, and `None` reads it as the CLI does.
pub fn answer_hook_at(home: Option<&Path>, harness: Harness, event: HookEvent, text: &str) -> Option<String> {
    git::remember_answers();
    let mut input: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    if matches!(harness, Harness::Json) {
        neutral_input(&mut input);
    }
    let mut run = || -> Result<Option<serde_json::Value>> {
        // Timed from here, so a hook's duration includes finding the project.
        let start = std::time::Instant::now();
        let mut steps = stats::Timings::default();
        // Without a home or a working folder there is no store to find, so no project to check.
        let home = match home {
            Some(home) => home.to_path_buf(),
            None => match store::home_dir() {
                Ok(home) => home,
                Err(_) => return Ok(None),
            },
        };
        let Ok(cwd) = input.get("cwd").and_then(|v| v.as_str()).map(PathBuf::from).map_or_else(cwd, Ok) else { return Ok(None) };
        if matches!(event, HookEvent::SessionStart) {
            // The skill and mod warnings are about Claude Code's and pi's files.
            let warnings = if matches!(harness, Harness::Claude) { skill_warnings()? } else { Vec::new() };
            return match timed(&mut steps, stats::Step::Store, || store::find(&cwd, &home)) {
                Ok(Some(p)) => {
                    timed(&mut steps, stats::Step::Lease, || bind_session_lease(&p, &cwd, &mut input));
                    let out = timed(&mut steps, stats::Step::Brief, || hooks::claude_session_start(Some(&p), &home, &input, &warnings));
                    count_duration(&p, stats::Hook::SessionStart, &input, start, &steps);
                    out
                }
                _ => hooks::claude_session_start(None, &home, &input, &warnings),
            };
        }
        let p = match timed(&mut steps, stats::Step::Store, || store::find(&cwd, &home)) {
            Ok(Some(p)) => p,
            Ok(None) => return Ok(None),
            // Outside a git repository, or before its first commit, there is no project to check.
            Err(e) if e.code == "no-commit" || git::toplevel(&cwd).is_err() => return Ok(None),
            Err(e) => return Err(e),
        };
        timed(&mut steps, stats::Step::Lease, || bind_session_lease(&p, &cwd, &mut input));
        let (counted, out) = match (harness, event) {
            (_, HookEvent::PreToolUse) => {
                (Some(stats::Hook::PreToolUse), timed(&mut steps, stats::Step::Rules, || hooks::claude_pre_tool_use(&p, &input)))
            }
            (_, HookEvent::PostToolUse) => {
                (Some(stats::Hook::PostToolUse), timed(&mut steps, stats::Step::Rules, || hooks::claude_post_tool_use(&p, &input)))
            }
            (_, HookEvent::SessionStart) => (None, hooks::claude_session_start(Some(&p), &home, &input, &[])),
            (_, HookEvent::Stop) => {
                // Both the stop answer and the re-prove read the living specs, so the hook loads them once.
                let living =
                    if hooks::stop_needs_specs(&input) { timed(&mut steps, stats::Step::Specs, || change::living(&p).ok()) } else { None };
                let out = timed(&mut steps, stats::Step::Rules, || match &living {
                    Some(living) => hooks::claude_stop_over(&p, &home, &input, living),
                    None => hooks::claude_stop(&p, &home, &input),
                });
                if matches!(out, Ok(None)) && input.get("agent_id").is_none() {
                    timed(&mut steps, stats::Step::Reprove, || start_reprove(&p, &home, &cwd, living.as_deref()));
                }
                (Some(stats::Hook::Stop), out)
            }
            (_, HookEvent::LeaseBind) => {
                (Some(stats::Hook::LeaseBind), timed(&mut steps, stats::Step::Rules, || hooks::claude_lease_bind(&p, &input)))
            }
            (_, HookEvent::LeaseEnd) => {
                (Some(stats::Hook::LeaseEnd), timed(&mut steps, stats::Step::Rules, || hooks::claude_lease_end(&p, &input)))
            }
            (_, HookEvent::View) => (None, view::view(&p, &home, &cwd)),
            (_, HookEvent::Status) => (None, hooks::status_line(&p, &home, &cwd).map(|s| s.map(|s| serde_json::json!({ "status": s })))),
        };
        if let Some(hook) = counted {
            count_duration(&p, hook, &input, start, &steps);
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
    answer(harness, event, out).map(|out| out.to_string())
}

/// The working folder of this process.
fn cwd() -> Result<PathBuf> {
    std::env::current_dir().map_err(|e| Error::io(Path::new("."), e))
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

/// Counts one hook call with its duration, the ms each step took, the session id and nothing else.
fn count_duration(p: &store::Project, hook: stats::Hook, input: &serde_json::Value, start: std::time::Instant, steps: &stats::Timings) {
    let ms = start.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    stats::record_duration(p, hook, input.get("session_id").and_then(|v| v.as_str()), ms, steps);
}

/// Runs `f` and adds the ms it took to the `step` of `steps`, so a step is
/// timed where it runs and nothing else about it changes. A step timed inside
/// `f`, such as the snapshot, moves out of `step` into its own.
fn timed<T>(steps: &mut stats::Timings, step: stats::Step, f: impl FnOnce() -> T) -> T {
    let _ = stats::take_inner();
    let start = std::time::Instant::now();
    let out = f();
    let ms: u64 = start.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    let inner = stats::take_inner();
    steps.add(step, ms.saturating_sub(inner.total()));
    steps.merge(&inner);
    out
}

/// One warning per stale or edited file that `rvow install` manages.
fn skill_warnings() -> Result<Vec<String>> {
    let (Some(home), Some(mod_dir)) = (env_dir("HOME"), install::mod_dir()) else {
        return Ok(Vec::new());
    };
    let claude = env_dir("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude"));
    let rendered = fill_skill(SKILL_TEMPLATE, &|_| None)?;
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("rvow"));
    let pi = install::pi_dir(Some(&home));
    let stale: Vec<String> =
        install::stale_managed_files(&claude, &mod_dir, pi.as_deref(), &rendered).into_iter().map(|path| stale_message(&path)).collect();
    Ok(if stale.is_empty() { stale } else { stale.into_iter().chain([stale_hint(&exe)]).collect() })
}

/// The skill template that `rvow install` renders and the session start compares.
pub const SKILL_TEMPLATE: &str = include_str!("../../../skills/rvow/SKILL.md.tmpl");

/// Fills each `{{cmd:<args>}}` of `template` with `` `rvow <args>` ``. `missing`
/// says why a command line is not a real command, or `None`; the CLI checks
/// against its command tree, and the session start checks nothing.
pub fn fill_skill(template: &str, missing: &dyn Fn(&str) -> Option<String>) -> Result<String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find("}}").ok_or_else(|| Error::internal("the skill template has an unclosed {{"))?;
        let Some(args) = after[..end].strip_prefix("cmd:") else {
            return Err(Error::internal(format!("the skill template has an unknown placeholder {{{{{}}}}}", &after[..end])));
        };
        let line = format!("rvow {args}");
        if let Some(why) = missing(&line) {
            return Err(Error::internal(format!("the skill template names a missing command: {why}")));
        }
        out.push_str(&format!("`{line}`"));
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The warning for one managed file that differs from what the running rvow writes. The brief cuts a
/// line at 240 chars, so a long path leaves no room for more.
pub fn stale_message(path: &Path) -> String {
    let what = if path.ends_with("SKILL.md") { "skill" } else { "file" };
    format!("rvow: stale or edited managed {what}. Please run rvow install. Your edits are kept. Path: {}", path.display())
}

/// The one line after the file warnings that names the running binary and the other way out.
pub fn stale_hint(exe: &Path) -> String {
    format!("rvow: this rvow is {}; if a newer rvow wrote those files, run cargo install first.", exe.display())
}

/// A variable that names a folder, or `None` when it is unset or empty.
pub fn env_dir(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvow_core::git::testrepo::{TempDir, repo_with_commit};
    use rvow_core::vow;
    use serde_json::json;

    /// An edit of `out.txt` in `repo` by the session `s1`.
    fn edit(repo: &Path) -> String {
        json!({
            "cwd": repo.to_str().unwrap(),
            "session_id": "s1",
            "tool_name": "Edit",
            "tool_input": {"file_path": repo.join("out.txt").to_str().unwrap(), "old_string": "a", "new_string": "ok"},
        })
        .to_string()
    }

    /// The answer of one PreToolUse call, or `None` when the call goes through.
    fn pre_tool_use(home: &Path, call: &str) -> Option<String> {
        answer_hook_at(Some(home), Harness::Claude, HookEvent::PreToolUse, call)
    }

    #[test]
    fn engine_context_dispatch_sees_a_plan_accepted_between_calls() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "r");
        let p = store::init(&repo, &home, store::Mode::Home).unwrap();
        let c = change::new(&p, "T", "main", true).unwrap();
        std::fs::write(
            c.path.join("tasks.md"),
            "- TSK-new1 Write ok\n  verify: grep -q ok out.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: out.txt\n",
        )
        .unwrap();
        let call = edit(&repo);

        let held = pre_tool_use(&home, &call).unwrap_or_else(|| panic!("an edit before the plan is accepted is denied"));
        assert!(held.contains("\"permissionDecision\":\"deny\""), "{held}");

        let prepared = vow::prepare(&p, &home, c, &repo, &mut std::io::sink()).unwrap();
        vow::commit(&p, prepared).unwrap();

        if let Some(out) = pre_tool_use(&home, &call) {
            assert!(!out.contains("\"permissionDecision\":\"deny\""), "the accepted plan lets the edit through: {out}");
        }
    }
}
