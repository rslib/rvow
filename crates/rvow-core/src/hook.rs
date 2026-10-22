//! The logic behind the harness hooks. Adapters pass the harness's JSON in and
//! print what comes back; every rule lives here.

use std::cell::OnceCell;
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
use crate::stats::{self, Hook, Outcome, STATS_FILE, StopReason};
use crate::store::{IDS_FILE, PROJECT_FILE, Project};
use crate::yolo::{self, Tier, YOLO_FILE};

mod brief_state;
mod rule;
mod session_files;
mod shell;
mod stop_state;
mod store_guard;
mod streak;
mod witness;
pub use rule::{Denial, Mode, Rule};
#[cfg(test)]
use shell::bash_enforcement_denial;
use shell::{bash_changes, bash_enforcement_rule, bash_secret_rule, only_accept_or_confirm, prove_args, reads_only};
pub use shell::{bash_denial, bash_rule, is_builtin, programs, script_denial, script_rule};
use store_guard::{bash_store_denial, store_denial};
use witness::WITNESS_FILE;

use crate::provenance::Locked;

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

/// Whether the key `target` is the key `dir` or lies under it.
fn at_or_under(target: &str, dir: &str) -> bool {
    target == dir || under(target, dir)
}

/// Whether the key `target` lies under the key `dir`.
fn under(target: &str, dir: &str) -> bool {
    rel_under(target, dir).is_some()
}

/// The rest of the key `target` below the key `dir`, without the separator.
fn rel_under<'t>(target: &'t str, dir: &str) -> Option<&'t str> {
    target.strip_prefix(dir)?.strip_prefix('/')
}

/// What one hook call reads from the store, read at its first use and kept for
/// the rest of the call, so each rule of the call does not load it again.
pub(crate) struct Call<'a> {
    pub(crate) project: &'a Project,
    living: OnceCell<Vec<change::CapSpec>>,
    active: OnceCell<Vec<proof::Active>>,
    home: OnceCell<Option<PathBuf>>,
    config: OnceCell<Option<config::Config>>,
}

impl<'a> Call<'a> {
    pub(crate) fn new(project: &'a Project) -> Call<'a> {
        Call { project, living: OnceCell::new(), active: OnceCell::new(), home: OnceCell::new(), config: OnceCell::new() }
    }

    fn living(&self) -> Result<&[change::CapSpec]> {
        if let Some(living) = self.living.get() {
            return Ok(living);
        }
        let living = change::living(self.project)?;
        Ok(self.living.get_or_init(|| living))
    }

    fn active(&self) -> Result<&[proof::Active]> {
        if let Some(active) = self.active.get() {
            return Ok(active);
        }
        let active = proof::active(self.project, self.living()?)?;
        Ok(self.active.get_or_init(|| active))
    }

    /// The active changes whose vow names `branch`; `proof::on_branch` also
    /// counts the changes that a lease on `branch` works on.
    fn vowed_on(&self, branch: &str) -> Result<Vec<&proof::Active>> {
        Ok(self.active()?.iter().filter(|c| c.branch() == Some(branch)).collect())
    }

    fn home(&self) -> Option<&Path> {
        self.home.get_or_init(|| crate::store::home_dir().ok()).as_deref()
    }

    /// The config, or `None` when it does not load.
    fn config(&self) -> Option<&config::Config> {
        self.config.get_or_init(|| self.home().and_then(|h| config::load(self.project, h).ok())).as_ref()
    }
}

/// The argument names that carry a file path for a writing tool.
const FILE_ARGS: &[&str] =
    &["file_path", "filePath", "notebook_path", "path", "filename", "file", "dest", "destination", "target", "to", "output", "out"];

/// What a tool call can do, decided once from the tool's name, so the same rules
/// hold for every harness that sends its calls to the hooks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    /// Changes the files its arguments name.
    Editor,
    /// Runs a shell command, which the shell rules read.
    Shell,
    /// Changes no file.
    ReadOnly,
    /// Any other tool, such as an MCP tool or codemode: it may change files.
    Other,
}

/// The editor, shell and read-only tool names of Claude Code, pi, OpenCode,
/// Codex and Gemini CLI, compared without case. A name in none of them is
/// read from its words.
const EDITOR_TOOLS: &[&str] =
    &["edit", "write", "multiedit", "multi_edit", "notebookedit", "patch", "apply_patch", "write_file", "replace", "create_file"];
const SHELL_TOOLS: &[&str] =
    &["bash", "monitor", "powershell", "shell", "shell_command", "local_shell", "exec_command", "run_shell_command"];
const READ_TOOLS: &[&str] = &[
    "read",
    "grep",
    "glob",
    "ls",
    "list",
    "find",
    "notebookread",
    "webfetch",
    "web_fetch",
    "websearch",
    "google_web_search",
    "todowrite",
    "todoread",
    "task",
    "agent",
    "read_file",
    "read_many_files",
    "list_directory",
    "search_file_content",
];

/// The kind of the tool named `tool`.
pub fn tool_kind(tool: &str) -> ToolKind {
    let lower = tool.to_ascii_lowercase();
    if EDITOR_TOOLS.contains(&lower.as_str()) {
        return ToolKind::Editor;
    }
    if SHELL_TOOLS.contains(&lower.as_str()) {
        return ToolKind::Shell;
    }
    if READ_TOOLS.contains(&lower.as_str()) {
        return ToolKind::ReadOnly;
    }
    if writes_files(tool) { ToolKind::Other } else { ToolKind::ReadOnly }
}

/// The command of a shell tool call: a string, or an argv such as Codex's
/// `["bash", "-lc", "<command>"]`. Only a shell's `-c` text is a command;
/// another program's argv stays whole.
fn shell_command(args: &Value) -> Option<String> {
    match args.get("command")? {
        Value::String(c) => Some(c.clone()),
        Value::Array(words) => {
            let words: Vec<&str> = words.iter().filter_map(Value::as_str).collect();
            let is_shell = |w: &str| shell::SHELLS.contains(&w.rsplit('/').next().unwrap_or(w));
            match words.as_slice() {
                [sh, flag, cmd] if is_shell(sh) && matches!(*flag, "-c" | "-lc") => Some((*cmd).to_string()),
                _ => Some(words.join(" ")),
            }
        }
        _ => None,
    }
}

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

/// Whether a tool changes files, from its name: a known editor does, a known
/// reader or shell does not here, and any other tool does unless its name says
/// it only reads.
fn writes_files(tool: &str) -> bool {
    let lower = tool.to_ascii_lowercase();
    if EDITOR_TOOLS.contains(&lower.as_str()) {
        return true;
    }
    if READ_TOOLS.contains(&lower.as_str()) || SHELL_TOOLS.contains(&lower.as_str()) {
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

/// Why an MCP tool call must not run: an argument skips the git hooks, such as
/// `no_verify: true`, or names `core.hooksPath`. Nested arguments count too.
fn mcp_git_hooks_denial(args: &Value) -> Option<Denial> {
    fn walk(v: &Value) -> Option<&'static str> {
        match v {
            Value::Object(o) => o.iter().find_map(|(k, v)| {
                let key = k.to_ascii_lowercase().replace(['_', '-'], "");
                if key == "noverify" && v.as_bool() == Some(true) {
                    return Some("no_verify");
                }
                walk(v)
            }),
            Value::Array(a) => a.iter().find_map(walk),
            Value::String(s) if s.trim().to_ascii_lowercase().starts_with("core.hookspath") => Some("core.hooksPath"),
            _ => None,
        }
    }
    let what = walk(args)?;
    denial(
        Rule::GitHooks,
        format!(
            "{what} in this call skips the hooks that check the plan; call the tool without it, or run rvow block <task> --reason \"...\" if the plan needs it"
        ),
    )
}

/// The directory that holds the hidden checks, which the implementing agent
/// may not read or write.
fn secret_paths(call: &Call) -> Vec<PathBuf> {
    call.home().map(|h| vec![h.join("hidden-checks")]).unwrap_or_default()
}

/// The allowed-signers file that signed confirmations are checked against, as
/// the configured path resolves from `top`.
fn signers_path(call: &Call, top: &Path) -> Option<PathBuf> {
    let config = call.config()?;
    (!config.allowed_signers.is_empty()).then(|| top.join(&config.allowed_signers))
}

/// The text of `path` that the owner-command check reads: `None` for a clean
/// committed test file, only the lines added against HEAD for a tracked
/// test-glob file with changes, and the whole file for every other case. The
/// path is resolved against `cwd`, canonicalized, and must lie inside `top`.
/// Any path failure or git failure means the whole file, so the check fails
/// closed.
fn committed_test_text(call: &Call, top: Option<&Path>, path: &Path) -> Option<String> {
    let whole = || std::fs::read_to_string(path).ok();
    let Some(top) = top else { return whole() };
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return whole();
    }
    let Ok(path) = path.canonicalize() else { return whole() };
    let Ok(top) = top.canonicalize() else { return whole() };
    let Some(rel) = path.strip_prefix(&top).ok().and_then(|r| r.to_str()) else { return whole() };
    let Some(config) = call.config() else { return whole() };
    let Ok(tests) = fingerprint::globset(&config.test_globs) else { return whole() };
    if !tests.is_match(rel) {
        return whole();
    }
    if git::run(&top, &["--literal-pathspecs", "ls-files", "--error-unmatch", "--", rel]).is_err() {
        return whole();
    }
    match git::run(&top, &["--literal-pathspecs", "diff", "--no-color", "-U0", "HEAD", "--", rel]) {
        // A clean committed test file is data; none of it is read.
        Ok(diff) if diff.is_empty() => None,
        Ok(diff) => Some(added_lines(&diff)),
        Err(_) => whole(),
    }
}

/// The '+' lines of a unified diff without the '+++' header, each without its
/// '+' prefix.
fn added_lines(diff: &str) -> String {
    diff.lines().filter(|l| l.starts_with('+') && !l.starts_with("+++")).map(|l| &l[1..]).collect::<Vec<_>>().join("\n")
}

