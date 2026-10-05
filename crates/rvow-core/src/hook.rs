//! The logic behind the harness hooks. Adapters pass the harness's JSON in and
//! print what comes back; every rule lives here.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};

use crate::amend;
use crate::brief;
use crate::change::{self, LEDGER};
use crate::config::{self, ENFORCEMENT};
use crate::drift;
use crate::error::Result;
use crate::fingerprint;
use crate::gate::cache::CACHE_FILE;
use crate::git;
use crate::lease::{self, LEASE_FILE};
use crate::ledger::Kind;
use crate::proof::{self, Now, State};
use crate::stats::{self, Hook, Outcome, STATS_FILE};
use crate::store::{IDS_FILE, PROJECT_FILE, Project};

mod rule;
mod shell;
mod stop_state;
pub use rule::{Denial, Rule};
#[cfg(test)]
use shell::bash_enforcement_denial;
pub use shell::{bash_denial, bash_rule, script_denial, script_rule};
use shell::{bash_enforcement_rule, bash_secret_rule};

use rule::denial;

/// The absolute, symlink-free form of `path`, also when the file does not
/// exist yet: the nearest existing ancestor is resolved and the rest appended.
pub(crate) fn real_path(path: &Path, cwd: &Path) -> PathBuf {
    let abs = if path.is_absolute() { path.to_path_buf() } else { cwd.join(path) };
    let mut clean = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::ParentDir => {
                clean.pop();
            }
            Component::CurDir => {}
            other => clean.push(other),
        }
    }
    let mut existing = clean.clone();
    let mut rest = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(name) => rest.push(name.to_os_string()),
            None => break,
        }
        if !existing.pop() {
            break;
        }
    }
    let mut out = existing.canonicalize().unwrap_or(existing);
    for name in rest.into_iter().rev() {
        out.push(name);
    }
    out
}

/// macOS file systems ignore case by default, so `Tests/T.sh` names
/// `tests/t.sh`.
pub(crate) fn key(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    if cfg!(target_os = "macos") { s.to_lowercase() } else { s }
}

/// The argument names that carry a file path for a writing tool.
const FILE_ARGS: &[&str] =
    &["file_path", "notebook_path", "path", "filename", "file", "dest", "destination", "target", "to", "output", "out"];

/// The files a tool call names: path arguments, and the file headers of a
/// patch text (`*** Update File: x`, `+++ b/x`).
fn file_args(input: &Value) -> Vec<String> {
    let mut out: Vec<String> = FILE_ARGS.iter().filter_map(|k| input.get(k).and_then(Value::as_str)).map(String::from).collect();
    for text in input.as_object().into_iter().flat_map(|o| o.values()).filter_map(Value::as_str).filter(|t| t.contains('\n')) {
        for line in text.lines() {
            let named = ["*** Add File: ", "*** Update File: ", "*** Delete File: ", "*** Move to: "]
                .iter()
                .find_map(|p| line.strip_prefix(p))
                .or_else(|| line.strip_prefix("+++ b/"));
            out.extend(named.map(|p| p.trim().to_string()));
        }
    }
    out
}

/// Whether a tool changes files. The built-in editors do, and so does any
/// other tool unless its name says it only reads; an unknown name that
/// carries a file argument counts as a writer.
fn writes_files(tool: &str) -> bool {
    if matches!(tool, "Edit" | "Write" | "MultiEdit" | "NotebookEdit") {
        return true;
    }
    if matches!(tool, "Read" | "Grep" | "Glob" | "LS" | "NotebookRead" | "WebFetch" | "WebSearch" | "TodoWrite" | "Task" | "Agent") {
        return false;
    }
    let name = tool.rsplit("__").next().unwrap_or(tool).to_lowercase();
    let says = |words: &[&str]| words.iter().any(|w| name.contains(w));
    if says(&[
        "write", "edit", "create", "save", "put", "move", "rename", "copy", "delete", "remove", "patch", "append", "update", "replace",
        "apply", "insert", "mkdir", "upload", "touch", "truncate",
    ]) {
        return true;
    }
    !says(&[
        "read", "get", "list", "search", "find", "view", "show", "stat", "fetch", "query", "ls", "cat", "grep", "glob", "info", "describe",
        "open",
    ])
}

/// The directory that holds the hidden checks, which the implementing agent
/// may not read or write.
fn secret_paths() -> Vec<PathBuf> {
    crate::store::home_dir().map(|h| vec![h.join("hidden-checks")]).unwrap_or_default()
}

/// The allowed-signers file that signed confirmations are checked against, as
/// the configured path resolves from `top`.
fn signers_path(project: &Project, top: &Path) -> Option<PathBuf> {
    let home = crate::store::home_dir().ok()?;
    let config = config::load(project, &home).ok()?;
    (!config.allowed_signers.is_empty()).then(|| top.join(&config.allowed_signers))
}

/// Files outside any repository that enforce rvow: the installed mod, Claude
/// Code's plugins and settings, the pi extension and the signing trust file.
fn enforcement_paths(trust: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(home) = crate::store::home_dir() {
        out.push(home.join(crate::install::MARKETPLACE_DIR));
    }
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from);
    if let Some(h) = &home {
        let claude = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|c| !c.is_empty()).map_or_else(|| h.join(".claude"), PathBuf::from);
        out.push(claude.join("plugins"));
        out.push(claude.join("settings.json"));
        out.push(claude.join("settings.local.json"));
    }
    if let Some(pi) = crate::install::pi_dir(home.as_deref()) {
        out.push(pi.join("extensions/rvow.ts"));
        out.push(pi.join("extensions/rvow"));
    }
    out.extend(trust.map(Path::to_path_buf));
    out
}

