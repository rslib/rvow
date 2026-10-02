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
use crate::git;
use crate::ledger::Kind;
use crate::proof::{self, Now, State};
use crate::store::{IDS_FILE, PROJECT_FILE, Project};

const SHELL_SPLIT: &[char] = &[';', '&', '|', '(', ')', '`', '\n', '<', '>'];

/// Words of a shell command, split on whitespace and control characters, with
/// simple quotes removed. Good enough to spot a command name; an obfuscated
/// call (built from variables) is a known gap that the final gate covers.
fn words(cmd: &str) -> Vec<String> {
    cmd.split(|c: char| c.is_whitespace() || SHELL_SPLIT.contains(&c))
        .map(|w| w.trim_matches(|c| c == '"' || c == '\''))
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

fn base_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// `cmd` without heredoc bodies and quoted text, which a shell does not run.
/// A double-quoted span that holds `$(` or a backtick stays, since it runs.
fn unquoted(cmd: &str) -> String {
    let mut kept = String::new();
    let mut end: Option<String> = None;
    for line in cmd.lines() {
        if let Some(e) = &end {
            if line.trim() == e {
                end = None;
            }
            continue;
        }
        kept.push_str(line);
        kept.push('\n');
        if let Some(i) = line.find("<<") {
            let tag = line[i + 2..].trim_start_matches('-').trim_start();
            let tag: String = tag
                .trim_start_matches(['\'', '"'])
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !tag.is_empty() {
                end = Some(tag);
            }
        }
    }
    let mut out = String::new();
    let mut rest = kept.as_str();
    while let Some(i) = rest.find(['\'', '"']) {
        out.push_str(&rest[..i]);
        let q = &rest[i..=i];
        let tail = &rest[i + 1..];
        let Some(j) = tail.find(q) else {
            out.push_str(&rest[i..]);
            return out;
        };
        let span = &tail[..j];
        if q == "\"" && (span.contains("$(") || span.contains('`')) {
            out.push_str(span);
        }
        out.push(' ');
        rest = &tail[j + 1..];
    }
    out.push_str(rest);
    out
}

/// Why a Bash command must not run, or `None`.
pub fn bash_denial(cmd: &str) -> Option<String> {
    let w = words(&unquoted(cmd));
    let lower = cmd.to_lowercase();
    for (i, word) in w.iter().enumerate() {
        let rvow_call = base_name(word) == "rvow"
            || (word == "run" && i > 0 && base_name(&w[i - 1]) == "cargo");
        if rvow_call
            && let Some(sub) = w[i + 1..]
                .iter()
                .find(|x| !x.starts_with('-') && *x != "rvow")
            && (sub == "accept" || sub == "confirm")
        {
            return Some(format!(
                "rvow {sub} is for the user only; ask the user to run `! rvow {sub}`"
            ));
        }
    }
    let rvow_answer = w.iter().any(|x| base_name(x) == "rvow") && w.iter().any(|x| x == "answer");
    let via_dialog = w.windows(2).any(|p| p[0] == "--via" && p[1] == "dialog")
        || w.iter().any(|x| x == "--via=dialog");
    if rvow_answer && via_dialog {
        return Some(
            "only the rvow_ask tool records an answer as given in a dialog; record a chat answer with rvow answer <ref> \"<choice>\"".into(),
        );
    }
    let git = w.iter().any(|x| base_name(x) == "git");
    if git && w.iter().any(|x| x == "--no-verify") {
        return Some("--no-verify skips the hooks that check the plan".into());
    }
    if git && w.iter().any(|x| x == "commit") && w.iter().any(|x| x == "-n") {
        return Some("git commit -n skips the hooks that check the plan".into());
    }
    if lower.contains("core.hookspath") {
        return Some("core.hooksPath decides which hooks run; changing it is for the user".into());
    }
    if lower.contains(".git/hooks") {
        return Some(".git/hooks holds the pre-push gate; only rvow install writes it".into());
    }
    let names_rvow_file = w.iter().any(|x| {
        let b = base_name(x);
        b == LEDGER || b == IDS_FILE
    });
    let only_rvow = w.first().is_some_and(|x| base_name(x) == "rvow");
    if names_rvow_file && !only_rvow {
        return Some(format!(
            "{LEDGER} and {IDS_FILE} are written only by rvow; use rvow commands"
        ));
    }
    None
}

/// The absolute, symlink-free form of `path`, also when the file does not
/// exist yet: the nearest existing ancestor is resolved and the rest appended.
pub(crate) fn real_path(path: &Path, cwd: &Path) -> PathBuf {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
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
    if cfg!(target_os = "macos") {
        s.to_lowercase()
    } else {
        s
    }
}

fn file_arg(input: &Value) -> Option<String> {
    ["file_path", "notebook_path", "path"]
        .iter()
        .find_map(|k| input.get(k).and_then(Value::as_str))
        .map(String::from)
}

fn writes_files(tool: &str) -> bool {
    matches!(tool, "Edit" | "Write" | "MultiEdit" | "NotebookEdit") || tool.starts_with("mcp__")
}

/// Files outside any repository that enforce rvow: the installed mod, Claude
/// Code's plugins and settings, and the pi extension.
fn enforcement_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(home) = crate::store::home_dir() {
        out.push(home.join(crate::install::MARKETPLACE_DIR));
    }
    if let Some(h) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        let h = PathBuf::from(h);
        let claude = std::env::var_os("CLAUDE_CONFIG_DIR")
            .filter(|c| !c.is_empty())
            .map_or_else(|| h.join(".claude"), PathBuf::from);
        out.push(claude.join("plugins"));
        out.push(claude.join("settings.json"));
        out.push(claude.join("settings.local.json"));
        out.push(h.join(".pi/agent/extensions/rvow.ts"));
    }
    out
}