/// Files outside any repository that enforce rvow: the installed mod, Claude
/// Code's plugins and settings, the pi extension and the signing trust file.
fn enforcement_paths(trust: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    out.extend(crate::install::mod_dir());
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
fn edit_rule(call: &Call, top: &Path, cwd: &Path, file: &str) -> Result<Option<Denial>> {
    let project = call.project;
    let target = key(&real_path(Path::new(file), cwd));
    let trust = signers_path(call, top);
    for p in enforcement_paths(trust.as_deref()) {
        let p = key(&real_path(&p, cwd));
        if at_or_under(&target, &p) {
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
    if under(&target, &root)
        && (name == LEDGER
            || name == IDS_FILE
            || name == PROJECT_FILE
            || name == crate::record::RECORD_FILE
            || rel_under(&target, &root).is_some_and(|r| r.starts_with("objects/")))
    {
        return Ok(denial(
            Rule::Ledger,
            format!(
                "{name} is written only by rvow; run `rvow show <change>` to read it, or `rvow amend --reason \"...\"` to change the plan"
            ),
        ));
    }
    if rel_under(&target, &root).is_some_and(|r| r.starts_with("archive/")) {
        return Ok(denial(
            Rule::History,
            "an archived change is history and does not change; run `rvow new \"<title>\"` to start a new change".into(),
        ));
    }
    if rel_under(&target, &root).is_some_and(|r| r.starts_with("specs/")) {
        return Ok(denial(
            Rule::History,
            "living specs change only through a change and rvow archive; run rvow guide spec to see how".into(),
        ));
    }
    if rel_under(&target, &top_key).is_some_and(|r| r.starts_with(".git/")) {
        return Ok(denial(
            Rule::GitHooks,
            ".git/ is not edited by hand; the hooks there run the gate. Use git commands, and run rvow install for the hooks".into(),
        ));
    }
    let Some(rel) = rel_under(&target, &top_key) else {
        return Ok(None);
    };
    let in_store = under(&target, &root);
    if !in_store && let Some(id) = unaccepted_plan(call, top)? {
        return Ok(unaccepted_denial(&id));
    }
    if ENFORCEMENT.iter().any(|e| key(Path::new(e)) == rel) {
        return Ok(denial(
            Rule::EnforcementFile,
            format!(
                "{rel} configures the hooks that enforce the plan; changing it is for the user, so only the user edits it. If the plan needs it, run rvow block <task> --reason \"...\""
            ),
        ));
    }
    if protected_in(call, top)?.contains(rel) && !unlocked(call, top, rel)? {
        return Ok(denial(
            Rule::ProtectedTest,
            format!(
                "{rel} is a protected test or test setup file; to change it, run rvow amend --unlock {rel} --reason \"...\" and ask the user to confirm"
            ),
        ));
    }
    if !in_store
        && let Some(files) = active_scope(call, top)?
        && !files.is_match(rel)
        // check-ignore rejects --literal-pathspecs, so a leading ./ keeps a
        // magic-looking name literal while matching the same ignore rules.
        && git::run(top, &["check-ignore", "-q", "--", &format!("./{rel}")]).is_err()
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
fn active_scope(call: &Call, top: &Path) -> Result<Option<globset::GlobSet>> {
    let branch = git::branch(top)?.unwrap_or_default();
    let mine = proof::on_branch(call.active()?, &branch);
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
    rel_under(&target, &top).map(String::from)
}

/// Whether the user confirmed an unlock of `rel` and no proof of a task whose
/// files match it has happened since.
fn unlocked(call: &Call, top: &Path, rel: &str) -> Result<bool> {
    let branch = git::branch(top)?.unwrap_or_default();
    let mut open = false;
    for c in call.vowed_on(&branch)? {
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

fn unaccepted_denial(id: &crate::id::Id) -> Option<Denial> {
    denial(
        Rule::UnacceptedPlan,
        format!(
            "the plan {id} is not accepted yet; finish it, run rvow check, then ask the user to run /rvow-accept {id} (or ! rvow accept {id}) before you edit code"
        ),
    )
}

/// Why a shell command must not write the repository files it names as
/// targets: the plan on the branch is not accepted, or a leased subagent
/// writes outside its task. A file that a program writes without naming it,
/// or that only script text names, is not seen here.
fn shell_write_denial(call: &Call, cmd: &str, cwd: &Path, agent: Option<&str>) -> Result<Option<Denial>> {
    let project = call.project;
    let Ok(top) = git::toplevel(cwd) else { return Ok(None) };
    let root = key(&real_path(&project.root, cwd));
    let files: Vec<PathBuf> = shell::written_paths(cmd)
        .iter()
        .filter_map(|f| repo_rel(f, cwd, &top))
        .filter(|rel| !rel.starts_with(".git/"))
        .map(|rel| top.join(rel))
        .filter(|f| !under(&key(&real_path(f, cwd)), &root))
        .collect();
    if files.is_empty() {
        return Ok(None);
    }
    if let Some(id) = unaccepted_plan(call, &top)? {
        return Ok(unaccepted_denial(&id));
    }
    let Some(agent) = agent else { return Ok(None) };
    for f in &files {
        if let Some(found) = lease_denial(project, cwd, agent, &f.to_string_lossy())? {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

/// The backlog change whose plan blocks code edits: the branch has one and no
/// active change, so the user has not accepted a plan to work on yet.
fn unaccepted_plan(call: &Call, top: &Path) -> Result<Option<crate::id::Id>> {
    let Some(branch) = git::branch(top)? else {
        return Ok(None);
    };
    if !proof::on_branch(call.active()?, &branch).is_empty() {
        return Ok(None);
    }
    let (all, _) = change::list(call.project)?;
    for c in all.into_iter().filter(|c| c.stage == change::Stage::Backlog) {
        if change::load(&c.path, call.living()?)?.proposal.branch == branch {
            return Ok(Some(c.id));
        }
    }
    Ok(None)
}

/// The protected paths of every active change, from their baselines.
pub(crate) fn protected(project: &Project, top: &Path) -> Result<BTreeSet<String>> {
    protected_in(&Call::new(project), top)
}

fn protected_in(call: &Call, top: &Path) -> Result<BTreeSet<String>> {
    let project = call.project;
    let mut out = BTreeSet::new();
    let branch = git::branch(top)?.unwrap_or_default();
    for c in call.vowed_on(&branch)? {
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
fn secret_denial(call: &Call, cwd: &Path, file: &str) -> Option<Denial> {
    let target = key(&real_path(Path::new(file), cwd));
    secret_paths(call).into_iter().find_map(|p| {
        let p = key(&real_path(&p, cwd));
        if !at_or_under(&target, &p) {
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

/// Why a Bash command whose shell glob expands to a protected file must not run.
/// A glob hides the name from the text rules, so each glob word is expanded from
/// `cwd` and the result is matched by the rule that guards it.
fn glob_denial(call: &Call, cmd: &str, cwd: &Path, top: Option<&Path>, trust: Option<&Path>) -> Option<Denial> {
    let secrets = secret_paths(call);
    let enforcement = enforcement_paths(trust);
    let hooks = top.map(|t| t.join(".git/hooks"));
    let read_only = reads_only(cmd);
    for path in shell::glob_paths(cmd, cwd) {
        let real = key(&real_path(&path, cwd));
        let under = |p: &Path| {
            let p = key(&real_path(p, cwd));
            at_or_under(&real, &p)
        };
        if let Some(p) = secrets.iter().find(|p| under(p)) {
            return denial(
                Rule::HiddenChecks,
                format!(
                    "{} holds checks that stay hidden from the implementing agent; do not read or write them. If the plan needs them, run `rvow block <task> --reason \"...\"`",
                    p.display()
                ),
            );
        }
        if let Some(p) = enforcement.iter().find(|p| under(p)) {
            return denial(
                Rule::EnforcementFile,
                format!(
                    "{} enforces rvow; changing it is for the user. If the plan needs it, run `rvow block <task> --reason \"...\"`",
                    p.display()
                ),
            );
        }
        if hooks.as_ref().is_some_and(|h| under(h)) {
            return denial(
                Rule::GitHooks,
                ".git/hooks holds the pre-push gate; only rvow install writes it. Run rvow install instead".into(),
            );
        }
        if read_only {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == LEDGER || name == IDS_FILE {
            return denial(
                Rule::Ledger,
                format!(
                    "{LEDGER} and {IDS_FILE} are written only by rvow; run `rvow show <change>` to read the plan or `rvow note` to record something"
                ),
            );
        }
        if name == CACHE_FILE {
            return denial(Rule::CacheFile, format!("{CACHE_FILE} is written and read only by rvow gate; run `rvow gate --rerun` instead"));
        }
        if name == crate::store::INDEX_FILE {
            return denial(
                Rule::CacheFile,
                format!("{} is written and read only by rvow; delete it to make rvow look the project up again", crate::store::INDEX_FILE),
            );
        }
        if name == STATS_FILE || Some(name) == Path::new(stats::ROTATED_FILE).file_name().and_then(|n| n.to_str()) {
            return denial(
                Rule::StatsFile,
                format!("{STATS_FILE} is written and read only by rvow; run `rvow stats` to see the counts instead"),
            );
        }
        if name == YOLO_FILE {
            return denial(Rule::YoloFile, format!("{YOLO_FILE} is the user's switch; ask the user to run `! rvow yolo <tier>`"));
        }
        if name == LEASE_FILE {
            return denial(
                Rule::LeaseFile,
                format!("{LEASE_FILE} is written and read only by rvow; run `rvow delegate <task>` to lease a task instead"),
            );
        }
        if name == WITNESS_FILE {
            return denial(Rule::WitnessFile, witness::reason());
        }
    }
    None
}

/// Why `file` must not be read or written: it is the gate's cache file or the lease file.
fn cache_denial(project: &Project, cwd: &Path, file: &str) -> Option<Denial> {
    let target = key(&real_path(Path::new(file), cwd));
    let is = |name: &str| target == key(&real_path(&project.root.join(name), cwd));
    if is(CACHE_FILE) {
        return denial(Rule::CacheFile, format!("{CACHE_FILE} is written and read only by rvow gate; run `rvow gate --rerun` instead"));
    }
    if Path::new(file).file_name().and_then(|n| n.to_str()) == Some(crate::store::INDEX_FILE) {
        return denial(
            Rule::CacheFile,
            format!("{} is written and read only by rvow; delete it to make rvow look the project up again", crate::store::INDEX_FILE),
        );
    }
    if is(STATS_FILE) {
        return denial(
            Rule::StatsFile,
            format!("{STATS_FILE} is written and read only by rvow; run `rvow stats` to see the counts instead"),
        );
    }
    if is(YOLO_FILE) {
        return denial(Rule::YoloFile, format!("{YOLO_FILE} is the user's switch; ask the user to run `! rvow yolo <tier>`"));
    }
    if is(LEASE_FILE) {
        return denial(
            Rule::LeaseFile,
            format!("{LEASE_FILE} is written and read only by rvow; run `rvow delegate <task>` to lease a task instead"),
        );
    }
    if is(WITNESS_FILE) {
        return denial(Rule::WitnessFile, witness::reason());
    }
    None
}

/// The lease holder of a call: its subagent when that subagent holds a live
/// lease, else the session that the hook bound to a lease, so a subagent of a
/// leased session meets the session's lease. A subagent of no leased session
/// is its own holder, as before.
fn lease_holder<'a>(project: &Project, input: &'a Value) -> Option<&'a str> {
    let agent = input.get("agent_id").and_then(Value::as_str);
    let session = input.get("lease_holder").and_then(Value::as_str);
    let now = crate::ledger::now_ms();
    let holds = |id: &str| lease::leases(project).is_ok_and(|all| all.iter().any(|l| l.agent_id.as_deref() == Some(id) && l.live(now)));
    match (agent, session) {
        (Some(a), Some(s)) if !holds(a) => Some(s),
        (Some(a), _) => Some(a),
        (None, s) => s,
    }
}

/// Why a subagent that holds a lease must not edit `file`: it lies outside the
/// files of the task leased to it.
fn lease_denial(project: &Project, cwd: &Path, agent_id: &str, file: &str) -> Result<Option<Denial>> {
    let Ok(home) = crate::store::home_dir() else { return Ok(None) };
    let Some(lease) = lease::holder(project, &home, cwd, agent_id, crate::ledger::now_ms())? else {
        return Ok(None);
    };
    let top = git::toplevel(cwd)?;
    let base = lease.worktree.clone().unwrap_or(top);
    let target = key(&real_path(Path::new(file), cwd));
    let Some(rel) = target.strip_prefix(&format!("{}/", key(&real_path(&base, cwd)))) else {
        // A worktree lease keeps its writes out of the other worktrees of the
        // repository, such as the main checkout; a scratch file elsewhere is
        // no work of the repository.
        if lease.worktree.is_none() || !in_other_worktree(&base, &target) {
            return Ok(None);
        }
        return Ok(denial(
            Rule::LeaseScope,
            format!(
                "{file} is outside {worktree}, the worktree of {task}, the task leased to this agent; edit the files of {task} in that worktree",
                worktree = base.display(),
                task = lease.task
            ),
        ));
    };
    if fingerprint::globset(&lease.files).is_ok_and(|set| set.is_match(rel)) {
        return Ok(None);
    }
    Ok(denial(
        Rule::LeaseScope,
        format!(
            "{rel} is outside the files of {task}, the task leased to this agent; edit only {files}, or run rvow block {task} --reason \"...\" if the task needs it",
            task = lease.task,
            files = lease_files_text(&lease.files)
        ),
    ))
}

/// Whether `target` lies in a worktree of the repository of `worktree` other
/// than `worktree` itself. A git failure counts as yes, so a lease fails closed.
fn in_other_worktree(worktree: &Path, target: &str) -> bool {
    let Ok(list) = git::run(worktree, &["worktree", "list", "--porcelain"]) else { return true };
    list.lines().filter_map(|l| l.strip_prefix("worktree ")).any(|w| {
        let w = key(&std::fs::canonicalize(w).unwrap_or_else(|_| PathBuf::from(w)));
        target.starts_with(&format!("{}/", w.trim_end_matches('/')))
    })
}

const MAX_LEASE_FILES: usize = 5;

/// The leased globs for a deny text: at most `MAX_LEASE_FILES`, then a count.
fn lease_files_text(files: &[String]) -> String {
    let more = files.len().saturating_sub(MAX_LEASE_FILES);
    let mut shown = files[..files.len() - more].join(", ");
    if more > 0 {
        shown.push_str(&format!(" and {more} more"));
    }
    shown
}

/// Why a subagent that holds a lease must not run this `rvow prove`: it proves
/// only the task leased to it, never another task or `--stale`.
fn lease_prove_denial(project: &Project, cwd: &Path, agent_id: &str, cmd: &str) -> Result<Option<Denial>> {
    let proves = prove_args(cmd);
    if proves.is_empty() {
        return Ok(None);
    }
    let Ok(home) = crate::store::home_dir() else { return Ok(None) };
    let Some(lease) = lease::holder(project, &home, cwd, agent_id, crate::ledger::now_ms())? else {
        return Ok(None);
    };
    let own = |args: &Vec<String>| matches!(args.as_slice(), [task] if !task.starts_with('-') && lease.task.starts_with(task.as_str()));
    if proves.iter().all(own) {
        return Ok(None);
    }
    Ok(denial(
        Rule::LeaseScope,
        format!("rvow prove from this agent covers only {task}, the task leased to it; run `rvow prove {task}`", task = lease.task),
    ))
}

/// The rule and the reason that deny a tool call, or `None` to allow it.
pub fn pre_tool_use_denial(project: &Project, input: &Value) -> Result<Option<Denial>> {
    denial_in(&Call::new(project), input)
}

/// The tool input of a call, borrowed: an Edit or Write input holds whole file texts.
fn tool_input(input: &Value) -> &Value {
    static NONE: Value = Value::Null;
    input.get("tool_input").unwrap_or(&NONE)
}

fn denial_in(call: &Call, input: &Value) -> Result<Option<Denial>> {
    let project = call.project;
    let cwd = PathBuf::from(input.get("cwd").and_then(Value::as_str).unwrap_or("."));
    let tool = input.get("tool_name").and_then(Value::as_str).unwrap_or("");
    let args = tool_input(input);
    if tool_kind(tool) == ToolKind::Shell {
        let Some(c) = shell_command(args) else {
            return Ok(None);
        };
        let c = c.as_str();
        let top = git::toplevel(&cwd).ok();
        let trust = top.as_deref().and_then(|t| signers_path(call, t));
        let committed = |path: &Path| committed_test_text(call, top.as_deref(), path);
        let found = bash_rule(c)
            .or_else(|| shell::script_rule_committed(c, &cwd, &committed))
            .or_else(|| glob_denial(call, c, &cwd, top.as_deref(), trust.as_deref()))
            .or_else(|| bash_store_denial(project, &cwd, c))
            .or_else(|| bash_secret_rule(c, &secret_paths(call)))
            .or_else(|| bash_enforcement_rule(c, &enforcement_paths(trust.as_deref())));
        let agent = lease_holder(project, input);
        if found.is_some() {
            return Ok(found);
        }
        if let Some(found) = shell_write_denial(call, c, &cwd, agent)? {
            return Ok(Some(found));
        }
        return match agent {
            Some(agent) => lease_prove_denial(project, &cwd, agent, c),
            None => Ok(None),
        };
    }
    if tool.starts_with("mcp__")
        && let Some(found) = mcp_git_hooks_denial(args)
    {
        return Ok(Some(found));
    }
    let files = file_args(args);
    // The hidden checks are closed to every tool. The plan files of the store are closed to direct
    // reads, and the stats, lease, yolo and cache files are only closed to writes.
    let writer = writes_files(tool);
    if let Some(found) = files.iter().find_map(|f| {
        secret_denial(call, &cwd, f)
            .or_else(|| (!writer).then(|| store_denial(project, &cwd, f, false)).flatten())
            .or_else(|| writer.then(|| cache_denial(project, &cwd, f)).flatten())
    }) {
        return Ok(Some(found));
    }
    if !writes_files(tool) {
        return Ok(None);
    }
    if files.is_empty() {
        return Ok(None);
    }
    let agent = lease_holder(project, input);
    let top = git::toplevel(&cwd)?;
    for f in &files {
        let mut found = edit_rule(call, &top, &cwd, f)?;
        if found.is_none() {
            found = store_denial(project, &cwd, f, true);
        }
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

/// What the harness saw the user do in this session, from the `user_made`
/// field that an adapter such as the Claude mod or the pi extension sends with
/// each call: since when it watched, the changes the user accepted, the
/// yolo tiers the user switched on, and the time spans in which the harness
/// itself ran an accept, confirm or archive for the user. A harness that sends no
/// such field has nothing to tell, and the checks that need it stay off.
struct UserMade {
    since: u64,
    changes: Vec<String>,
    yolo: Vec<UserYolo>,
    runs: Vec<(u64, u64)>,
}

struct UserYolo {
    tier: Tier,
    at: u64,
    until: u64,
}

/// How long after the user's command a yolo file may say it was switched on.
const YOLO_SKEW_MS: u64 = 60_000;

impl UserMade {
    fn from(project: &Project, input: &Value) -> Option<UserMade> {
        let record = witness::merged(project, input.get("user_made")?);
        let seen = &record;
        let since = seen.get("since").and_then(Value::as_u64)?;
        let changes =
            seen.get("changes").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
        let yolo = seen
            .get("yolo")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|y| {
                let tier = serde_json::from_value(y.get("tier")?.clone()).ok()?;
                Some(UserYolo { tier, at: y.get("at")?.as_u64()?, until: y.get("until")?.as_u64()? })
            })
            .collect();
        let runs = seen
            .get("runs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|r| Some((r.get("at")?.as_u64()?, r.get("until")?.as_u64()?)))
            .collect();
        Some(UserMade { since, changes, yolo, runs })
    }

    /// The tier `on` when the user switched it on, or when it was on before the session began.
    /// A file from before the switch time was recorded has 0, so it counts as before.
    fn yolo(&self, on: yolo::On) -> Option<yolo::On> {
        let theirs =
            |y: &UserYolo| y.tier == on.tier && on.at + 1_000 >= y.at && on.at <= y.at + YOLO_SKEW_MS && on.until <= y.until + YOLO_SKEW_MS;
        (on.at < self.since || self.yolo.iter().any(theirs)).then_some(on)
    }

    /// Whether the user stands behind an accept or confirm of `change` at `time`.
    fn made(&self, change: &str, time: u64) -> bool {
        time < self.since
            || self.changes.iter().any(|c| c == change)
            || self.runs.iter().any(|(at, until)| *at <= time && time <= *until)
            || self.yolo.iter().any(|y| y.tier >= Tier::Auto && y.at <= time && time <= y.until)
    }
}

/// Why work on the branch must wait: an active change was accepted, or an
/// amendment of it confirmed, in this session without the user, as with a
/// copy of the rvow binary under another name.
fn unseen_denial(call: &Call, input: &Value, seen: &UserMade) -> Result<Option<Denial>> {
    let tool = input.get("tool_name").and_then(Value::as_str).unwrap_or("");
    let kind = tool_kind(tool);
    if kind == ToolKind::ReadOnly || (kind != ToolKind::Shell && !writes_files(tool)) {
        return Ok(None);
    }
    let cwd = PathBuf::from(input.get("cwd").and_then(Value::as_str).unwrap_or("."));
    let Ok(top) = git::toplevel(&cwd) else { return Ok(None) };
    let in_repo = !changed_rels(input, &cwd, &top).is_empty();
    let names_no_file = kind != ToolKind::Shell && file_args(tool_input(input)).is_empty();
    if !in_repo && !names_no_file {
        return Ok(None);
    }
    let Some(branch) = git::branch(&top)? else { return Ok(None) };
    for c in proof::on_branch(call.active()?, &branch) {
        let id = c.dir.id.to_string();
        let accepted = |e: &&crate::ledger::Event| {
            e.kind == Kind::Vow || (e.kind == Kind::Confirm && e.data.get("decision").and_then(Value::as_str) == Some("accept"))
        };
        if c.events.iter().filter(accepted).any(|e| !seen.made(&id, e.time)) {
            return Ok(denial(
                Rule::UnseenAccept,
                format!(
                    "{id} was accepted or confirmed in this session, but not by the user in a dialog; ask the user to check it and type /rvow-accept {id}"
                ),
            ));
        }
    }
    Ok(None)
}

/// Claude Code `PreToolUse`: a deny or ask response, or `None` to allow.
/// A rule in audit mode lets the call through and is only counted. After
/// `BACKSTOP` denials in a row the next one is a question for the user.
pub fn claude_pre_tool_use(project: &Project, input: &Value) -> Result<Option<Value>> {
    let session = input.get("session_id").and_then(Value::as_str);
    let seen = UserMade::from(project, input);
    let tier = yolo::active(project).and_then(|on| seen.as_ref().map_or(Some(on), |s| s.yolo(on))).map(|on| on.tier);
    let call = Call::new(project);
    let unseen = match &seen {
        Some(s) => unseen_denial(&call, input, s)?,
        None => None,
    };
    let found = unseen.map_or_else(|| denial_in(&call, input), |d| Ok(Some(d)))?;
    // The auto tier lets the agent accept and confirm; every other owner-only call stays denied.
    let command = input.get("tool_input").and_then(|t| t.get("command")).and_then(Value::as_str);
    let mode = |d: &Denial| {
        if tier >= Some(Tier::Auto) && d.rule == Rule::OwnerCommand && command.is_some_and(only_accept_or_confirm) {
            Mode::Audit
        } else {
            d.rule.mode_in(tier)
        }
    };
    if let Some(d) = found.as_ref().filter(|d| mode(d) == Mode::Audit) {
        stats::record(project, Hook::PreToolUse, Outcome::Audit, Some(d.rule), session);
    }
    let Some(d) = found.filter(|d| mode(d) == Mode::Enforce) else {
        if let Some(s) = session {
            streak::reset(project, s);
            record_changes(project, s, input);
            if record_before_snapshot(project, s, input) == Locked::Busy {
                return Ok(Some(json!({"hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": format!(
                        "rvow: the project is busy: another rvow process held its lock for {} s; try the call again in a moment",
                        crate::provenance::LOCK_WAIT.as_secs()
                    ),
                }})));
            }
        }
        return Ok(None);
    };
    let denied = session.map(|s| streak::add(project, s));
    let asked = !d.rule.tampering() && denied.is_some_and(|n| n > streak::BACKSTOP);
    if asked && let Some(s) = session {
        streak::reset(project, s);
    }
    let (outcome, decision, reason) = if asked {
        (
            Outcome::Ask,
            "ask",
            format!(
                "rvow: this session was denied {} times in a row. Allow this call? The rule is {}: {}",
                streak::BACKSTOP,
                d.rule.name(),
                d.reason
            ),
        )
    } else {
        (Outcome::Deny, "deny", format!("rvow: {}", d.reason))
    };
    stats::record(project, Hook::PreToolUse, outcome, Some(d.rule), session);
    Ok(Some(json!({"hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": decision,
        "permissionDecisionReason": reason,
    }})))
}

/// Claude Code `PreToolUse` when the hook itself failed: a deny for a tool
/// that writes, so a broken store does not turn the checks off, and `None`
/// for a reading tool.
pub fn claude_pre_tool_use_failure(input: &Value, error: &crate::error::Error) -> Option<Value> {
    let tool = input.get("tool_name").and_then(Value::as_str)?;
    if tool_kind(tool) == ToolKind::ReadOnly {
        return None;
    }
    let reason = format!(
        "rvow: the hook failed, so tools that write stay denied until it works: {}: {}. Fix: {}. If rvow itself is broken, run `rvow install` again",
        error.code, error.message, error.fix
    );
    Some(json!({"hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "deny",
        "permissionDecisionReason": reason,
    }}))
}

/// The path of `file` relative to the repository top in its real case, or `None` outside it.
fn repo_rel(file: &str, cwd: &Path, top: &Path) -> Option<String> {
    let target = real_path(Path::new(file), cwd);
    let top = real_path(top, cwd);
    target.strip_prefix(&top).ok().and_then(Path::to_str).filter(|r| !r.is_empty()).map(String::from)
}

/// Adds the files an allowed tool call may change to the record of its owner: the
/// subagent record when the input carries an agent id, else the main session record.
/// The main session stop reads only the main record.
fn record_changes(project: &Project, session: &str, input: &Value) {
    let cwd = PathBuf::from(input.get("cwd").and_then(Value::as_str).unwrap_or("."));
    let Ok(top) = git::toplevel(&cwd) else { return };
    let rels = changed_rels(input, &cwd, &top);
    if let Some(agent) = input.get("agent_id").and_then(Value::as_str) {
        session_files::record_agent(project, session, agent, &rels);
    } else {
        session_files::record(project, session, &rels);
    }
}

/// The repo-relative files a tool call may change: the file arguments of a writing tool, and the
/// files that a Bash command writes. Empty for a call that only reads.
fn changed_rels(input: &Value, cwd: &Path, top: &Path) -> Vec<String> {
    let tool = input.get("tool_name").and_then(Value::as_str).unwrap_or("");
    let args = input.get("tool_input").cloned().unwrap_or(Value::Null);
    let cwd = cwd.to_path_buf();
    if tool_kind(tool) == ToolKind::Shell {
        let Some(c) = shell_command(&args).filter(|c| !reads_only(c)) else { return Vec::new() };
        let changes = bash_changes(&c, &cwd);
        let bases: Vec<PathBuf> = std::iter::once(cwd.clone()).chain(changes.dirs.iter().map(|d| cwd.join(d))).collect();
        let mut seen = std::collections::BTreeSet::new();
        let mut rels: Vec<String> = Vec::new();
        let mut push = |rel: String| {
            if seen.insert(rel.clone()) {
                rels.push(rel);
            }
        };
        for (f, base) in changes.paths.iter().flat_map(|f| bases.iter().map(move |base| (f, base))) {
            let at = base.join(f);
            if at.is_dir() {
                continue;
            }
            let writable = at.exists() || at.parent().is_some_and(Path::is_dir);
            if !writable {
                continue;
            }
            if at.exists() && std::fs::metadata(&at).is_ok_and(|m| m.is_dir()) {
                continue;
            }
            if let Some(rel) = repo_rel(f, base, top) {
                push(rel);
            }
        }
        for (f, base) in changes.script_paths.iter().flat_map(|f| bases.iter().map(move |base| (f, base))) {
            let at = base.join(f);
            if at.is_dir() {
                continue;
            }
            let keep = at.is_file() || (f.contains('/') && at.parent().is_some_and(Path::is_dir));
            if !keep {
                continue;
            }
            if let Some(rel) = repo_rel(f, base, top) {
                push(rel);
            }
        }
        if changes.plan {
            push(session_files::PLAN.to_string());
        }
        rels
    } else if writes_files(tool) {
        file_args(&args).iter().filter_map(|f| repo_rel(f, &cwd, top)).collect()
    } else {
        Vec::new()
    }
}

/// Whether a tool call may write the work tree, so a before snapshot is taken
/// around it. Read, Grep, Glob and a read-only Bash command are skipped.
fn may_write(tool: &str, args: &Value) -> bool {
    match tool_kind(tool) {
        ToolKind::Editor | ToolKind::Other => true,
        ToolKind::Shell => shell_command(args).is_some_and(|c| !reads_only(&c)),
        ToolKind::ReadOnly => false,
    }
}

/// The pre-tool-use provenance: snapshots the work tree, or the files an
/// editor call names, before an allowed write call, and stores the before tree
/// per (session, agent, tool_use_id).
fn record_before_snapshot(project: &Project, session: &str, input: &Value) -> Locked {
    let Some(id) = input.get("tool_use_id").and_then(Value::as_str) else {
        return Locked::Taken;
    };
    let tool = input.get("tool_name").and_then(Value::as_str).unwrap_or("");
    let args = input.get("tool_input").cloned().unwrap_or(Value::Null);
    if !may_write(tool, &args) {
        return Locked::Taken;
    }
    let cwd = PathBuf::from(input.get("cwd").and_then(Value::as_str).unwrap_or("."));
    let Ok(top) = git::toplevel(&cwd) else { return Locked::Taken };
    let agent = input.get("agent_id").and_then(Value::as_str);
    // The editors and the shell keep a before snapshot, since overlapping calls of
    // other sessions are judged by it; any other writer takes one snapshot after.
    if matches!(tool_kind(tool), ToolKind::Editor | ToolKind::Shell) {
        let paths = if tool_kind(tool) == ToolKind::Editor { named_paths(&args, &cwd, &top) } else { Vec::new() };
        crate::provenance::before_call(project, &top, session, agent, id, &paths)
    } else {
        crate::provenance::before_call_light(project, &top, session, agent, id)
    }
}

/// The repo-relative files an editor call names, when every file it names is
/// inside the work tree: its two snapshots then hash only those, so a change a
/// background job makes elsewhere is not recorded as this call's work. A call
/// that names one file outside the work tree, or none, scans the whole tree.
fn named_paths(args: &Value, cwd: &Path, top: &Path) -> Vec<String> {
    let named = file_args(args);
    let paths: Vec<String> = named.iter().filter_map(|f| repo_rel(f, cwd, top)).collect();
    if paths.len() == named.len() { paths } else { Vec::new() }
}

/// Claude Code `PostToolUse`: snapshots the same paths again after a checked
/// call, diffs it against the before snapshot of the same call, and records
/// each changed path with its new blob id for the session (or its subagent). A
/// call with no before record (a read) takes no snapshot and records nothing.
pub fn claude_post_tool_use(project: &Project, input: &Value) -> Result<Option<Value>> {
    let Some(session) = input.get("session_id").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(id) = input.get("tool_use_id").and_then(Value::as_str) else {
        return Ok(None);
    };
    let cwd = PathBuf::from(input.get("cwd").and_then(Value::as_str).unwrap_or("."));
    let Ok(top) = git::toplevel(&cwd) else {
        return Ok(None);
    };
    let agent = input.get("agent_id").and_then(Value::as_str);
    crate::provenance::after_call(project, &top, session, agent, id);
    Ok(None)
}

/// Whether drift item text names `path` as a whole word, so a short name
/// such as `a` does not match every item.
fn names_path(what: &str, path: &str) -> bool {
    what.match_indices(path).any(|(i, _)| {
        let before = what[..i].chars().next_back();
        let after = what[i + path.len()..].chars().next();
        before.is_none_or(|c| matches!(c, ' ' | '(' | ',')) && after.is_none_or(|c| matches!(c, ' ' | ')' | ',' | ';' | '#' | ':'))
    })
}

/// The Claude mod's `agent.spawn`: binds the new subagent to the lease that its
/// prompt names. Prints nothing for a prompt without a marker.
pub fn claude_lease_bind(project: &Project, input: &Value) -> Result<Option<Value>> {
    let text = |k: &str| input.get(k).and_then(Value::as_str);
    let (Some(agent), Some(task)) = (text("agent_id"), text("prompt").and_then(lease::marker)) else {
        return Ok(None);
    };
    let home = crate::store::home_dir()?;
    let cwd = PathBuf::from(text("cwd").unwrap_or("."));
    Ok(Some(match lease::bind_checked(project, &task, agent, |held| lease::task_open(project, &home, &cwd, held))? {
        lease::Bound::Holds(held) => json!({
            "bound": false,
            "task": task,
            "reason": format!("this session still holds the lease of {held}; prove or block {held}, or start a new session for {task}"),
        }),
        bound => json!({"bound": bound == lease::Bound::Yes, "task": task}),
    }))
}

/// The Claude mod's `SubagentStop`: ends the lease that the stopping subagent holds.
pub fn claude_lease_end(project: &Project, input: &Value) -> Result<Option<Value>> {
    if let Some(agent) = input.get("agent_id").and_then(Value::as_str) {
        lease::end(project, agent)?;
    }
    Ok(None)
}

/// Claude Code `SessionStart`: the brief with each of `warnings` as added
/// context, or `None` when there is nothing new for the session. Outside a
/// project only the warnings go.
pub fn claude_session_start(project: Option<&Project>, home: &Path, input: &Value, warnings: &[String]) -> Result<Option<Value>> {
    let cwd = PathBuf::from(input.get("cwd").and_then(Value::as_str).unwrap_or("."));
    let agent = project.and_then(|p| lease_holder(p, input));
    let mut text = match project {
        Some(p) => brief::brief_for(p, home, &cwd, agent)?,
        None => String::new(),
    };
    if let Some(on) = project.and_then(yolo::active) {
        let warning = format!("rvow: yolo {} is on and ends by itself; the hooks relax, the gate still checks at push", on.tier.name());
        text = brief::with_warning(&text, &warning);
    }
    for warning in warnings {
        text = brief::with_warning(&text, warning);
    }
    let state = project.map_or_else(|| home.join("hook-state"), |p| p.root.join("hook-state"));
    let session = input.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty());
    let send = match session {
        Some(s) if !text.is_empty() => {
            if matches!(input.get("source").and_then(Value::as_str), Some("compact" | "clear")) {
                brief_state::clear(&state, s);
            }
            let sent = brief_state::load(&state, s);
            brief_state::store(&state, s, &text);
            match sent {
                Some(last) => brief_delta(&last, &text),
                None => Some(text),
            }
        }
        _ => Some(text),
    };
    Ok(send.filter(|t| !t.is_empty()).map(|text| {
        json!({"hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": text,
        }})
    }))
}

/// What a session still needs of `text` after it got `last`: nothing when only the counts changed,
/// the tail from the `next:` line when only that moved, else the full brief.
fn brief_delta(last: &str, text: &str) -> Option<String> {
    if last == text {
        return None;
    }
    let (Some((old_head, old_tail)), Some((new_head, new_tail))) = (split_next(last), split_next(text)) else {
        return Some(text.to_string());
    };
    if head_without_tasks(old_head) != head_without_tasks(new_head) {
        return Some(text.to_string());
    }
    if old_tail == new_tail { None } else { Some(new_tail.to_string()) }
}

/// The brief before its last `next:` line and the tail from that line on, or `None` without one.
/// The last one, since a note in the head may hold a line that starts with `next:`.
fn split_next(text: &str) -> Option<(&str, &str)> {
    let mut at = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if line.starts_with("next:") {
            at = Some(offset);
        }
        offset += line.len();
    }
    at.map(|at| text.split_at(at))
}

/// The lines before the tail without the `tasks:` line, which only holds counts.
fn head_without_tasks(head: &str) -> String {
    head.lines().filter(|line| !line.starts_with("tasks:")).collect::<Vec<_>>().join("\n")
}

const MAX_DRIFT_ITEMS: usize = 3;

/// The stop text for a plan change without an amendment: at most `MAX_DRIFT_ITEMS` items, then a count.
fn unamended_text(blocking: &[String]) -> String {
    let more = blocking.len().saturating_sub(MAX_DRIFT_ITEMS);
    let mut shown = blocking[..blocking.len() - more].join("; ");
    if more > 0 {
        shown.push_str(&format!("; and {more} more"));
    }
    format!(
        "the plan changed without an amendment ({shown}); run rvow amend --reason \"...\"; the user confirms it when the change is archived"
    )
}

/// True while a rebase, merge or cherry-pick is half done in the repository at `top`.
fn git_operation_running(top: &Path) -> bool {
    // The project lookup of this call already printed the git dir.
    let dir = match crate::store::known_dirs(top) {
        Some((git_dir, _)) => git_dir,
        None => {
            let Ok(dir) = git::run(top, &["rev-parse", "--absolute-git-dir"]) else {
                return false;
            };
            PathBuf::from(dir.trim())
        }
    };
    ["rebase-merge", "rebase-apply", "MERGE_HEAD", "CHERRY_PICK_HEAD"].iter().any(|n| dir.join(n).exists())
}

/// Why a stop is held back, and the kind of reason that the stats count.
pub struct StopHold {
    pub kind: StopReason,
    pub text: String,
}

/// One reason of the session's stop block, with the kind that the stats count.
struct OpenItem {
    kind: StopReason,
    text: String,
}

const MAX_STOP_ITEMS: usize = 3;

/// One block reason for every open item: a single item keeps its own text, and more are listed
/// up to `MAX_STOP_ITEMS` and then counted.
fn stop_hold(items: Vec<OpenItem>) -> Option<StopHold> {
    let mut items = items.into_iter();
    let first = items.next()?;
    let rest: Vec<OpenItem> = items.collect();
    if rest.is_empty() {
        return Some(StopHold { kind: first.kind, text: first.text });
    }
    let total = rest.len() + 1;
    let more = total.saturating_sub(MAX_STOP_ITEMS);
    let mut shown = vec![first.text];
    shown.extend(rest.into_iter().take(MAX_STOP_ITEMS - 1).map(|i| i.text));
    let mut text = format!("{total} open items: {}", shown.join("; "));
    if more > 0 {
        text.push_str(&format!("; and {more} more"));
    }
    Some(StopHold { kind: first.kind, text })
}

/// Why the agent must keep working, or `None` when it may stop. Only the session that changed a
/// file of a task, or a file that drift names, is held. A change with a pending amendment holds
/// nothing, since only the user can decide it.
pub fn stop_reason(project: &Project, home: &Path, cwd: &Path, session: Option<&str>) -> Result<Option<StopHold>> {
    let top = git::toplevel(cwd)?;
    let Some(branch) = git::branch(&top)? else {
        return Ok(None);
    };
    if git_operation_running(&top) {
        return Ok(None);
    }
    let living = change::living(project)?;
    let changes = proof::active(project, &living)?;
    let queue = proof::on_branch(&changes, &branch);
    if queue.is_empty() {
        return Ok(None);
    }
    let pending = amend::pending(&changes);
    let config = config::load(project, home)?;
    // The files the session owns: its latest blob record matches the current
    // blob. A session without after-call records (an older mod) decides from
    // today's text record.
    let touched = match session {
        Some(s) => match crate::provenance::owned(project, &top, s)? {
            Some(mut set) => {
                if session_files::load(project, s).contains(session_files::PLAN) {
                    set.insert(session_files::PLAN.to_string());
                }
                set
            }
            None => session_files::load(project, s),
        },
        None => BTreeSet::new(),
    };
    let mut items: Vec<OpenItem> = Vec::new();
    let mut protected_files = None;
    for c in &queue {
        if pending.iter().any(|p| p.change == c.dir.id) {
            continue;
        }
        let blocking: Vec<String> = drift::drift_over(project, Some(home), &top, &config, c, &living)?
            .into_iter()
            .filter(|i| i.class.blocks())
            .map(|i| i.what)
            .collect();
        if blocking.is_empty() {
            continue;
        }
        let guarded = match &protected_files {
            Some(set) => set,
            None => protected_files.insert(protected(project, &top)?),
        };
        let own = touched.iter().any(|f| {
            f == session_files::ANY
                || f == session_files::PLAN
                || guarded.contains(&key(Path::new(f)))
                || blocking.iter().any(|what| names_path(what, f))
        });
        if !own {
            continue;
        }
        items.push(OpenItem { kind: StopReason::Drift, text: unamended_text(&blocking) });
    }
    let now = Now { top: &top, config: &config, env: fingerprint::check_env(&top, &config)?, living: &living };
    let leases = lease::leases(project)?;
    // Only the newest accepted change is the agent's current plan; older changes on the branch are
    // enforced by the pre-push gate, and nagging about them each time hides the plan that matters.
    let newest = queue.iter().max_by_key(|c| c.vow().map_or(0, |e| e.time));
    for c in queue.iter().filter(|c| newest.is_some_and(|n| std::ptr::eq(*c, n))) {
        if pending.iter().any(|p| p.change == c.dir.id) {
            continue;
        }
        for t in &c.loaded.tasks {
            let s = proof::state(c, t, &now)?;
            if !s.open() {
                continue;
            }
            // Stale proofs are left to the push gate.
            if matches!(s, State::Stale(_) | State::Flaky) {
                continue;
            }
            let scope = fingerprint::globset(&t.files)?;
            if !touched.iter().any(|f| f == session_files::ANY || scope.is_match(f)) {
                continue;
            }
            let id = t.id.to_string();
            if lease::leased_away_in(&leases, &id, session, crate::ledger::now_ms()) {
                continue;
            }
            let text = format!("{id} has no proof: run rvow prove {id}, or rvow block {id} --reason \"...\" if it cannot pass as written");
            items.push(OpenItem { kind: StopReason::NoProof, text });
        }
    }
    Ok(stop_hold(items))
}

/// One line for a status bar: proved tasks, drift and pending amendments of
/// the branch's active changes, or `None` when the branch has none.
pub fn status_line(project: &Project, home: &Path, cwd: &Path) -> Result<Option<String>> {
    let line = status_line_inner(project, home, cwd)?;
    Ok(match (line, yolo::active(project)) {
        (line, None) => line,
        (Some(line), Some(on)) => Some(format!("{line} [yolo {}]", on.tier.name())),
        (None, Some(on)) => Some(format!("rvow [yolo {}]", on.tier.name())),
    })
}

fn status_line_inner(project: &Project, home: &Path, cwd: &Path) -> Result<Option<String>> {
    let top = git::toplevel(cwd)?;
    let Some(branch) = git::branch(&top)? else {
        return Ok(None);
    };
    let living = change::living(project)?;
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
    let now = Now { top: &top, config: &config, env: fingerprint::check_env(&top, &config)?, living: &living };
    let (mut proved, mut total, mut drift_count) = (0, 0, 0);
    for c in &queue {
        for t in &c.loaded.tasks {
            total += 1;
            if proof::state(c, t, &now)? == State::Proved {
                proved += 1;
            }
        }
        drift_count += drift::drift_over(project, Some(home), &top, &config, c, &living)?.iter().filter(|i| i.class.blocks()).count();
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
    let kind = out.as_ref().map(|(_, kind)| *kind);
    stats::record_stop(project, outcome, kind, input.get("session_id").and_then(Value::as_str));
    Ok(out.map(|(block, _)| block))
}

fn stop_decision(project: &Project, home: &Path, input: &Value) -> Result<Option<(Value, StopReason)>> {
    let cwd = PathBuf::from(input.get("cwd").and_then(Value::as_str).unwrap_or("."));
    let flag = |k: &str| input.get(k).and_then(Value::as_bool).unwrap_or(false);
    if input.get("agent_id").is_some() || flag("stop_hook_active") {
        return Ok(None);
    }
    if yolo::active(project).is_some_and(|on| on.tier == Tier::Bypass) {
        return Ok(None);
    }
    let session = input.get("session_id").and_then(Value::as_str);
    // A change that no call of the session recorded, such as a background job's, still counts.
    if let (Some(s), Ok(top)) = (session, git::toplevel(&cwd)) {
        crate::provenance::catch_up_at_stop(project, &top, s, None);
    }
    let Some(hold) = stop_reason(project, home, &cwd, session)? else {
        if let Some(s) = session {
            stop_state::clear(project, s);
        }
        return Ok(None);
    };
    if session.is_some_and(|s| stop_state::already_given(project, s, &hold.text)) {
        return Ok(None);
    }
    Ok(Some((json!({"decision": "block", "reason": format!("rvow: {}", hold.text)}), hold.kind)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proof::tests::accepted;

    #[test]
    fn any_tool_change_only_a_shell_argv_drops_the_program() {
        let cmd = |v: Value| shell_command(&json!({ "command": v }));
        assert_eq!(cmd(json!(["/bin/bash", "-lc", "rvow accept CHG-x"])).as_deref(), Some("rvow accept CHG-x"));
        assert_eq!(cmd(json!(["python3", "-c", "print(1)"])).as_deref(), Some("python3 -c print(1)"));
    }

    #[test]
    fn a_literal_glob_matches_a_casewise_edited_path_on_macos() {
        // `key` lowercases the edited path on macOS; the scope must do the same
        // to the vowed globs or a literal glob like SKILL.md.tmpl never matches.
        let rel = &key(Path::new("skills/rvow/SKILL.md.tmpl"));
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

    fn items(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("f{i}")).collect()
    }

    #[test]
    fn lease_deny_lists_five_files_and_counts_the_rest() {
        let files: Vec<String> = (0..8).map(|i| format!("f{i}.rs")).collect();
        assert_eq!(lease_files_text(&files), "f0.rs, f1.rs, f2.rs, f3.rs, f4.rs and 3 more");
        assert_eq!(lease_files_text(&files[..2]), "f0.rs, f1.rs");
    }

    #[test]
    fn confirm_once_at_archive_lists_three_and_counts_the_rest() {
        let text = unamended_text(&items(5));
        assert!(text.contains("f1; f2; f3; and 2 more)"), "{text}");
        assert!(!text.contains("f4"), "{text}");
    }

    #[test]
    fn confirm_once_at_archive_lists_one_item_without_a_count() {
        let text = unamended_text(&items(1));
        assert!(text.contains("(f1)") && !text.contains("more"), "{text}");
    }

    #[test]
    fn confirm_once_at_archive_names_amend_and_not_confirm() {
        let text = unamended_text(&items(2));
        assert!(text.contains("rvow amend --reason"), "{text}");
        assert!(!text.contains("/rvow-confirm") && !text.contains("rvow confirm"), "{text}");
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
            "the plan changed without an amendment (a); run rvow amend --reason \"...\"; the user confirms it when the change is archived",
            "rvow accept belongs to the user; the hooks deny it for agents. Ask the user to run /rvow-accept <id>, or to type ! rvow accept",
            "--no-verify skips the hooks that check the plan; run git commit and git push without it",
            "git commit -n skips the hooks that check the plan; run git commit without -n",
            "core.hooksPath decides which hooks run; changing it is for the user. If the plan needs it, run rvow block <task> --reason \"...\"",
            ".git/hooks holds the pre-push gate; only rvow install writes it. Run rvow install instead",
        ] {
            denial_has_a_next_step(reason);
        }
    }

    fn aged(path: &Path, days: u64) {
        let when = std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86_400);
        std::fs::File::options().write(true).open(path).unwrap().set_modified(when).unwrap();
    }

    fn hook_state(p: &Project) -> PathBuf {
        let dir = p.root.join("hook-state");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn prune_session_removes_a_record_older_than_7_days() {
        let (_tmp, p, _repo, _task) = accepted();
        let dir = hook_state(&p);
        for name in ["files-old", "stop-old"] {
            std::fs::write(dir.join(name), "a.txt\n").unwrap();
            aged(&dir.join(name), 8);
        }
        session_files::record(&p, "fresh", &["b.txt".to_string()]);
        assert!(!dir.join("files-old").exists());
        assert!(!dir.join("stop-old").exists());
        assert!(dir.join("files-fresh").exists());
    }

    #[test]
    fn prune_session_keeps_a_record_6_days_old() {
        let (_tmp, p, _repo, _task) = accepted();
        let dir = hook_state(&p);
        for name in ["files-recent", "stop-recent"] {
            std::fs::write(dir.join(name), "a.txt\n").unwrap();
            aged(&dir.join(name), 6);
        }
        session_files::record(&p, "fresh", &["b.txt".to_string()]);
        assert!(dir.join("files-recent").exists());
        assert!(dir.join("stop-recent").exists());
    }

    #[test]
    fn prune_session_keeps_a_file_that_is_not_a_session_record() {
        let (_tmp, p, _repo, _task) = accepted();
        let dir = hook_state(&p);
        for name in ["denied-old", "notes.txt"] {
            std::fs::write(dir.join(name), "x\n").unwrap();
            aged(&dir.join(name), 30);
        }
        session_files::record(&p, "fresh", &["b.txt".to_string()]);
        assert!(dir.join("denied-old").exists());
        assert!(dir.join("notes.txt").exists());
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
            "echo x >> ids.jsonl",
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
    fn a_corrupt_spec_record_fails_the_edit_check() {
        let tmp = crate::git::testrepo::TempDir::new();
        let home = tmp.0.join("home");
        let repo = crate::git::testrepo::repo_with_commit(&tmp.0, "r");
        let p = crate::store::init(&repo, &home, crate::store::Mode::Home).unwrap();
        std::fs::write(p.root.join(crate::record::RECORD_FILE), "{ not json").unwrap();
        let input = json!({"cwd": repo, "tool_name": "Edit", "tool_input": {"file_path": repo.join("src/lib.rs")}});
        // An error here makes the hook entry deny every writing tool.
        assert!(pre_tool_use_denial(&p, &input).is_err(), "a corrupt record must not let the edit through");
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
        let plan_file = edit(&p, &repo, c.path.join("tasks.md").to_str().unwrap()).unwrap();
        let reason = plan_file["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains("rvow put"), "{reason}");

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

    fn store_search(cmd: &str) -> Option<String> {
        let tmp = crate::git::testrepo::TempDir::new();
        let home = tmp.0.join("home");
        let repo = crate::git::testrepo::repo_with_commit(&tmp.0, "r");
        let p = crate::store::init(&repo, &home, crate::store::Mode::Home).unwrap();
        store_guard::bash_store_denial(&p, &repo, cmd).map(|d| d.reason)
    }

    #[test]
    fn store_search_own_path_is_the_searchers_argument() {
        assert_eq!(store_search("tmux send-keys / && tmux capture-pane -p | grep x"), None);
        assert_eq!(store_search("tmux send-keys / Enter; grep -E 'a|b' notes.txt"), None);
        assert!(store_search("grep -r secret /").is_some_and(|r| r.contains("covers the rvow store")));
        assert!(store_search("find / -name x").is_some_and(|r| r.contains("covers the rvow store")));
        assert!(store_search("echo go && find / -name x").is_some());
        assert!(store_search("xargs grep -r x /").is_some());
    }

    #[test]
    fn store_search_own_cwd_after_cd_is_searched() {
        assert!(store_search("cd / && ls").is_some_and(|r| r.contains("covers the rvow store")));
        assert!(store_search("cd / && grep -r secret").is_some());
        assert_eq!(store_search("ls"), None);
        assert_eq!(store_search("cd / && echo hi"), None);
    }

    #[test]
    fn store_search_own_plan_tree_word_is_denied_for_any_program() {
        let tmp = crate::git::testrepo::TempDir::new();
        let home = tmp.0.join("home");
        let repo = crate::git::testrepo::repo_with_commit(&tmp.0, "r");
        let p = crate::store::init(&repo, &home, crate::store::Mode::Home).unwrap();
        let cmd = format!("cat {}/active/CHG-abcdef-x/proposal.md", p.root.display());
        assert!(store_guard::bash_store_denial(&p, &repo, &cmd).is_some());
    }

    /// What the rules find for an edit, whatever the rule's mode does with it.
    fn edit(p: &Project, repo: &Path, file: &str) -> Option<Value> {
        let input = json!({
            "cwd": repo.to_str().unwrap(),
            "tool_name": "Edit",
            "tool_input": {"file_path": file},
        });
        pre_tool_use_denial(p, &input).unwrap().map(
            |d| json!({"hookSpecificOutput": {"permissionDecision": "deny", "permissionDecisionReason": format!("rvow: {}", d.reason)}}),
        )
    }

    fn call(p: &Project, repo: &Path, file: &str, session: &str) -> Option<Value> {
        let input = json!({
            "cwd": repo.to_str().unwrap(),
            "session_id": session,
            "tool_name": "Edit",
            "tool_input": {"file_path": file},
        });
        claude_pre_tool_use(p, &input).unwrap()
    }

    fn decision(out: &Option<Value>) -> Option<&str> {
        out.as_ref().and_then(|v| v["hookSpecificOutput"]["permissionDecision"].as_str())
    }

    fn project_with_a_task() -> (crate::git::testrepo::TempDir, Project, PathBuf) {
        let tmp = crate::git::testrepo::TempDir::new();
        let repo = crate::git::testrepo::repo_with_commit(&tmp.0, "r");
        std::fs::create_dir_all(repo.join("tests")).unwrap();
        std::fs::write(repo.join("tests/t.sh"), "exit 0").unwrap();
        let home = tmp.0.join("home");
        let p = crate::store::init(&repo, &home, crate::store::Mode::Home).unwrap();
        let c = change::new(&p, "T", "main", true).unwrap();
        std::fs::write(c.path.join("tasks.md"), "- TSK-new1 T\n  verify: false\n  files: src/**, tests/**\n").unwrap();
        let prepared = crate::vow::prepare(&p, &home, c, &repo, &mut std::io::sink()).unwrap();
        crate::vow::commit(&p, prepared).unwrap();
        (tmp, p, repo)
    }

    #[test]
    fn a_protected_test_and_a_stray_file_are_audited_not_denied() {
        let (_tmp, p, repo) = project_with_a_task();
        assert_eq!(call(&p, &repo, "tests/t.sh", "s1"), None);
        assert_eq!(call(&p, &repo, "docs/readme.md", "s1"), None);
        let c = stats::counts(&p, 1, crate::ledger::now_ms());
        assert_eq!(c.audits.get("protected-test"), Some(&1));
        assert_eq!(c.audits.get("outside-scope"), Some(&1));
        assert!(c.denials.is_empty(), "{c:?}");
    }

    #[test]
    fn the_fourth_denial_in_a_row_asks_the_user_and_an_allowed_call_ends_the_streak() {
        let tmp = crate::git::testrepo::TempDir::new();
        let repo = crate::git::testrepo::repo_with_commit(&tmp.0, "r");
        let home = tmp.0.join("home");
        let p = crate::store::init(&repo, &home, crate::store::Mode::Home).unwrap();
        let _c = change::new(&p, "T", "main", true).unwrap();
        // An edit before the plan is accepted is enforced and is not tampering, so the backstop applies.
        let denied = || call(&p, &repo, "src/lib.rs", "s2");
        assert_eq!(decision(&denied()), Some("deny"));
        assert_eq!(decision(&denied()), Some("deny"));
        assert_eq!(decision(&denied()), Some("deny"));
        assert_eq!(decision(&denied()), Some("ask"));
        assert_eq!(decision(&denied()), Some("deny"), "the streak starts again after a question");
        // A read is still allowed before accept, and it ends the streak.
        let allowed =
            json!({"cwd": repo.to_str().unwrap(), "session_id": "s2", "tool_name": "Read", "tool_input": {"file_path": "src/lib.rs"}});
        assert_eq!(claude_pre_tool_use(&p, &allowed).unwrap(), None);
        assert_eq!(decision(&denied()), Some("deny"));
        assert_eq!(stats::counts(&p, 1, crate::ledger::now_ms()).asks, 1);
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
        let plan_file = edit(&p, &repo, vowed.path.join("tasks.md").to_str().unwrap()).unwrap();
        assert!(plan_file["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().contains("rvow put"));

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
        assert!(reason(c.path.join("specs/cap.md")).unwrap().contains("rvow put"));
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
        assert!(!proof::prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap().proved);

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

    /// The session edits `out.txt`, the file of the task that `accepted` makes.
    fn edits_out(p: &Project, repo: &Path, session: &str) {
        let edit = json!({"file_path": repo.join("out.txt").to_str().unwrap(), "content": "ok"});
        assert_eq!(pre(p, repo, session, "Write", edit), None);
    }

    #[test]
    fn stop_ignores_untouched_tasks() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        let input = json!({"cwd": repo.to_str().unwrap(), "session_id": "s1"});
        assert_eq!(claude_stop(&p, &home, &input).unwrap(), None);

        std::fs::write(repo.join("other.txt"), "x").unwrap();
        assert_eq!(claude_stop(&p, &home, &input).unwrap(), None);

        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        edits_out(&p, &repo, "s1");
        let out = claude_stop(&p, &home, &input).unwrap().unwrap();
        assert!(out["reason"].as_str().unwrap().contains(&task), "{out}");
    }

    #[test]
    fn stop_lets_a_stale_task_through() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        let input = json!({"cwd": repo.to_str().unwrap(), "session_id": "s1"});
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        proof::prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        std::fs::write(repo.join("out.txt"), "ok changed").unwrap();
        edits_out(&p, &repo, "s1");
        assert_eq!(claude_stop(&p, &home, &input).unwrap(), None, "stale proofs are left to the push gate");
    }

    #[test]
    fn stop_blocks_on_open_tasks_and_drift_but_not_on_pending() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        let input = json!({"cwd": repo.to_str().unwrap(), "session_id": "s1"});
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        edits_out(&p, &repo, "s1");
        let out = claude_stop(&p, &home, &input).unwrap().unwrap();
        assert!(out["reason"].as_str().unwrap().contains(&task), "{out}");

        proof::prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        assert_eq!(claude_stop(&p, &home, &input).unwrap(), None);

        let dir = std::fs::read_dir(p.root.join("active")).unwrap().next().unwrap().unwrap().path();
        let tasks = dir.join("tasks.md");
        let text = std::fs::read_to_string(&tasks).unwrap();
        // A title change is free drift: it asks for a new proof and not for the user's confirm.
        std::fs::write(&tasks, text.replace("Write ok", "Write it")).unwrap();
        assert_eq!(claude_stop(&p, &home, &input).unwrap(), None, "stale proofs are left to the push gate");
        std::fs::write(&tasks, &text).unwrap();
        let proposal = dir.join("proposal.md");
        let text = std::fs::read_to_string(&proposal).unwrap();
        std::fs::write(&proposal, format!("{text}\nOne more thing.\n")).unwrap();
        assert_eq!(claude_stop(&p, &home, &input).unwrap(), None, "a plan edit names no file of the session");

        amend::amend(&p, &home, &repo, None, "rename", None).unwrap();
        assert_eq!(claude_stop(&p, &home, &input).unwrap(), None);

        assert_eq!(
            status_line(&p, &home, &repo).unwrap().unwrap(),
            format!("rvow {}: 1/1 proved, 1 drift, 1 pending", dir.file_name().unwrap().to_str().unwrap().get(..10).unwrap())
        );
        let start = claude_session_start(Some(&p), &home, &input, &[]).unwrap().unwrap();
        assert!(start["hookSpecificOutput"]["additionalContext"].as_str().unwrap().contains("tasks:"));
    }

    fn pre(p: &Project, repo: &Path, session: &str, tool: &str, input: Value) -> Option<Value> {
        let call = json!({"session_id": session, "cwd": repo.to_str().unwrap(), "tool_name": tool, "tool_input": input});
        retried(p, &call)
    }

    fn pre_agent(p: &Project, repo: &Path, session: &str, agent: &str, tool: &str, input: Value) -> Option<Value> {
        let call = json!({"session_id": session, "agent_id": agent, "cwd": repo.to_str().unwrap(), "tool_name": tool, "tool_input": input});
        retried(p, &call)
    }

    /// The pre-tool-use answer of `call`; the first source edit is denied once for the baseline, and an agent retries it.
    fn retried(p: &Project, call: &Value) -> Option<Value> {
        let out = claude_pre_tool_use(p, call).unwrap();
        if out.as_ref().is_some_and(|v| v.to_string().contains("has no baseline yet")) {
            return claude_pre_tool_use(p, call).unwrap();
        }
        out
    }

    fn stop_of(p: &Project, home: &Path, repo: &Path, session: &str) -> Option<String> {
        let input = json!({"cwd": repo.to_str().unwrap(), "session_id": session});
        claude_stop(p, home, &input).unwrap().map(|v| v["reason"].as_str().unwrap().to_string())
    }

    /// The no-proof hold of `session`, without the once-per-session filter of the hook.
    fn hold(p: &Project, home: &Path, repo: &Path, session: &str) -> Option<String> {
        stop_reason(p, home, repo, Some(session)).unwrap().map(|h| h.text)
    }

    #[test]
    fn stop_lease_a_task_leased_to_another_worker_does_not_hold() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        std::fs::write(repo.join("out.txt"), "no").unwrap();
        edits_out(&p, &repo, "s1");
        assert!(hold(&p, &home, &repo, "s1").is_some_and(|r| r.contains(&task)));
        lease::issue(&p, &home, &repo, &task).unwrap();
        assert_eq!(hold(&p, &home, &repo, "s1"), None, "an unbound live lease");
        assert!(lease::bind(&p, &task, "worker").unwrap());
        assert_eq!(hold(&p, &home, &repo, "s1"), None, "a lease bound to another worker");
    }

    #[test]
    fn stop_lease_an_ended_or_old_lease_holds_again() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        std::fs::write(repo.join("out.txt"), "no").unwrap();
        edits_out(&p, &repo, "s1");
        lease::issue(&p, &home, &repo, &task).unwrap();
        assert!(lease::bind(&p, &task, "worker").unwrap());
        lease::end(&p, "worker").unwrap();
        assert!(hold(&p, &home, &repo, "s1").is_some_and(|r| r.contains(&task)), "an ended lease");

        let old = json!({"event": "issue", "task": task, "change": "CHG-x", "files": ["out.txt"], "time": 1});
        std::fs::write(p.root.join(LEASE_FILE), format!("{old}\n")).unwrap();
        assert!(hold(&p, &home, &repo, "s2").is_none(), "s2 changed nothing");
        assert!(hold(&p, &home, &repo, "s1").is_some_and(|r| r.contains(&task)), "a lease older than 4 hours");
    }

    #[test]
    fn stop_lease_the_session_that_holds_the_lease_is_held() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        lease::issue(&p, &home, &repo, &task).unwrap();
        assert!(lease::bind(&p, &task, "w1").unwrap());
        std::fs::write(repo.join("out.txt"), "no").unwrap();
        edits_out(&p, &repo, "w1");
        assert!(hold(&p, &home, &repo, "w1").is_some_and(|r| r.contains(&task)));
    }

    #[test]
    fn stop_agent_subagent_edits_do_not_hold_the_main_session() {
        let (tmp, p, repo, _task) = accepted();
        let home = tmp.0.join("home");
        let edit = json!({"file_path": repo.join("out.txt").to_str().unwrap(), "content": "ok"});
        assert_eq!(pre_agent(&p, &repo, "s1", "a1", "Write", edit), None);
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        assert_eq!(stop_of(&p, &home, &repo, "s1"), None);
    }

    #[test]
    fn stop_agent_main_session_edits_hold_the_main_session() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        let edit = json!({"file_path": repo.join("out.txt").to_str().unwrap(), "content": "no"});
        assert_eq!(pre(&p, &repo, "s1", "Write", edit), None);
        std::fs::write(repo.join("out.txt"), "no").unwrap();
        let reason = stop_of(&p, &home, &repo, "s1").unwrap();
        assert!(reason.contains(&task), "{reason}");
    }

    #[test]
    fn record_only_formatter_run_holds_nothing() {
        let (tmp, p, repo, _task) = accepted();
        let home = tmp.0.join("home");
        std::fs::write(repo.join("out.txt"), "a").unwrap();
        assert_eq!(pre(&p, &repo, "s1", "Bash", json!({"command": "cargo fmt --all"})), None);
        assert_eq!(stop_of(&p, &home, &repo, "s1"), None, "a formatter run alone holds nothing");
    }

    #[test]
    fn record_only_script_text_names_files_not_code_words() {
        let (_tmp, p, repo, _task) = accepted();
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/a.rs"), "fn main() {}").unwrap();
        let cmd = "python3 - <<'EOF'\nstack.pop()\nopen(\"src/a.rs\", \"w\")\nEOF";
        assert_eq!(pre(&p, &repo, "s1", "Bash", json!({"command": cmd})), None);
        let touched = session_files::load(&p, "s1");
        assert!(touched.contains("src/a.rs"), "{touched:?}");
        assert!(!touched.iter().any(|f| f.contains("stack.pop")), "{touched:?}");
    }

    #[test]
    fn record_only_cd_touch_records_the_file_not_the_folder() {
        let (_tmp, p, repo, _task) = accepted();
        std::fs::create_dir_all(repo.join("crates/core")).unwrap();
        assert_eq!(pre(&p, &repo, "s1", "Bash", json!({"command": "cd crates/core && touch new.rs"})), None);
        let touched = session_files::load(&p, "s1");
        assert!(touched.contains("crates/core/new.rs"), "{touched:?}");
        assert!(!touched.contains("crates/core"), "{touched:?}");
    }

    #[test]
    fn record_text_of_rvow_put_is_data() {
        let (tmp, p, repo, _task) = accepted();
        let home = tmp.0.join("home");
        std::fs::write(repo.join("out.txt"), "a").unwrap();
        let cmd = "rvow put CHG-abc123 tasks <<'EOF'\n- TSK-new1 x\n  files: out.txt\nEOF";
        assert_eq!(pre(&p, &repo, "s1", "Bash", json!({"command": cmd})), None);
        let touched = session_files::load(&p, "s1");
        assert!(!touched.contains("out.txt"), "{touched:?}");
        assert_eq!(stop_of(&p, &home, &repo, "s1"), None, "naming a task file in plan text is no edit");
    }

    #[test]
    fn record_text_of_a_note_is_data() {
        let (_tmp, p, repo, _task) = accepted();
        std::fs::write(repo.join("out.txt"), "a").unwrap();
        assert_eq!(pre(&p, &repo, "s1", "Bash", json!({"command": "rvow note \"pi edits out.txt next\""})), None);
        assert!(session_files::load(&p, "s1").iter().all(|f| f.starts_with(':')), "{:?}", session_files::load(&p, "s1"));
    }

    #[test]
    fn record_text_of_a_python_script_counts() {
        let (_tmp, p, repo, _task) = accepted();
        std::fs::write(repo.join("out.txt"), "a").unwrap();
        let cmd = "python3 - <<'EOF'\nopen('out.txt', 'w').write('b')\nEOF";
        assert_eq!(pre(&p, &repo, "s1", "Bash", json!({"command": cmd})), None);
        assert!(session_files::load(&p, "s1").contains("out.txt"));
    }

    #[test]
    fn record_text_of_cat_into_a_file_counts_only_the_target() {
        let (_tmp, p, repo, _task) = accepted();
        std::fs::write(repo.join("out.txt"), "a").unwrap();
        std::fs::write(repo.join("other.txt"), "a").unwrap();
        let cmd = "cat > other.txt <<'EOF'\nsee out.txt\nEOF";
        assert_eq!(pre(&p, &repo, "s1", "Bash", json!({"command": cmd})), None);
        let touched = session_files::load(&p, "s1");
        assert!(touched.contains("other.txt") && !touched.contains("out.txt"), "{touched:?}");
    }

    #[test]
    fn record_only_each_path_once() {
        let (_tmp, p, repo, _task) = accepted();
        std::fs::write(repo.join("out.txt"), "a").unwrap();
        assert_eq!(pre(&p, &repo, "s1", "Bash", json!({"command": "echo x > out.txt && echo y > out.txt"})), None);
        let file = p.root.join("hook-state").join("files-s1");
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        assert_eq!(text.lines().filter(|l| *l == "out.txt").count(), 1, "{text:?}");
    }

    #[test]
    fn stop_owner_another_session_built_the_task() {
        let (tmp, p, repo, _task) = accepted();
        let home = tmp.0.join("home");
        let edit = json!({"file_path": repo.join("out.txt").to_str().unwrap(), "old_string": "a", "new_string": "b"});
        assert_eq!(pre(&p, &repo, "builder", "Edit", edit), None);
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        assert_eq!(stop_of(&p, &home, &repo, "bystander"), None);
        assert_eq!(stop_of(&p, &home, &repo, ""), None);
    }

    #[test]
    fn stop_owner_the_session_edited_the_file_of_the_task() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        let edit = json!({"file_path": repo.join("out.txt").to_str().unwrap(), "content": "no"});
        assert_eq!(pre(&p, &repo, "s1", "Write", edit), None);
        std::fs::write(repo.join("out.txt"), "no").unwrap();
        let reason = stop_of(&p, &home, &repo, "s1").unwrap();
        assert!(reason.contains(&task), "{reason}");
    }

    #[test]
    fn stop_owner_a_shell_command_changed_the_file() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        std::fs::write(repo.join("out.txt"), "a").unwrap();
        assert_eq!(pre(&p, &repo, "s1", "Bash", json!({"command": "sed -i s/a/b/ out.txt"})), None);
        let reason = stop_of(&p, &home, &repo, "s1").unwrap();
        assert!(reason.contains(&task), "{reason}");
    }

    #[test]
    fn stop_owner_scripts_globs_cd_and_formatters_are_recorded() {
        let commands = [
            "python3 - <<'EOF'\nopen('out.txt', 'w').write('b')\nEOF",
            "python3 -c \"open('out.txt', 'w').write('b')\"",
            "sed -i s/a/b/ *.txt",
            "mkdir -p sub && cd sub && sed -i s/a/b/ ../out.txt",
            "tool --out=out.txt",
        ];
        for (i, command) in commands.iter().enumerate() {
            let (tmp, p, repo, task) = accepted();
            let home = tmp.0.join("home");
            std::fs::write(repo.join("out.txt"), "a").unwrap();
            std::fs::create_dir_all(repo.join("sub")).unwrap();
            let session = format!("s{i}");
            assert_eq!(pre(&p, &repo, &session, "Bash", json!({"command": command})), None, "{command}");
            let reason = stop_of(&p, &home, &repo, &session).unwrap_or_else(|| panic!("{command} is not recorded"));
            assert!(reason.contains(&task), "{command}: {reason}");
        }
    }

    #[test]
    fn stop_owner_reading_commands_hold_nothing() {
        for (i, command) in ["git diff -- out.txt", "git log -p out.txt", "sed -n 1,5p out.txt", "cargo fmt --check"].iter().enumerate() {
            let (tmp, p, repo, _task) = accepted();
            let home = tmp.0.join("home");
            std::fs::write(repo.join("out.txt"), "a").unwrap();
            let session = format!("r{i}");
            assert_eq!(pre(&p, &repo, &session, "Bash", json!({"command": command})), None, "{command}");
            assert_eq!(stop_of(&p, &home, &repo, &session), None, "{command} only reads");
        }
    }

    #[test]
    fn stop_owner_drift_names_a_path_as_a_whole_word() {
        assert!(names_path("protected file tests/t.sh changed", "tests/t.sh"));
        assert!(names_path("test crates/a.rs#cfg(test) changed", "crates/a.rs"));
        assert!(!names_path("TSK-x covers changed", "a"));
        assert!(!names_path("protected file tests/t.sh changed", "t.sh"));
    }

    #[test]
    fn stop_owner_rvow_put_owns_the_plan() {
        let cwd = std::env::temp_dir();
        assert!(bash_changes("rvow put CHG-abc123 tasks <<'EOF'\n- TSK-new1 x\nEOF", &cwd).plan);
        assert!(!bash_changes("rvow show CHG-abc123", &cwd).plan);
    }

    #[test]
    fn stop_owner_a_read_only_command_holds_nothing() {
        let (tmp, p, repo, _task) = accepted();
        let home = tmp.0.join("home");
        std::fs::write(repo.join("out.txt"), "a").unwrap();
        assert_eq!(pre(&p, &repo, "s1", "Bash", json!({"command": "grep a out.txt"})), None);
        assert_eq!(stop_of(&p, &home, &repo, "s1"), None);
    }

    #[test]
    fn stop_owner_a_pending_amendment_releases_the_change() {
        let (tmp, p, repo, _task) = accepted();
        let home = tmp.0.join("home");
        let edit = json!({"file_path": repo.join("out.txt").to_str().unwrap(), "content": "no"});
        assert_eq!(pre(&p, &repo, "s1", "Write", edit.clone()), None);
        assert_eq!(pre(&p, &repo, "s2", "Write", edit), None);
        std::fs::write(repo.join("out.txt"), "no").unwrap();
        assert!(stop_of(&p, &home, &repo, "s1").is_some());

        let dir = std::fs::read_dir(p.root.join("active")).unwrap().next().unwrap().unwrap().path();
        std::fs::write(dir.join("design.md"), "A new design note.\n").unwrap();
        amend::amend(&p, &home, &repo, None, "design", None).unwrap();
        assert_eq!(stop_of(&p, &home, &repo, "s2"), None);
    }

    fn protected_fixture() -> (crate::git::testrepo::TempDir, Project, PathBuf) {
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
        crate::vow::commit(&p, prepared).unwrap();
        (tmp, p, repo)
    }

    #[test]
    fn stop_owner_drift_is_held_for_the_session_that_made_it() {
        let (tmp, p, repo) = protected_fixture();
        let home = tmp.0.join("home");
        std::fs::write(repo.join("tests/t.sh"), "exit 0").unwrap();
        assert_eq!(pre(&p, &repo, "maker", "Bash", json!({"command": "echo 'exit 0' > tests/t.sh"})), None);
        let reason = stop_of(&p, &home, &repo, "maker").unwrap();
        assert!(reason.contains("amend"), "{reason}");
    }

    #[test]
    fn stop_owner_drift_made_by_another_session_passes() {
        let (tmp, p, repo) = protected_fixture();
        let home = tmp.0.join("home");
        std::fs::write(repo.join("tests/t.sh"), "exit 0").unwrap();
        assert_eq!(pre(&p, &repo, "maker", "Bash", json!({"command": "echo 'exit 0' > tests/t.sh"})), None);
        assert_eq!(stop_of(&p, &home, &repo, "bystander"), None);
    }

    /// A proved task whose file changed since, edited by session `s1`, so the proof is stale.
    fn stale_after_edit() -> (crate::git::testrepo::TempDir, Project, PathBuf, String) {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        proof::prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        std::fs::write(repo.join("out.txt"), "ok changed").unwrap();
        edits_out(&p, &repo, "s1");
        (tmp, p, repo, task)
    }

    #[test]
    fn stop_stale_a_session_that_edited_a_proved_task_may_stop() {
        let (tmp, p, repo, _task) = stale_after_edit();
        let home = tmp.0.join("home");
        assert_eq!(stop_of(&p, &home, &repo, "s1"), None);
    }

    #[test]
    fn stop_stale_the_gate_still_refuses_the_stale_proof() {
        let (tmp, p, repo, task) = stale_after_edit();
        let home = tmp.0.join("home");
        assert_eq!(stop_of(&p, &home, &repo, "s1"), None);
        let opts = crate::gate::Options { strict: true, ..Default::default() };
        let report = crate::gate::gate_uncached(&p, &home, &repo, opts, &mut std::io::sink()).unwrap();
        assert!(!report.passed(), "{report:?}");
        assert!(report.failures.iter().any(|f| f.contains(&task) && f.contains("stale")), "{report:?}");
    }
}