/// Why an edit of `file` must not happen, or `None`.
fn edit_rule(project: &Project, top: &Path, cwd: &Path, file: &str) -> Result<Option<Denial>> {
    let target = key(&real_path(Path::new(file), cwd));
    let trust = signers_path(project, top);
    for p in enforcement_paths(trust.as_deref()) {
        let p = key(&real_path(&p, cwd));
        if target == p || target.starts_with(&format!("{p}/")) {
            return Ok(denial(
                Rule::EnforcementFile,
                format!(
                    "{p} enforces rvow; changing it is for the user, so only the user edits it. If the plan needs it, run rvow block <task> --reason \"...\""
                ),
            ));
        }
    }
    let root = key(&real_path(&project.root, cwd));
    let top_key = key(&real_path(top, cwd));
    let name = Path::new(&target).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if target.starts_with(&format!("{root}/"))
        && (name == LEDGER
            || name == IDS_FILE
            || name == PROJECT_FILE
            || name == crate::record::RECORD_FILE
            || target.starts_with(&format!("{root}/objects/")))
    {
        return Ok(denial(
            Rule::Ledger,
            format!(
                "{name} is written only by rvow; run `rvow show <change>` to read it, or `rvow amend --reason \"...\"` to change the plan"
            ),
        ));
    }
    if target.starts_with(&format!("{root}/archive/")) {
        return Ok(denial(
            Rule::History,
            "an archived change is history and does not change; run `rvow new \"<title>\"` to start a new change".into(),
        ));
    }
    if target.starts_with(&format!("{root}/specs/")) {
        return Ok(denial(
            Rule::History,
            "living specs change only through a change and rvow archive; run rvow guide spec to see how".into(),
        ));
    }
    if target.starts_with(&format!("{top_key}/.git/")) {
        return Ok(denial(
            Rule::GitHooks,
            ".git/ is not edited by hand; the hooks there run the gate. Use git commands, and run rvow install for the hooks".into(),
        ));
    }
    let Some(rel) = target.strip_prefix(&format!("{top_key}/")) else {
        return Ok(None);
    };
    if !target.starts_with(&format!("{root}/"))
        && let Some(id) = unaccepted_plan(project, top)?
    {
        return Ok(denial(
            Rule::UnacceptedPlan,
            format!(
                "the plan {id} is not accepted yet; finish it, run rvow check, then ask the user to run /rvow-accept {id} (or ! rvow accept {id}) before you edit code"
            ),
        ));
    }
    if ENFORCEMENT.iter().any(|e| key(Path::new(e)) == rel) {
        return Ok(denial(
            Rule::EnforcementFile,
            format!(
                "{rel} configures the hooks that enforce the plan; changing it is for the user, so only the user edits it. If the plan needs it, run rvow block <task> --reason \"...\""
            ),
        ));
    }
    if protected(project, top)?.contains(rel) && !unlocked(project, top, rel)? {
        return Ok(denial(
            Rule::ProtectedTest,
            format!(
                "{rel} is a protected test or test setup file; to change it, run rvow amend --unlock {rel} --reason \"...\" and ask the user to confirm"
            ),
        ));
    }
    if !target.starts_with(&format!("{root}/"))
        && let Some(files) = active_scope(project, top)?
        && !files.is_match(rel)
        && git::run(top, &["check-ignore", "-q", "--", rel]).is_err()
    {
        return Ok(denial(
            Rule::OutsideScope,
            format!(
                "{rel} is outside the files: of every task in the active change; run rvow amend --reason \"...\" after adding it to a task, and ask the user to confirm"
            ),
        ));
    }
    Ok(None)
}

/// The `files:` globs of every task of the active changes on this branch, or
/// `None` when the branch has no active change.
fn active_scope(project: &Project, top: &Path) -> Result<Option<globset::GlobSet>> {
    let (living, _) = change::load_living(project)?;
    let branch = git::branch(top)?.unwrap_or_default();
    let active = proof::active(project, &living)?;
    let mine = proof::on_branch(&active, &branch);
    if mine.is_empty() {
        return Ok(None);
    }
    let globs: Vec<String> = mine.iter().flat_map(|c| c.loaded.tasks.iter().flat_map(|t| t.files.clone())).collect();
    // The edited path arrives lowercased by `key` on macOS, so the globs must
    // match it in the same form. macOS file systems ignore case by default.
    let globs = if cfg!(target_os = "macos") { globs.iter().map(|g| g.to_lowercase()).collect() } else { globs };
    fingerprint::globset(&globs).map(Some)
}

/// The path of `file` relative to the repository top, in the form the
/// protected set uses, or `None` outside the repository.
pub(crate) fn rel_path(file: &Path, cwd: &Path, top: &Path) -> Option<String> {
    let target = key(&real_path(file, cwd));
    let top = key(&real_path(top, cwd));
    target.strip_prefix(&format!("{top}/")).map(String::from)
}

