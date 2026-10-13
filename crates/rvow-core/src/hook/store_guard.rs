//! The plan files of the rvow store are read and written only through rvow:
//! `rvow show` to read, `rvow put` to write, never a direct path.

use std::path::Path;

use super::rule::denial;
use super::shell::{glob_paths, path_words, reads_only};
use super::{Denial, Rule, key, programs, real_path};
use crate::store::{Mode, Project};

/// The store trees that hold plans: the stage folders of a change and the living specs.
const PLAN_TREES: [&str; 4] = ["backlog", "active", "archive", "specs"];

/// The part of a change that a path names, for the reason that names the command.
fn part_of(rel: &str) -> Option<String> {
    let parts: Vec<&str> = rel.split('/').collect();
    match parts.as_slice() {
        [_, _, "proposal.md"] => Some("proposal".to_string()),
        [_, _, "tasks.md"] => Some("tasks".to_string()),
        [_, _, "specs", cap] => cap.strip_suffix(".md").map(|cap| format!("spec:{cap}")),
        _ => None,
    }
}

/// The change id that a path names, when its folder is a change folder.
fn change_of(rel: &str) -> Option<String> {
    rel.split('/').find_map(|c| c.get(..10).filter(|p| p[..4].eq_ignore_ascii_case("chg-")).map(|p| format!("CHG-{}", &p[4..])))
}

/// The path relative to the store root when it is inside one of the plan trees.
fn plan_rel(project: &Project, cwd: &Path, file: &str) -> Option<String> {
    if project.meta.mode != Mode::Home {
        return None;
    }
    let target = key(&real_path(Path::new(file), cwd));
    let root = key(&real_path(&project.root, cwd));
    let rel = target.strip_prefix(&format!("{root}/"))?;
    rel.split('/').next().filter(|c| PLAN_TREES.contains(c)).map(|_| rel.to_string())
}

/// Why a tool call must not read or write a plan file of the store directly.
fn reason(rel: &str, write: bool) -> String {
    let id = change_of(rel).unwrap_or_else(|| "<change>".to_string());
    match (part_of(rel), write) {
        (Some(part), false) => {
            format!("{rel} is part of the rvow store; read it with `rvow show {id} {part}` or list the parts with `rvow show {id}`")
        }
        (Some(part), true) => format!(
            "{rel} is part of the rvow store; replace it with `rvow put {id} {part}` from stdin, or `rvow put <task or requirement id>` for one block"
        ),
        (None, false) => {
            format!(
                "{rel} is part of the rvow store; read it through rvow: run `rvow ls` to list the changes and `rvow show <id>` to read one"
            )
        }
        (None, true) => {
            format!("{rel} is part of the rvow store; change plans through rvow: run `rvow put <change> <part>` from stdin")
        }
    }
}

/// Why a tool call must not touch a plan file of the store, or `None` outside
/// the plan trees. The operational files keep their own rules.
pub(crate) fn store_denial(project: &Project, cwd: &Path, file: &str, write: bool) -> Option<Denial> {
    let rel = plan_rel(project, cwd, file)?;
    denial(Rule::Store, reason(&rel, write))
}

/// The programs that search the files under a path they are given.
const SEARCHERS: &[&str] = &["find", "grep", "egrep", "fgrep", "rg", "ag", "ack", "fd", "locate", "ls", "tree", "du"];

/// Why a Bash command must not read, write or search the plan files of the
/// store: it names a path inside them, or a search program covers them.
pub(crate) fn bash_store_denial(project: &Project, cwd: &Path, cmd: &str) -> Option<Denial> {
    if project.meta.mode != Mode::Home {
        return None;
    }
    let root = key(&real_path(&project.root, cwd));
    let write = !reads_only(cmd);
    let searches = programs(cmd).iter().any(|p| SEARCHERS.contains(&p.rsplit('/').next().unwrap_or(p).to_lowercase().as_str()));
    for word in path_words(cmd) {
        let path = key(&real_path(Path::new(&word), cwd));
        if let Some(rel) =
            path.strip_prefix(&format!("{root}/")).filter(|rel| rel.split('/').next().is_some_and(|c| PLAN_TREES.contains(&c)))
        {
            return denial(Rule::Store, reason(rel, write));
        }
        if searches && (root == path || root.starts_with(&path)) {
            return denial(
                Rule::Store,
                format!(
                    "a search from {word} covers the rvow store at {root}; run `rvow ls` to list the changes and `rvow show <id>` to read one"
                ),
            );
        }
    }
    // A glob hides the name from the word rules, so its expansions are checked too.
    for path in glob_paths(cmd, cwd) {
        let path = key(&path);
        if let Some(rel) =
            path.strip_prefix(&format!("{root}/")).filter(|rel| rel.split('/').next().is_some_and(|c| PLAN_TREES.contains(&c)))
        {
            return denial(Rule::Store, reason(rel, write));
        }
    }
    None
}
