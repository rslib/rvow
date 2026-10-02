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

/// Why a Bash command must not run, or `None`.
pub fn bash_denial(cmd: &str) -> Option<String> {
    let w = words(cmd);
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
fn real_path(path: &Path, cwd: &Path) -> PathBuf {
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
fn key(p: &Path) -> String {
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

/// Why an edit of `file` must not happen, or `None`.
fn edit_denial(project: &Project, top: &Path, cwd: &Path, file: &str) -> Result<Option<String>> {
    let target = key(&real_path(Path::new(file), cwd));
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
            || target.starts_with(&format!("{root}/objects/")))
    {
        return Ok(Some(format!(
            "{name} is written only by rvow; use rvow commands"
        )));
    }
    if target.starts_with(&format!("{top_key}/.git/")) {
        return Ok(Some(
            ".git/ is not edited by hand; the hooks there run the gate".into(),
        ));
    }
    let Some(rel) = target.strip_prefix(&format!("{top_key}/")) else {
        return Ok(None);
    };
    if ENFORCEMENT.iter().any(|e| key(Path::new(e)) == rel) {
        return Ok(Some(format!(
            "{rel} configures the hooks that enforce the plan; changing it is for the user"
        )));
    }
    if protected(project, top)?.contains(rel) {
        return Ok(Some(format!(
            "{rel} is a protected test or test setup file; to change it, run rvow amend --unlock {rel} --reason \"...\" and ask the user to confirm"
        )));
    }
    Ok(None)
}

/// The protected paths of every active change, from their baselines.
fn protected(project: &Project, top: &Path) -> Result<BTreeSet<String>> {
    let (living, _) = change::load_living(project)?;
    let mut out = BTreeSet::new();
    let branch = git::branch(top)?.unwrap_or_default();
    for c in proof::active(project, &living)?
        .iter()
        .filter(|c| c.branch() == Some(branch.as_str()))
    {
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
        args.get("command")
            .and_then(Value::as_str)
            .and_then(bash_denial)
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
    Ok(proof::next(&queue, &now)?.map(|(_, t, s)| {
        let id = t.id.to_string();
        match s {
            State::Stale(_) | State::Flaky => {
                format!("{id} is {}: run rvow prove --stale", s.name())
            }
            _ => format!(
                "{id} has no proof: run rvow prove {id}, or rvow block {id} --reason \"...\" if it cannot pass as written"
            ),
        }
    }))
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
        ];
        for cmd in denied {
            assert!(bash_denial(cmd).is_some(), "{cmd}");
        }
        let allowed = [
            "rvow prove x7k",
            "rvow status",
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
            "- TSK-new1 T\n  verify: false\n  files: src/**\n",
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
    fn stop_blocks_on_open_tasks_and_drift_but_not_on_pending() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        let input = json!({"cwd": repo.to_str().unwrap()});
        let out = claude_stop(&p, &home, &input).unwrap().unwrap();
        assert!(out["reason"].as_str().unwrap().contains(&task), "{out}");

        std::fs::write(repo.join("out.txt"), "ok").unwrap();
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

        amend::amend(&p, &home, &repo, None, "rename").unwrap();
        assert_eq!(claude_stop(&p, &home, &input).unwrap(), None);

        let start = claude_session_start(&p, &home, &input).unwrap().unwrap();
        assert!(
            start["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .contains("tasks:")
        );
    }
}
