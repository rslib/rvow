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

fn report_removal(r: &mut Reply, what: &str, o: install::Removal) {
    r.line(match o {
        install::Removal::Removed(p) => format!("{what}: removed {}", p.display()),
        install::Removal::Absent(p) => format!("{what}: none at {}", p.display()),
        install::Removal::KeptEdit(p) => format!("{what}: kept {}, which you edited; delete it by hand", p.display()),
        install::Removal::Restored { path, prev } => {
            format!("{what}: removed the gate and moved your hook back from {} to {}", prev.display(), path.display())
        }
    });
}

pub fn uninstall(all: bool, purge: bool) -> Result<Reply> {
    let mut r = Reply::new();
    let rvow_home = store::home_dir()?;
    let home = env_dir("HOME").ok_or_else(|| Error::new("no-home", "HOME is not set", "set HOME"))?;
    let claude = env_dir("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude"));
    let skill = crate::skill::render()?;
    uninstall_mod(&mut r, &rvow_home)?;
    uninstall_settings(&mut r, "Claude Code hooks", &claude.join("settings.json"))?;
    if let Ok(top) = git::toplevel(&cwd()?) {
        let project = top.join(".claude/settings.json");
        if project.is_file() {
            uninstall_settings(&mut r, "Claude Code project hooks", &project)?;
        }
    }
    report_removal(&mut r, "skill", install::remove_skill(&claude, &skill)?);
    let pi = install::pi_dir(Some(&home)).unwrap_or_else(|| home.join(".pi/agent"));
    report_removal(&mut r, "pi extension", install::remove_pi_extension(&pi)?);
    report_removal(&mut r, "pi skill", install::remove_skill(&pi, &skill)?);
    // The purge deletes the list of gates, and a gate pins the binary, so a gate left behind would fail every push.
    if all || purge {
        uninstall_all_gates(&mut r, &rvow_home);
    } else {
        match git::toplevel(&cwd()?) {
            Ok(top) => uninstall_gate(&mut r, &rvow_home, &top)?,
            Err(_) => r.line("pre-push gate: skipped, not in a git repository"),
        }
        let left = store::gate_repos(&rvow_home).len();
        if left > 0 {
            r.line(format!("pre-push gates: {left} other repositories still have one; run rvow uninstall --all to remove them"));
        }
    }
    if purge {
        purge_home(&mut r, &rvow_home, &home)?;
    } else {
        r.line(format!("plans: kept in {}; run rvow uninstall --purge to delete them", rvow_home.display()));
    }
    let exe = std::env::current_exe().and_then(|p| p.canonicalize()).map_err(|e| Error::io(Path::new("rvow"), e))?;
    let cargo_bin = env_dir("CARGO_HOME").unwrap_or_else(|| home.join(".cargo")).join("bin");
    if exe.parent().is_some_and(|dir| dir.canonicalize().is_ok_and(|d| cargo_bin.canonicalize().is_ok_and(|c| c == d))) {
        r.line("binary: to remove it, run: cargo uninstall rvow");
    } else {
        r.line(format!("binary: to remove it, run: rm {}", install::shell_quote(&exe.display().to_string())));
    }
    let text = std::mem::take(&mut r.text);
    Ok(Reply::lines(text))
}

fn uninstall_mod(r: &mut Reply, rvow_home: &Path) -> Result<()> {
    let dir = install::mod_dir().ok_or_else(|| Error::new("no-home", "HOME is not set", "set HOME"))?;
    let old = rvow_home.join(install::MARKETPLACE_DIR);
    let installed = [&dir, &old].iter().any(|d| d.join(".claude-plugin/marketplace.json").is_file());
    // Claude Code is asked only when a marketplace is on disk, so a settings install never starts claude.
    if installed {
        match which("claude") {
            Some(claude) => match install::unregister_claude_mod(&claude) {
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
                r.line("Claude Code: claude is not on PATH; to unload the mod, run once:");
                r.line("  claude plugin uninstall rvow@rvow");
                r.line("  claude plugin marketplace remove rvow");
            }
        }
    }
    report_removal(r, "Claude Code mod", install::remove_claude_mod(&dir)?);
    if old != dir
        && let install::Removal::Removed(p) = install::remove_claude_mod(&old)?
    {
        r.line(format!("old Claude Code mod: removed {}", p.display()));
    }
    Ok(())
}