/// Whether the user confirmed an unlock of `rel` and no proof of a task whose
/// files match it has happened since.
fn unlocked(project: &Project, top: &Path, rel: &str) -> Result<bool> {
    let (living, _) = change::load_living(project)?;
    let branch = git::branch(top)?.unwrap_or_default();
    let mut open = false;
    for c in proof::active(project, &living)?.iter().filter(|c| c.branch() == Some(branch.as_str())) {
        let unlock_ids: BTreeSet<&str> = c
            .events
            .iter()
            .filter(|e| e.kind == Kind::Amend && e.data.get("unlock").and_then(Value::as_str) == Some(rel))
            .filter_map(|e| e.data.get("amendment").and_then(Value::as_str))
            .collect();
        for e in &c.events {
            let field = |k: &str| e.data.get(k).and_then(Value::as_str);
            match e.kind {
                Kind::Confirm if field("decision") == Some("accept") && field("amendment").is_some_and(|a| unlock_ids.contains(a)) => {
                    open = true;
                }
                Kind::Prove if open => {
                    let task = c.loaded.tasks.iter().find(|t| Some(t.id.to_string().as_str()) == field("task"));
                    if let Some(t) = task
                        && fingerprint::globset(&t.files)?.is_match(rel)
                    {
                        open = false;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(open)
}

/// The backlog change whose plan blocks code edits: the branch has one and no
/// active change, so the user has not accepted a plan to work on yet.
fn unaccepted_plan(project: &Project, top: &Path) -> Result<Option<crate::id::Id>> {
    let Some(branch) = git::branch(top)? else {
        return Ok(None);
    };
    let (living, _) = change::load_living(project)?;
    if !proof::on_branch(&proof::active(project, &living)?, &branch).is_empty() {
        return Ok(None);
    }
    let (all, _) = change::list(project)?;
    for c in all.into_iter().filter(|c| c.stage == change::Stage::Backlog) {
        if change::load(&c.path, &living)?.proposal.branch == branch {
            return Ok(Some(c.id));
        }
    }
    Ok(None)
}

/// The protected paths of every active change, from their baselines.
pub(crate) fn protected(project: &Project, top: &Path) -> Result<BTreeSet<String>> {
    let (living, _) = change::load_living(project)?;
    let mut out = BTreeSet::new();
    let branch = git::branch(top)?.unwrap_or_default();
    for c in proof::active(project, &living)?.iter().filter(|c| c.branch() == Some(branch.as_str())) {
        out.extend(crate::guard::guarded(c).into_keys().filter(|k| !k.ends_with(crate::guard::RUST_TESTS)).map(|k| key(Path::new(&k))));
        let Some(hash) = c.baseline().and_then(|b| b.get("protected")).and_then(Value::as_str) else {
            continue;
        };
        let path = project.root.join("objects").join(hash);
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        if let Ok(map) = serde_json::from_str::<serde_json::Map<String, Value>>(&text) {
            out.extend(map.keys().map(|k| key(Path::new(k))));
        }
    }
    Ok(out)
}

/// Why `file` must not be read or written: it is inside the hidden checks.
fn secret_denial(cwd: &Path, file: &str) -> Option<Denial> {
    let target = key(&real_path(Path::new(file), cwd));
    secret_paths().into_iter().find_map(|p| {
        let p = key(&real_path(&p, cwd));
        if !(target == p || target.starts_with(&format!("{p}/"))) {
            return None;
        }
        denial(
            Rule::HiddenChecks,
            format!(
                "{p} holds checks that stay hidden from the implementing agent; do not read or write them. If the plan needs them, run `rvow block <task> --reason \"...\"`"
            ),
        )
    })
}

/// Why `file` must not be read or written: it is the gate's cache file or the lease file.
fn cache_denial(project: &Project, cwd: &Path, file: &str) -> Option<Denial> {
    let target = key(&real_path(Path::new(file), cwd));
    let is = |name: &str| target == key(&real_path(&project.root.join(name), cwd));
    if is(CACHE_FILE) {
        return denial(Rule::CacheFile, format!("{CACHE_FILE} is written and read only by rvow gate; run `rvow gate --rerun` instead"));
    }
    if is(STATS_FILE) {
        return denial(
            Rule::StatsFile,
            format!("{STATS_FILE} is written and read only by rvow; run `rvow stats` to see the counts instead"),
        );
    }
    if is(LEASE_FILE) {
        return denial(
            Rule::LeaseFile,
            format!("{LEASE_FILE} is written and read only by rvow; run `rvow delegate <task>` to lease a task instead"),
        );
    }
    None
}

/// Why a subagent that holds a lease must not edit `file`: it lies outside the
/// files of the task leased to it.
fn lease_denial(project: &Project, cwd: &Path, agent_id: &str, file: &str) -> Result<Option<Denial>> {
    let Ok(home) = crate::store::home_dir() else { return Ok(None) };
    let Some(lease) = lease::holder(project, &home, cwd, agent_id, crate::ledger::now_ms())? else {
        return Ok(None);
    };
    let top = git::toplevel(cwd)?;
    let target = key(&real_path(Path::new(file), cwd));
    let Some(rel) = target.strip_prefix(&format!("{}/", key(&real_path(&top, cwd)))) else {
        return Ok(None);
    };
    if fingerprint::globset(&lease.files).is_ok_and(|set| set.is_match(rel)) {
        return Ok(None);
    }
    Ok(denial(
        Rule::LeaseScope,
        format!(
            "{rel} is outside the files of {task}, the task leased to this agent; edit only {files}, or run rvow block {task} --reason \"...\" if the task needs it",
            task = lease.task,
            files = lease.files.join(", ")
        ),
    ))
}

/// The rule and the reason that deny a tool call, or `None` to allow it.
pub fn pre_tool_use_denial(project: &Project, input: &Value) -> Result<Option<Denial>> {
    let cwd = PathBuf::from(input.get("cwd").and_then(Value::as_str).unwrap_or("."));
    let tool = input.get("tool_name").and_then(Value::as_str).unwrap_or("");
    let args = input.get("tool_input").cloned().unwrap_or(Value::Null);
    if tool == "Bash" {
        let Some(c) = args.get("command").and_then(Value::as_str) else {
            return Ok(None);
        };
        let top = git::toplevel(&cwd).ok();
        let trust = top.as_deref().and_then(|t| signers_path(project, t));
        return Ok(bash_rule(c)
            .or_else(|| script_rule(c, &cwd))
            .or_else(|| bash_secret_rule(c, &secret_paths()))
            .or_else(|| bash_enforcement_rule(c, &enforcement_paths(trust.as_deref()))));
    }
    let files = file_args(&args);
    if let Some(found) = files.iter().find_map(|f| secret_denial(&cwd, f).or_else(|| cache_denial(project, &cwd, f))) {
        return Ok(Some(found));
    }
    if !writes_files(tool) {
        return Ok(None);
    }
    let agent = input.get("agent_id").and_then(Value::as_str);
    for f in &files {
        let mut found = edit_rule(project, &git::toplevel(&cwd)?, &cwd, f)?;
        if found.is_none()
            && let Some(agent) = agent
        {
            found = lease_denial(project, &cwd, agent, f)?;
        }
        if found.is_some() {
            return Ok(found);
        }
    }
    Ok(None)
}

/// Claude Code `PreToolUse`: a deny response, or `None` to allow.
pub fn claude_pre_tool_use(project: &Project, input: &Value) -> Result<Option<Value>> {
    let denial = pre_tool_use_denial(project, input)?;
    if let Some(d) = &denial {
        stats::record(project, Hook::PreToolUse, Outcome::Deny, Some(d.rule), input.get("session_id").and_then(Value::as_str));
    }
    Ok(denial.map(|d| {
        json!({"hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": format!("rvow: {}", d.reason),
        }})
    }))
}

/// The Claude mod's `agent.spawn`: binds the new subagent to the lease that its
/// prompt names. Prints nothing for a prompt without a marker.
pub fn claude_lease_bind(project: &Project, input: &Value) -> Result<Option<Value>> {
    let text = |k: &str| input.get(k).and_then(Value::as_str);
    let (Some(agent), Some(task)) = (text("agent_id"), text("prompt").and_then(lease::marker)) else {
        return Ok(None);
    };
    Ok(Some(json!({"bound": lease::bind(project, &task, agent)?, "task": task})))
}

/// The Claude mod's `SubagentStop`: ends the lease that the stopping subagent holds.
pub fn claude_lease_end(project: &Project, input: &Value) -> Result<Option<Value>> {
    if let Some(agent) = input.get("agent_id").and_then(Value::as_str) {
        lease::end(project, agent)?;
    }
    Ok(None)
}

/// Claude Code `SessionStart`: the brief as added context, or `None` when the
/// branch has no active change.
pub fn claude_session_start(project: &Project, home: &Path, input: &Value) -> Result<Option<Value>> {
    let cwd = PathBuf::from(input.get("cwd").and_then(Value::as_str).unwrap_or("."));
    let text = brief::brief(project, home, &cwd)?;
    Ok((!text.is_empty()).then(|| {
        json!({"hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": text,
        }})
    }))
}

const MAX_DRIFT_ITEMS: usize = 3;

/// True while a rebase, merge or cherry-pick is half done in the repository at `top`.
fn git_operation_running(top: &Path) -> bool {
    let Ok(dir) = git::run(top, &["rev-parse", "--absolute-git-dir"]) else {
        return false;
    };
    let dir = PathBuf::from(dir.trim());
    ["rebase-merge", "rebase-apply", "MERGE_HEAD", "CHERRY_PICK_HEAD"].iter().any(|n| dir.join(n).exists())
}

/// Why the agent must keep working, or `None` when it may stop. Pending
/// amendments let it stop, since only the user can decide them.
pub fn stop_reason(project: &Project, home: &Path, cwd: &Path) -> Result<Option<String>> {
    let top = git::toplevel(cwd)?;
    let Some(branch) = git::branch(&top)? else {
        return Ok(None);
    };
    if git_operation_running(&top) {
        return Ok(None);
    }
    let (living, _) = change::load_living(project)?;
    let changes = proof::active(project, &living)?;
    let queue = proof::on_branch(&changes, &branch);
    if queue.is_empty() {
        return Ok(None);
    }
    if !amend::pending(&changes).is_empty() {
        return Ok(None);
    }
    let config = config::load(project, home)?;
    for c in &queue {
        let blocking: Vec<String> =
            drift::drift(project, &top, &config, c)?.into_iter().filter(|i| i.class.blocks()).map(|i| i.what).collect();
        if !blocking.is_empty() {
            let more = blocking.len().saturating_sub(MAX_DRIFT_ITEMS);
            let mut shown = blocking[..blocking.len() - more].join("; ");
            if more > 0 {
                shown.push_str(&format!("; and {more} more"));
            }
            return Ok(Some(format!(
                "the plan changed without an amendment ({shown}); run rvow amend --reason \"...\" and ask the user to run /rvow-confirm (or ! rvow confirm)"
            )));
        }
    }
    let now = Now { top: &top, config: &config, env: fingerprint::env(&top, &config)?, living: &living };
    let mut stale = false;
    // Only the newest accepted change is the agent's current plan; older changes on the branch are
    // enforced by the pre-push gate, and nagging about them each time hides the plan that matters.
    let newest = queue.iter().max_by_key(|c| c.vow().map_or(0, |e| e.time));
    for c in queue.iter().filter(|c| newest.is_some_and(|n| std::ptr::eq(*c, n))) {
        let changed = changed_since_vow(&top, c);
        for t in &c.loaded.tasks {
            let s = proof::state(c, t, &now)?;
            if !s.open() {
                continue;
            }
            // A task nobody started is no reason to hold the agent back from a plain answer.
            if s == State::Missing
                && let Some(files) = &changed
                && !t
                    .files
                    .iter()
                    .any(|g| fingerprint::globset(std::slice::from_ref(g)).is_ok_and(|set| files.iter().any(|f| set.is_match(f))))
            {
                continue;
            }
            if matches!(s, State::Stale(_) | State::Flaky) {
                stale = true;
                continue;
            }
            let id = t.id.to_string();
            return Ok(Some(format!(
                "{id} has no proof: run rvow prove {id}, or rvow block {id} --reason \"...\" if it cannot pass as written"
            )));
        }
    }
    // One fixed text for every stale task, so it is given once and not once for each task.
    Ok(stale.then(|| "some tasks have stale proofs; run rvow prove --stale before you push".to_string()))
}

/// The files that differ from the vow's base commit, committed or not, or
/// `None` when the base is unknown.
fn changed_since_vow(top: &Path, c: &proof::Active) -> Option<Vec<String>> {
    let base = c.vow()?.data.get("base_commit")?.as_str()?;
    let tracked = git::run(top, &["diff", "--name-only", base]).ok()?;
    let new = git::run(top, &["ls-files", "-o", "--exclude-standard"]).ok()?;
    Some(tracked.lines().chain(new.lines()).map(String::from).collect())
}

/// One line for a status bar: proved tasks, drift and pending amendments of
/// the branch's active changes, or `None` when the branch has none.
pub fn status_line(project: &Project, home: &Path, cwd: &Path) -> Result<Option<String>> {
    let top = git::toplevel(cwd)?;
    let Some(branch) = git::branch(&top)? else {
        return Ok(None);
    };
    let (living, _) = change::load_living(project)?;
    let changes = proof::active(project, &living)?;
    let queue = proof::on_branch(&changes, &branch);
    if queue.is_empty() {
        let plans: Vec<String> = crate::plan::check(project, cwd, None)?
            .into_iter()
            .filter(|r| r.change.stage == change::Stage::Backlog)
            .map(|r| r.change.id.to_string())
            .collect();
        return Ok(match plans.as_slice() {
            [] => None,
            [one] => Some(format!("rvow {one}: plan waits for accept")),
            many => Some(format!("rvow: {} plans wait for accept", many.len())),
        });
    }
    let config = config::load(project, home)?;
    let now = Now { top: &top, config: &config, env: fingerprint::env(&top, &config)?, living: &living };
    let (mut proved, mut total, mut drift_count) = (0, 0, 0);
    for c in &queue {
        for t in &c.loaded.tasks {
            total += 1;
            if proof::state(c, t, &now)? == State::Proved {
                proved += 1;
            }
        }
        drift_count += drift::drift(project, &top, &config, c)?.iter().filter(|i| i.class.blocks()).count();
    }
    let pending = amend::pending(&changes).len();
    let name = match queue.as_slice() {
        [one] => one.dir.id.to_string(),
        many => format!("{} changes", many.len()),
    };
    let mut line = format!("rvow {name}: {proved}/{total} proved");
    if drift_count > 0 {
        line.push_str(&format!(", {drift_count} drift"));
    }
    if pending > 0 {
        line.push_str(&format!(", {pending} pending"));
    }
    Ok(Some(line))
}

/// Claude Code `Stop`: a block response, or `None` to let the agent stop.
pub fn claude_stop(project: &Project, home: &Path, input: &Value) -> Result<Option<Value>> {
    let out = stop_decision(project, home, input)?;
    let outcome = if out.is_some() { Outcome::Block } else { Outcome::Pass };
    stats::record(project, Hook::Stop, outcome, None, input.get("session_id").and_then(Value::as_str));
    Ok(out)
}

fn stop_decision(project: &Project, home: &Path, input: &Value) -> Result<Option<Value>> {
    let cwd = PathBuf::from(input.get("cwd").and_then(Value::as_str).unwrap_or("."));
    let flag = |k: &str| input.get(k).and_then(Value::as_bool).unwrap_or(false);
    if input.get("agent_id").is_some() || flag("stop_hook_active") {
        return Ok(None);
    }
    let session = input.get("session_id").and_then(Value::as_str);
    let Some(reason) = stop_reason(project, home, &cwd)? else {
        if let Some(s) = session {
            stop_state::clear(project, s);
        }
        return Ok(None);
    };
    if session.is_some_and(|s| stop_state::already_given(project, s, &reason)) {
        return Ok(None);
    }
    Ok(Some(json!({"decision": "block", "reason": format!("rvow: {reason}")})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proof::tests::accepted;

    #[test]
    fn a_literal_glob_matches_a_casewise_edited_path_on_macos() {
        // `key` lowercases the edited path on macOS; the scope must do the same
        // to the vowed globs or a literal glob like SKILL.md.tmpl never matches.
        let rel = "skills/rvow/skill.md.tmpl";
        let globs = if cfg!(target_os = "macos") {
            vec!["skills/rvow/SKILL.md.tmpl".to_lowercase()]
        } else {
            vec!["skills/rvow/SKILL.md.tmpl".into()]
        };
        assert!(fingerprint::globset(&globs).unwrap().is_match(rel));
    }

    /// Every denial says what is blocked, why, and the command or user action
    /// to run next, and no text doubles the `rvow:` prefix that the adapter adds.
    fn denial_has_a_next_step(reason: &str) {
        assert!(!reason.starts_with("rvow: rvow"), "doubled prefix: {reason}");
        assert!(!reason.starts_with("Never "), "state the rule in the positive form: {reason}");
        let next = ["rvow ", "git ", "run ", "ask the user", "only the user", "instead", "Use "];
        assert!(next.iter().any(|n| reason.contains(n)), "no next step: {reason}");
    }

    #[test]
    fn every_deny_reason_names_a_next_step() {
        for reason in [
            "x enforces rvow; changing it is for the user, so only the user edits it. If the plan needs it, run rvow block <task> --reason \"...\"",
            "ledger.jsonl is written only by rvow; use rvow commands instead of editing it",
            "an archived change is history and does not change; start a new change with rvow new instead",
            "living specs change only through a change and rvow archive; run rvow guide spec to see how",
            ".git/ is not edited by hand; the hooks there run the gate. Use git commands, and run rvow install for the hooks",
            "t.sh is a protected test or test setup file; to change it, run rvow amend --unlock t.sh --reason \"...\" and ask the user to confirm",
            "t.sh is outside the files: of every task in the active change; run rvow amend --reason \"...\" after adding it to a task, and ask the user to confirm",
            "the plan CHG-x is not accepted yet; finish it, run rvow check, then ask the user to run /rvow-accept CHG-x (or ! rvow accept CHG-x) before you edit code",
            "the plan changed without an amendment (a); run rvow amend --reason \"...\" and ask the user to run ! rvow confirm",
            "rvow accept belongs to the user; the hooks deny it for agents. Ask the user to run /rvow-accept <id>, or to type ! rvow accept",
            "--no-verify skips the hooks that check the plan; run git commit and git push without it",
            "git commit -n skips the hooks that check the plan; run git commit without -n",
            "core.hooksPath decides which hooks run; changing it is for the user. If the plan needs it, run rvow block <task> --reason \"...\"",
            ".git/hooks holds the pre-push gate; only rvow install writes it. Run rvow install instead",
        ] {
            denial_has_a_next_step(reason);
        }
    }

    #[test]
    fn bash_rules() {
        let denied = [
            "rvow accept 7k2 --yes",
            "cd x && /usr/local/bin/rvow confirm --accept --all ab12",
            "./target/debug/rvow --toon confirm AMD-x --accept",
            "cargo run -q -- confirm --accept --all ab12",
            "git push --no-verify",
            "git commit -n -m x",
            "git -c core.hooksPath=/dev/null push",
            "git config core.hooksPath /tmp/none",
            "echo '{}' >> ~/.local/share/rvow/x/active/c/ledger.jsonl",
            "cat ids.jsonl",
            "rm .git/hooks/pre-push",
            "rvow answer CHG-x:proposal.md:3 yes --via dialog",
            "rvow answer CHG-x:proposal.md:3 yes --via=dialog",
        ];
        for cmd in denied {
            assert!(bash_denial(cmd).is_some(), "{cmd}");
        }
        let allowed = [
            "rvow prove x7k",
            "rvow status",
            "rvow answer CHG-x:proposal.md:3 yes",
            "rvow amend --reason 'accept the new name'",
            "cargo test confirm_flow",
            "git commit -m 'confirm accept'",
            "git push",
            "rvow show ledger.jsonl",
            "grep -rn accept src/",
        ];
        for cmd in allowed {
            assert_eq!(bash_denial(cmd), None, "{cmd}");
        }
    }

    #[test]
    fn quoted_accept_is_not_denied() {
        let allowed = [
            "cat > f <<'EOF'\nThe accept command with rvow accept --yes\nEOF",
            "cat <<-EOF\n\trvow confirm x\n\tEOF\necho done",
            "echo \"run rvow accept X\"",
            "echo 'run rvow confirm X'",
        ];
        for cmd in allowed {
            assert_eq!(bash_denial(cmd), None, "{cmd}");
        }
        let denied = [
            "cat <<EOF\nx\nEOF\nrvow accept X",
            "echo 'hi' && rvow accept X --yes",
            "echo \"$(rvow accept X)\"",
            "env FOO=1 rvow accept",
            "x; rvow confirm",
        ];
        for cmd in denied {
            assert!(bash_denial(cmd).is_some(), "{cmd}");
        }
    }

    #[test]
    fn a_backlog_plan_blocks_code_edits_until_accept() {
        let tmp = crate::git::testrepo::TempDir::new();
        let home = tmp.0.join("home");
        let repo = crate::git::testrepo::repo_with_commit(&tmp.0, "r");
        let p = crate::store::init(&repo, &home, crate::store::Mode::Home).unwrap();
        assert_eq!(edit(&p, &repo, "src/lib.rs"), None);
        let c = change::new(&p, "T", "main", true).unwrap();
        let denied = edit(&p, &repo, "src/lib.rs").unwrap();
        let reason = denied["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains(&c.id.to_string()), "{reason}");
        assert_eq!(edit(&p, &repo, c.path.join("tasks.md").to_str().unwrap()), None);

        std::fs::write(c.path.join("tasks.md"), "- TSK-new1 T\n  verify: false\n  files: src/**\n").unwrap();
        let prepared = crate::vow::prepare(&p, &home, c, &repo, &mut std::io::sink()).unwrap();
        crate::vow::commit(&p, prepared).unwrap();
        assert_eq!(edit(&p, &repo, "src/lib.rs"), None);
    }

    #[test]
    fn status_line_names_a_waiting_plan() {
        let tmp = crate::git::testrepo::TempDir::new();
        let home = tmp.0.join("home");
        let repo = crate::git::testrepo::repo_with_commit(&tmp.0, "r");
        let p = crate::store::init(&repo, &home, crate::store::Mode::Home).unwrap();
        assert_eq!(status_line(&p, &home, &repo).unwrap(), None);
        let c = change::new(&p, "T", "main", true).unwrap();
        assert_eq!(status_line(&p, &home, &repo).unwrap(), Some(format!("rvow {}: plan waits for accept", c.id)));
    }

    #[test]
    fn writes_to_enforcement_files_are_denied() {
        let paths = vec![PathBuf::from("/h/.claude/settings.json"), PathBuf::from("/h/.local/share/rvow/claude-plugin")];
        assert!(bash_enforcement_denial("echo {} > /h/.claude/settings.json", &paths).is_some());
        assert!(bash_enforcement_denial("rm -rf /h/.local/share/rvow/claude-plugin/plugins", &paths).is_some());
        assert_eq!(bash_enforcement_denial("cat /h/.claude/settings.json", &paths), None);
        assert_eq!(bash_enforcement_denial("rm -rf build", &paths), None);
    }

    #[test]
    fn real_paths_resolve_dots_links_and_missing_files() {
        let tmp = crate::git::testrepo::TempDir::new();
        std::fs::create_dir_all(tmp.0.join("a/b")).unwrap();
        assert_eq!(real_path(Path::new("a/./b/../b/new.txt"), &tmp.0), tmp.0.join("a/b/new.txt"));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(tmp.0.join("a/b"), tmp.0.join("link")).unwrap();
            assert_eq!(real_path(Path::new("link/x.rs"), &tmp.0), tmp.0.join("a/b/x.rs"));
        }
    }

    fn edit(p: &Project, repo: &Path, file: &str) -> Option<Value> {
        let input = json!({
            "cwd": repo.to_str().unwrap(),
            "tool_name": "Edit",
            "tool_input": {"file_path": file},
        });
        claude_pre_tool_use(p, &input).unwrap()
    }

    #[test]
    fn edits_of_rvow_files_hooks_and_protected_tests_are_denied() {
        let tmp = crate::git::testrepo::TempDir::new();
        let home = tmp.0.join("home");
        let repo = crate::git::testrepo::repo_with_commit(&tmp.0, "r");
        std::fs::create_dir_all(repo.join("tests")).unwrap();
        std::fs::write(repo.join("tests/t.sh"), "exit 0").unwrap();
        let p = crate::store::init(&repo, &home, crate::store::Mode::Home).unwrap();
        let c = change::new(&p, "T", "main", true).unwrap();
        std::fs::write(c.path.join("tasks.md"), "- TSK-new1 T\n  verify: false\n  files: src/**, tests/**\n").unwrap();
        let prepared = crate::vow::prepare(&p, &home, c, &repo, &mut std::io::sink()).unwrap();
        let vowed = crate::vow::commit(&p, prepared).unwrap();

        assert!(edit(&p, &repo, "tests/t.sh").is_some());
        assert!(edit(&p, &repo, repo.join("tests/./t.sh").to_str().unwrap()).is_some());
        if cfg!(target_os = "macos") {
            assert!(edit(&p, &repo, "Tests/T.sh").is_some());
        }
        assert!(edit(&p, &repo, ".claude/settings.json").is_some());
        assert!(edit(&p, &repo, ".git/hooks/pre-push").is_some());
        assert!(edit(&p, &repo, vowed.path.join(LEDGER).to_str().unwrap()).is_some());
        assert!(edit(&p, &repo, p.root.join(IDS_FILE).to_str().unwrap()).is_some());

        assert_eq!(edit(&p, &repo, "tests/new.sh"), None);
        assert_eq!(edit(&p, &repo, "src/lib.rs"), None);
        assert_eq!(edit(&p, &repo, vowed.path.join("tasks.md").to_str().unwrap()), None);

        let bash = json!({"cwd": repo.to_str().unwrap(), "tool_name": "Bash", "tool_input": {"command": "rvow accept x"}});
        let out = claude_pre_tool_use(&p, &bash).unwrap().unwrap();
        assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny");
    }

    #[test]
    fn edit_outside_active_scope_is_denied() {
        let tmp = crate::git::testrepo::TempDir::new();
        let home = tmp.0.join("home");
        let repo = crate::git::testrepo::repo_with_commit(&tmp.0, "r");
        std::fs::write(repo.join(".gitignore"), "target/\n").unwrap();
        let p = crate::store::init(&repo, &home, crate::store::Mode::Home).unwrap();
        let c = change::new(&p, "T", "main", true).unwrap();
        std::fs::write(c.path.join("tasks.md"), "- TSK-new1 T\n  verify: false\n  files: src/**\n").unwrap();
        let prepared = crate::vow::prepare(&p, &home, c, &repo, &mut std::io::sink()).unwrap();
        crate::vow::commit(&p, prepared).unwrap();

        assert_eq!(edit(&p, &repo, "src/lib.rs"), None);
        assert_eq!(edit(&p, &repo, "target/debug/x"), None);
        let out = edit(&p, &repo, "docs/readme.md").unwrap();
        assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(out["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().contains("rvow amend"));
    }

    #[test]
    fn edits_of_living_specs_and_the_archive_are_denied() {
        let tmp = crate::git::testrepo::TempDir::new();
        let repo = crate::git::testrepo::repo_with_commit(&tmp.0, "r");
        let p = crate::store::init(&repo, &tmp.0.join("home"), crate::store::Mode::Home).unwrap();
        let reason = |file: PathBuf| {
            edit(&p, &repo, file.to_str().unwrap())
                .map(|v| v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().to_string())
        };
        assert!(reason(p.root.join("specs/cap/spec.md")).unwrap().ends_with("run rvow guide spec to see how"));
        let archived = reason(p.root.join("archive/done/2026-10-01-CHG-7k2mq8-x/tasks.md"));
        assert!(archived.unwrap().contains("history"));
        let c = change::new(&p, "T", "main", false).unwrap();
        assert_eq!(reason(c.path.join("specs/cap.md")), None);
    }

    #[test]
    fn unlock_opens_a_protected_file_until_the_next_matching_proof() {
        let tmp = crate::git::testrepo::TempDir::new();
        let home = tmp.0.join("home");
        let repo = crate::git::testrepo::repo_with_commit(&tmp.0, "r");
        std::fs::create_dir_all(repo.join("tests")).unwrap();
        std::fs::write(repo.join("tests/t.sh"), "exit 1").unwrap();
        let p = crate::store::init(&repo, &home, crate::store::Mode::Home).unwrap();
        let c = change::new(&p, "T", "main", true).unwrap();
        std::fs::write(
            c.path.join("tasks.md"),
            "- TSK-new1 T\n  verify: sh tests/t.sh && echo 'test result: ok. 1 passed; 0 failed;'\n  files: tests/**\n",
        )
        .unwrap();
        let prepared = crate::vow::prepare(&p, &home, c, &repo, &mut std::io::sink()).unwrap();
        let vowed = crate::vow::commit(&p, prepared).unwrap();
        let task = vowed.minted[0].1.to_string();

        assert!(edit(&p, &repo, "tests/t.sh").is_some());
        let err = amend::amend(&p, &home, &repo, None, "fix", Some("src/x.rs")).unwrap_err();
        assert_eq!(err.code, "unlock");
        let a = amend::amend(&p, &home, &repo, None, "the test is wrong", Some("tests/t.sh")).unwrap();
        assert!(edit(&p, &repo, "tests/t.sh").is_some(), "pending is still locked");
        amend::confirm(&p, &[a[0].id.to_string()], amend::Decision::Accept, None).unwrap();
        assert_eq!(edit(&p, &repo, "tests/t.sh"), None);

        std::fs::write(repo.join("tests/t.sh"), "exit 0").unwrap();
        proof::prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        assert!(edit(&p, &repo, "tests/t.sh").is_some(), "a proof locks it again");
    }

    #[test]
    fn stop_ignores_untouched_tasks() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        let input = json!({"cwd": repo.to_str().unwrap()});
        assert_eq!(claude_stop(&p, &home, &input).unwrap(), None);

        std::fs::write(repo.join("other.txt"), "x").unwrap();
        assert_eq!(claude_stop(&p, &home, &input).unwrap(), None);

        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        let out = claude_stop(&p, &home, &input).unwrap().unwrap();
        assert!(out["reason"].as_str().unwrap().contains(&task), "{out}");
    }

    #[test]
    fn stop_gives_one_fixed_reason_for_stale_tasks() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        let input = json!({"cwd": repo.to_str().unwrap()});
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        proof::prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        std::fs::write(repo.join("out.txt"), "ok changed").unwrap();
        let reason = claude_stop(&p, &home, &input).unwrap().unwrap()["reason"].as_str().unwrap().to_string();
        assert!(reason.contains("stale") && reason.contains("rvow prove --stale"), "{reason}");
        assert!(!reason.contains(&task), "a stale task must not put its id in the reason, or each task is a new reason: {reason}");
    }

    #[test]
    fn stop_blocks_on_open_tasks_and_drift_but_not_on_pending() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        let input = json!({"cwd": repo.to_str().unwrap()});
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        let out = claude_stop(&p, &home, &input).unwrap().unwrap();
        assert!(out["reason"].as_str().unwrap().contains(&task), "{out}");

        proof::prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        assert_eq!(claude_stop(&p, &home, &input).unwrap(), None);

        let dir = std::fs::read_dir(p.root.join("active")).unwrap().next().unwrap().unwrap().path();
        let tasks = dir.join("tasks.md");
        let text = std::fs::read_to_string(&tasks).unwrap();
        std::fs::write(&tasks, text.replace("Write ok", "Write it")).unwrap();
        let out = claude_stop(&p, &home, &input).unwrap().unwrap();
        assert!(out["reason"].as_str().unwrap().contains("amend"), "{out}");

        amend::amend(&p, &home, &repo, None, "rename", None).unwrap();
        assert_eq!(claude_stop(&p, &home, &input).unwrap(), None);

        assert_eq!(
            status_line(&p, &home, &repo).unwrap().unwrap(),
            format!("rvow {}: 0/1 proved, 1 drift, 1 pending", dir.file_name().unwrap().to_str().unwrap().get(..10).unwrap())
        );
        let start = claude_session_start(&p, &home, &input).unwrap().unwrap();
        assert!(start["hookSpecificOutput"]["additionalContext"].as_str().unwrap().contains("tasks:"));
    }
}
