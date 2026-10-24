use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rvow_core::error::{Error, Result};
use rvow_core::git;
use rvow_core::install;
use rvow_core::store::{self, Mode, Rebind};
use rvow_daemon::dispatch::env_dir;

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

/// One hook call: read the call, answer it in process, print what it answers,
/// then record the harness and start an engine when none runs.
///
/// The engine work comes last, after the answer is on its way, so the harness
/// never waits on it. A read error prints nothing; a hook never fails the harness.
pub fn hook(harness: Harness, event: HookEvent) -> ExitCode {
    let mut text = String::new();
    if std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).is_ok()
        && let Some(out) = rvow_daemon::dispatch::answer_hook(harness.into(), event.into(), &text)
    {
        println!("{out}");
    }
    let _ = std::io::Write::flush(&mut std::io::stdout());
    crate::engine_client::after_answer(event);
    ExitCode::SUCCESS
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

pub fn install(mode: ClaudeMode, scope: Scope, all: bool) -> Result<Reply> {
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
    if all {
        install_all_gates(&mut r, &rvow)?;
    } else {
        install_pre_push(&mut r, &rvow)?;
    }
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
        Ok(Some(_)) => {
            let top = git::toplevel(&cwd()?)?;
            report(r, "pre-push gate", install::pre_push(&top, rvow)?);
            remember_gate(&top)?;
        }
        _ => r.line("pre-push gate: skipped, run rvow install inside a repository with an rvow tree"),
    }
    Ok(())
}

/// Remembers the git common dir of the work tree at `top`, where the gate that
/// `rvow install` just wrote lives, so `rvow install --all` finds it again.
fn remember_gate(top: &Path) -> Result<()> {
    let common = PathBuf::from(git::run(top, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?);
    if common.is_absolute() {
        store::remember_gate(&store::home_dir()?, &common);
    }
    Ok(())
}

/// Refreshes every gate that `rvow install` remembered, naming each repository
/// it leaves alone, then lists the repositories of the project index that have
/// no gate.
fn install_all_gates(r: &mut Reply, rvow: &Path) -> Result<()> {
    let home = store::home_dir()?;
    let remembered = store::gate_repos(&home);
    let (mut refreshed, mut skipped) = (0, 0);
    for common in &remembered {
        if refresh_gate(r, common, rvow)? {
            refreshed += 1;
        } else {
            skipped += 1;
        }
    }
    for common in store::indexed_repos(&home) {
        if remembered.contains(&common) {
            continue;
        }
        // A gate from an install made before the record existed is refreshed and recorded.
        if install::has_gate(&common) {
            if refresh_gate(r, &common, rvow)? {
                store::remember_gate(&home, &common);
                refreshed += 1;
            } else {
                skipped += 1;
            }
            continue;
        }
        let Some(top) = install::work_tree(&common) else {
            continue;
        };
        r.line(format!("pre-push gate: {} has none; run: cd {} && rvow install", top.display(), top.display()));
    }
    r.line(format!("pre-push gates: {refreshed} refreshed, {skipped} skipped"));
    Ok(())
}

/// Refreshes the gate of `common`, or names why it leaves the repository as it
/// is. Returns whether it refreshed the gate.
fn refresh_gate(r: &mut Reply, common: &Path, rvow: &Path) -> Result<bool> {
    let repo = install::work_tree(common).unwrap_or(common);
    if !common.is_dir() {
        r.line(format!("pre-push gate: {} is gone; skipped", repo.display()));
        return Ok(false);
    }
    if !install::has_gate(common) {
        r.line(format!("pre-push gate: {} has no rvow gate any more; skipped", repo.display()));
        return Ok(false);
    }
    let Some(top) = install::work_tree(common) else {
        r.line(format!("pre-push gate: {} is not a work tree; skipped", common.display()));
        return Ok(false);
    };
    // With another hook manager, pre_push still re-pins the recorded binary and leaves the hook to the user.
    let outcome = install::pre_push(top, rvow)?;
    let refreshed = !matches!(outcome, install::Outcome::AddLine { .. });
    if !refreshed {
        r.line(format!("pre-push gate: another hook manager owns the hook of {}", top.display()));
    }
    report(r, "pre-push gate", outcome);
    Ok(refreshed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvow_daemon::dispatch::{stale_hint, stale_message};

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