const WRITERS: &[&str] = &[
    "rm", "mv", "cp", "tee", "sed", "perl", "python", "python3", "chmod", "ln", "truncate", "dd",
    "install",
];

/// Why a Bash command that names an enforcement file must not run: a write to
/// one of them could switch rvow off.
fn bash_enforcement_denial(cmd: &str, paths: &[PathBuf]) -> Option<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let named = paths.iter().find(|p| {
        let full = p.display().to_string();
        let tilde = full.replacen(&home, "~", 1);
        cmd.contains(&full) || (!home.is_empty() && cmd.contains(&tilde))
    })?;
    let writes = cmd.contains('>') || words(cmd).iter().any(|w| WRITERS.contains(&base_name(w)));
    writes.then(|| {
        format!(
            "{} enforces rvow; changing it is for the user",
            named.display()
        )
    })
}

/// Why an edit of `file` must not happen, or `None`.
fn edit_denial(project: &Project, top: &Path, cwd: &Path, file: &str) -> Result<Option<String>> {
    let target = key(&real_path(Path::new(file), cwd));
    for p in enforcement_paths() {
        let p = key(&real_path(&p, cwd));
        if target == p || target.starts_with(&format!("{p}/")) {
            return Ok(Some(format!(
                "{p} enforces rvow; changing it is for the user"
            )));
        }
    }
    let root = key(&real_path(&project.root, cwd));
    let top_key = key(&real_path(top, cwd));
    let name = Path::new(&target)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if target.starts_with(&format!("{root}/"))
        && (name == LEDGER
            || name == IDS_FILE
            || name == PROJECT_FILE
            || name == crate::record::RECORD_FILE
            || target.starts_with(&format!("{root}/objects/")))
    {
        return Ok(Some(format!(
            "{name} is written only by rvow; use rvow commands"
        )));
    }
    if target.starts_with(&format!("{root}/archive/")) {
        return Ok(Some(
            "an archived change is history and does not change; start a new change instead".into(),
        ));
    }
    if target.starts_with(&format!("{root}/specs/")) {
        return Ok(Some(
            "living specs change only through a change and rvow archive".into(),
        ));
    }
    if target.starts_with(&format!("{top_key}/.git/")) {
        return Ok(Some(
            ".git/ is not edited by hand; the hooks there run the gate".into(),
        ));
    }
    let Some(rel) = target.strip_prefix(&format!("{top_key}/")) else {
        return Ok(None);
    };
    if !target.starts_with(&format!("{root}/"))
        && let Some(id) = unaccepted_plan(project, top)?
    {
        return Ok(Some(format!(
            "the plan {id} is not accepted yet; finish it, run rvow check, then ask the user to run /rvow-accept {id} (or ! rvow accept {id}) before you edit code"
        )));
    }
    if ENFORCEMENT.iter().any(|e| key(Path::new(e)) == rel) {
        return Ok(Some(format!(
            "{rel} configures the hooks that enforce the plan; changing it is for the user"
        )));
    }
    if protected(project, top)?.contains(rel) && !unlocked(project, top, rel)? {
        return Ok(Some(format!(
            "{rel} is a protected test or test setup file; to change it, run rvow amend --unlock {rel} --reason \"...\" and ask the user to confirm"
        )));
    }
    if !target.starts_with(&format!("{root}/"))
        && let Some(files) = active_scope(project, top)?
        && !files.is_match(rel)
        && git::run(top, &["check-ignore", "-q", "--", rel]).is_err()
    {
        return Ok(Some(format!(
            "{rel} is outside the files: of every task in the active change; run rvow amend --reason \"...\" after adding it to a task, and ask the user to confirm"
        )));
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
    let globs: Vec<String> = mine
        .iter()
        .flat_map(|c| c.loaded.tasks.iter().flat_map(|t| t.files.clone()))
        .collect();
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
    for c in proof::active(project, &living)?
        .iter()
        .filter(|c| c.branch() == Some(branch.as_str()))
    {
        let unlock_ids: BTreeSet<&str> = c
            .events
            .iter()
            .filter(|e| {
                e.kind == Kind::Amend && e.data.get("unlock").and_then(Value::as_str) == Some(rel)
            })
            .filter_map(|e| e.data.get("amendment").and_then(Value::as_str))
            .collect();
        for e in &c.events {
            let field = |k: &str| e.data.get(k).and_then(Value::as_str);
            match e.kind {
                Kind::Confirm
                    if field("decision") == Some("accept")
                        && field("amendment").is_some_and(|a| unlock_ids.contains(a)) =>
                {
                    open = true;
                }
                Kind::Prove if open => {
                    let task = c
                        .loaded
                        .tasks
                        .iter()
                        .find(|t| Some(t.id.to_string().as_str()) == field("task"));
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
    for c in all
        .into_iter()
        .filter(|c| c.stage == change::Stage::Backlog)
    {
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
    for c in proof::active(project, &living)?
        .iter()
        .filter(|c| c.branch() == Some(branch.as_str()))
    {
        out.extend(
            crate::guard::guarded(c)
                .into_keys()
                .filter(|k| !k.ends_with(crate::guard::RUST_TESTS))
                .map(|k| key(Path::new(&k))),
        );
        let Some(hash) = c
            .baseline()
            .and_then(|b| b.get("protected"))
            .and_then(Value::as_str)
        else {
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

/// Claude Code `PreToolUse`: a deny response, or `None` to allow.
pub fn claude_pre_tool_use(project: &Project, input: &Value) -> Result<Option<Value>> {
    let cwd = PathBuf::from(input.get("cwd").and_then(Value::as_str).unwrap_or("."));
    let tool = input.get("tool_name").and_then(Value::as_str).unwrap_or("");
    let args = input.get("tool_input").cloned().unwrap_or(Value::Null);
    let reason = if tool == "Bash" {
        args.get("command").and_then(Value::as_str).and_then(|c| {
            bash_denial(c).or_else(|| bash_enforcement_denial(c, &enforcement_paths()))
        })
    } else if writes_files(tool) {
        match file_arg(&args) {
            Some(f) => edit_denial(project, &git::toplevel(&cwd)?, &cwd, &f)?,
            None => None,
        }
    } else {
        None
    };
    Ok(reason.map(|r| {
        json!({"hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": format!("rvow: {r}"),
        }})
    }))
}

/// Claude Code `SessionStart`: the brief as added context, or `None` when the
/// branch has no active change.
pub fn claude_session_start(
    project: &Project,
    home: &Path,
    input: &Value,
) -> Result<Option<Value>> {
    let cwd = PathBuf::from(input.get("cwd").and_then(Value::as_str).unwrap_or("."));
    let text = brief::brief(project, home, &cwd)?;
    Ok((!text.is_empty()).then(|| {
        json!({"hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": text,
        }})
    }))
}

/// Why the agent must keep working, or `None` when it may stop. Pending
/// amendments let it stop, since only the user can decide them.
pub fn stop_reason(project: &Project, home: &Path, cwd: &Path) -> Result<Option<String>> {
    let top = git::toplevel(cwd)?;
    let Some(branch) = git::branch(&top)? else {
        return Ok(None);
    };
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
        let blocking: Vec<String> = drift::drift(project, &top, &config, c)?
            .into_iter()
            .filter(|i| i.class.blocks())
            .map(|i| i.what)
            .collect();
        if !blocking.is_empty() {
            return Ok(Some(format!(
                "the plan changed without an amendment ({}); run rvow amend --reason \"...\" and ask the user to run ! rvow confirm",
                blocking.join("; ")
            )));
        }
    }
    let now = Now {
        top: &top,
        config: &config,
        env: fingerprint::env(&top, &config)?,
        living: &living,
    };
    for c in &queue {
        let changed = changed_since_vow(&top, c);
        for t in &c.loaded.tasks {
            let s = proof::state(c, t, &now)?;
            if !s.open() {
                continue;
            }
            // A task nobody started is no reason to hold the agent back from a plain answer.
            if s == State::Missing
                && let Some(files) = &changed
                && !t.files.iter().any(|g| {
                    fingerprint::globset(std::slice::from_ref(g))
                        .is_ok_and(|set| files.iter().any(|f| set.is_match(f)))
                })
            {
                continue;
            }
            let id = t.id.to_string();
            return Ok(Some(match s {
                State::Stale(_) | State::Flaky => {
                    format!("{id} is {}: run rvow prove --stale", s.name())
                }
                _ => format!(
                    "{id} has no proof: run rvow prove {id}, or rvow block {id} --reason \"...\" if it cannot pass as written"
                ),
            }));
        }
    }
    Ok(None)
}

/// The files that differ from the vow's base commit, committed or not, or
/// `None` when the base is unknown.
fn changed_since_vow(top: &Path, c: &proof::Active) -> Option<Vec<String>> {
    let base = c.vow()?.data.get("base_commit")?.as_str()?;
    let tracked = git::run(top, &["diff", "--name-only", base]).ok()?;
    let new = git::run(top, &["ls-files", "-o", "--exclude-standard"]).ok()?;
    Some(
        tracked
            .lines()
            .chain(new.lines())
            .map(String::from)
            .collect(),
    )
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
    let now = Now {
        top: &top,
        config: &config,
        env: fingerprint::env(&top, &config)?,
        living: &living,
    };
    let (mut proved, mut total, mut drift_count) = (0, 0, 0);
    for c in &queue {
        for t in &c.loaded.tasks {
            total += 1;
            if proof::state(c, t, &now)? == State::Proved {
                proved += 1;
            }
        }
        drift_count += drift::drift(project, &top, &config, c)?
            .iter()
            .filter(|i| i.class.blocks())
            .count();
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
    let cwd = PathBuf::from(input.get("cwd").and_then(Value::as_str).unwrap_or("."));
    Ok(stop_reason(project, home, &cwd)?
        .map(|reason| json!({"decision": "block", "reason": format!("rvow: {reason}")})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proof::tests::accepted;

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
        let reason = denied["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap();
        assert!(reason.contains(&c.id.to_string()), "{reason}");
        assert_eq!(
            edit(&p, &repo, c.path.join("tasks.md").to_str().unwrap()),
            None
        );

        std::fs::write(
            c.path.join("tasks.md"),
            "- TSK-new1 T\n  verify: false\n  files: src/**\n",
        )
        .unwrap();
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
        assert_eq!(
            status_line(&p, &home, &repo).unwrap(),
            Some(format!("rvow {}: plan waits for accept", c.id))
        );
    }

    #[test]
    fn writes_to_enforcement_files_are_denied() {
        let paths = vec![
            PathBuf::from("/h/.claude/settings.json"),
            PathBuf::from("/h/.local/share/rvow/claude-plugin"),
        ];
        assert!(bash_enforcement_denial("echo {} > /h/.claude/settings.json", &paths).is_some());
        assert!(
            bash_enforcement_denial("rm -rf /h/.local/share/rvow/claude-plugin/plugins", &paths)
                .is_some()
        );
        assert_eq!(
            bash_enforcement_denial("cat /h/.claude/settings.json", &paths),
            None
        );
        assert_eq!(bash_enforcement_denial("rm -rf build", &paths), None);
    }

    #[test]
    fn real_paths_resolve_dots_links_and_missing_files() {
        let tmp = crate::git::testrepo::TempDir::new();
        std::fs::create_dir_all(tmp.0.join("a/b")).unwrap();
        assert_eq!(
            real_path(Path::new("a/./b/../b/new.txt"), &tmp.0),
            tmp.0.join("a/b/new.txt")
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(tmp.0.join("a/b"), tmp.0.join("link")).unwrap();
            assert_eq!(
                real_path(Path::new("link/x.rs"), &tmp.0),
                tmp.0.join("a/b/x.rs")
            );
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
        std::fs::write(
            c.path.join("tasks.md"),
            "- TSK-new1 T\n  verify: false\n  files: src/**, tests/**\n",
        )
        .unwrap();
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
        assert_eq!(
            edit(&p, &repo, vowed.path.join("tasks.md").to_str().unwrap()),
            None
        );

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
        std::fs::write(
            c.path.join("tasks.md"),
            "- TSK-new1 T\n  verify: false\n  files: src/**\n",
        )
        .unwrap();
        let prepared = crate::vow::prepare(&p, &home, c, &repo, &mut std::io::sink()).unwrap();
        crate::vow::commit(&p, prepared).unwrap();

        assert_eq!(edit(&p, &repo, "src/lib.rs"), None);
        assert_eq!(edit(&p, &repo, "target/debug/x"), None);
        let out = edit(&p, &repo, "docs/readme.md").unwrap();
        assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(
            out["hookSpecificOutput"]["permissionDecisionReason"]
                .as_str()
                .unwrap()
                .contains("rvow amend")
        );
    }

    #[test]
    fn edits_of_living_specs_and_the_archive_are_denied() {
        let tmp = crate::git::testrepo::TempDir::new();
        let repo = crate::git::testrepo::repo_with_commit(&tmp.0, "r");
        let p = crate::store::init(&repo, &tmp.0.join("home"), crate::store::Mode::Home).unwrap();
        let reason = |file: PathBuf| {
            edit(&p, &repo, file.to_str().unwrap()).map(|v| {
                v["hookSpecificOutput"]["permissionDecisionReason"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
        };
        assert_eq!(
            reason(p.root.join("specs/cap/spec.md")).unwrap(),
            "rvow: living specs change only through a change and rvow archive"
        );
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
        let a = amend::amend(
            &p,
            &home,
            &repo,
            None,
            "the test is wrong",
            Some("tests/t.sh"),
        )
        .unwrap();
        assert!(
            edit(&p, &repo, "tests/t.sh").is_some(),
            "pending is still locked"
        );
        amend::confirm(&p, &[a[0].id.to_string()], amend::Decision::Accept, None).unwrap();
        assert_eq!(edit(&p, &repo, "tests/t.sh"), None);

        std::fs::write(repo.join("tests/t.sh"), "exit 0").unwrap();
        proof::prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        assert!(
            edit(&p, &repo, "tests/t.sh").is_some(),
            "a proof locks it again"
        );
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
    fn stop_blocks_on_open_tasks_and_drift_but_not_on_pending() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        let input = json!({"cwd": repo.to_str().unwrap()});
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        let out = claude_stop(&p, &home, &input).unwrap().unwrap();
        assert!(out["reason"].as_str().unwrap().contains(&task), "{out}");

        proof::prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        assert_eq!(claude_stop(&p, &home, &input).unwrap(), None);

        let dir = std::fs::read_dir(p.root.join("active"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let tasks = dir.join("tasks.md");
        let text = std::fs::read_to_string(&tasks).unwrap();
        std::fs::write(&tasks, text.replace("Write ok", "Write it")).unwrap();
        let out = claude_stop(&p, &home, &input).unwrap().unwrap();
        assert!(out["reason"].as_str().unwrap().contains("amend"), "{out}");

        amend::amend(&p, &home, &repo, None, "rename", None).unwrap();
        assert_eq!(claude_stop(&p, &home, &input).unwrap(), None);

        assert_eq!(
            status_line(&p, &home, &repo).unwrap().unwrap(),
            format!(
                "rvow {}: 0/1 proved, 1 drift, 1 pending",
                dir.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .get(..10)
                    .unwrap()
            )
        );
        let start = claude_session_start(&p, &home, &input).unwrap().unwrap();
        assert!(
            start["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .contains("tasks:")
        );
    }
}