fn uninstall_settings(r: &mut Reply, what: &str, path: &Path) -> Result<()> {
    match install::remove_claude_settings(path)? {
        install::Outcome::Written(p) => r.line(format!("{what}: removed the rvow hooks from {}", p.display())),
        _ if std::fs::read_to_string(path).is_ok_and(|t| t.contains("hook claude")) => {
            r.line(format!("{what}: {} is not valid JSON; remove its rvow hooks by hand", path.display()));
        }
        _ => r.line(format!("{what}: none in {}", path.display())),
    }
    Ok(())
}

fn uninstall_gate(r: &mut Reply, rvow_home: &Path, top: &Path) -> Result<()> {
    let common = PathBuf::from(git::run(top, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?);
    report_removal(r, "pre-push gate", install::remove_pre_push(&common)?);
    if let Some(path) = install::hook_manager_gate(top) {
        r.line(format!("pre-push gate: another hook manager owns {}; delete its rvow gate line", path.display()));
    }
    store::forget_gate(rvow_home, &common);
    Ok(())
}

/// Removes every gate that `rvow install` remembered, and the gates of
/// indexed repositories that an install before the record wrote.
fn uninstall_all_gates(r: &mut Reply, rvow_home: &Path) {
    let mut commons = store::gate_repos(rvow_home);
    for common in store::indexed_repos(rvow_home) {
        if !commons.contains(&common) && install::has_gate(&common) {
            commons.push(common);
        }
    }
    let (mut removed, mut failed) = (0, 0);
    for common in &commons {
        let repo = install::work_tree(common).unwrap_or(common);
        if !common.is_dir() {
            r.line(format!("pre-push gate: {} is gone; skipped", repo.display()));
            store::forget_gate(rvow_home, common);
            continue;
        }
        // One broken repository must not stop the others, so its error becomes a line.
        match install::remove_pre_push(common) {
            Ok(o) => {
                if matches!(o, install::Removal::Removed(_) | install::Removal::Restored { .. }) {
                    removed += 1;
                }
                report_removal(r, "pre-push gate", o);
                store::forget_gate(rvow_home, common);
            }
            Err(e) => {
                failed += 1;
                r.line(format!("pre-push gate: {}: {}", repo.display(), e.message));
            }
        }
        if let Some(path) = install::work_tree(common).and_then(install::hook_manager_gate) {
            r.line(format!("pre-push gate: another hook manager owns {}; delete its rvow gate line", path.display()));
        }
    }
    r.line(format!("pre-push gates: {removed} removed, {failed} failed"));
}

fn purge_home(r: &mut Reply, rvow_home: &Path, home: &Path) -> Result<()> {
    let real = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    if !rvow_home.is_absolute() || real(home).starts_with(real(rvow_home)) {
        return Err(Error::new(
            "purge",
            format!("the rvow home {} is relative or holds your home folder {}", rvow_home.display(), home.display()),
            "delete the rvow files in it by hand",
        ));
    }
    match std::fs::remove_dir_all(rvow_home) {
        Ok(()) => r.line(format!("plans: deleted {}", rvow_home.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => r.line(format!("plans: none at {}", rvow_home.display())),
        Err(e) => return Err(Error::io(rvow_home, e)),
    }
    // The pointer stays when it names another home, which still holds plans.
    if let Some(pointer) = store::home_pointer()
        && std::fs::read_to_string(&pointer).is_ok_and(|t| Path::new(t.trim()) == rvow_home)
    {
        match std::fs::remove_file(&pointer) {
            Ok(()) => {
                r.line(format!("home pointer: removed {}", pointer.display()));
                if let Some(dir) = pointer.parent() {
                    let _ = std::fs::remove_dir(dir);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io(&pointer, e)),
        }
    }
    Ok(())
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
