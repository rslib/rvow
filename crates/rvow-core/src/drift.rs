//! Differences between a change's files and its baseline snapshot, each in one
//! of three classes: free, confirm or weaken.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::change::{DELTAS, DESIGN, PROPOSAL, TASKS};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::proof::Active;
use crate::spec::{DeltaOp, parse_deltas};
use crate::store::Project;
use crate::tasks::{Task, parse_tasks};
use crate::vow::{self, protected_files, requirement_hash, sha256_hex};

/// Protected files up to this size, before and after together, show their
/// text in an amendment so the user can review the change.
const SHOW_LIMIT: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Class {
    Weaken,
    Confirm,
    Free,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::Weaken => "weaken",
            Class::Confirm => "confirm",
            Class::Free => "free",
        }
    }

    /// Whether the gate refuses until the user confirms it.
    pub fn blocks(self) -> bool {
        self != Class::Free
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Item {
    pub class: Class,
    pub what: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

fn item(class: Class, what: impl Into<String>) -> Item {
    Item {
        class,
        what: what.into(),
        before: None,
        after: None,
    }
}

fn changed(class: Class, what: impl Into<String>, before: &str, after: &str) -> Item {
    Item {
        class,
        what: what.into(),
        before: Some(before.to_string()),
        after: Some(after.to_string()),
    }
}

fn object(project: &Project, hash: &str) -> Result<String> {
    let path = project.root.join("objects").join(hash);
    std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))
}

fn hashes(snapshot: &Map<String, Value>, key: &str) -> BTreeMap<String, String> {
    snapshot
        .get(key)
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `glob` lies inside one of `scope` by its text: the same glob, a path
/// under a `dir/**` glob, or anything under `**`. Reading text and not files
/// keeps `newdir/**` from looking free while it matches nothing yet.
pub fn inside(glob: &str, scope: &[String]) -> bool {
    scope.iter().any(|s| {
        s == glob
            || s == "**"
            || s.strip_suffix("/**")
                .is_some_and(|dir| glob.starts_with(&format!("{dir}/")))
    })
}

fn task_items(before: &[Task], after: &[Task], out: &mut Vec<Item>) {
    let scope: Vec<String> = before.iter().flat_map(|t| t.files.clone()).collect();
    for old in before {
        let Some(new) = after.iter().find(|t| t.id == old.id) else {
            out.push(item(Class::Confirm, format!("{} removed", old.id)));
            continue;
        };
        let id = &old.id;
        if old.verify != new.verify {
            out.push(changed(
                Class::Weaken,
                format!("{id} verify changed"),
                old.verify.as_deref().unwrap_or(""),
                new.verify.as_deref().unwrap_or(""),
            ));
        }
        if old.title != new.title {
            out.push(changed(
                Class::Confirm,
                format!("{id} title changed"),
                &old.title,
                &new.title,
            ));
        }
        if old.covers != new.covers {
            let list = |t: &Task| {
                t.covers
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            out.push(changed(
                Class::Confirm,
                format!("{id} covers changed"),
                &list(old),
                &list(new),
            ));
        }
        if old.files != new.files {
            let wider = new.files.iter().any(|g| !inside(g, &old.files));
            let class = if wider { Class::Confirm } else { Class::Free };
            let what = if wider {
                "scope made wider"
            } else {
                "scope made narrower"
            };
            out.push(changed(
                class,
                format!("{id} {what}"),
                &old.files.join(", "),
                &new.files.join(", "),
            ));
        }
        if old.baseline_pass != new.baseline_pass || old.timeout != new.timeout {
            out.push(item(
                Class::Free,
                format!("{id} baseline or timeout changed"),
            ));
        }
    }
    for new in after
        .iter()
        .filter(|t| !before.iter().any(|o| o.id == t.id))
    {
        let class = if new.files.iter().all(|g| inside(g, &scope)) {
            Class::Free
        } else {
            Class::Confirm
        };
        out.push(item(
            class,
            format!("{} added with files {}", new.id, new.files.join(", ")),
        ));
    }
}

fn delta_map(files: &BTreeMap<String, String>) -> BTreeMap<String, (DeltaOp, String)> {
    let mut map = BTreeMap::new();
    for text in files.values() {
        for d in parse_deltas(text).0.deltas {
            let hash = if d.op.has_body() {
                requirement_hash(&d.req)
            } else {
                String::new()
            };
            map.insert(d.req.id.to_string(), (d.op, hash));
        }
    }
    map
}

fn delta_items(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
    out: &mut Vec<Item>,
) {
    let old = delta_map(before);
    let new = delta_map(after);
    for (id, (op, hash)) in &old {
        match new.get(id) {
            None => out.push(item(Class::Confirm, format!("delta for {id} removed"))),
            Some((nop, _)) if nop != op => out.push(item(
                Class::Confirm,
                format!("delta for {id} changed from {op:?} to {nop:?}"),
            )),
            Some((_, nhash)) if nhash != hash => {
                out.push(item(Class::Confirm, format!("requirement {id} changed")))
            }
            Some(_) => {}
        }
    }
    for (id, (op, _)) in new.iter().filter(|(id, _)| !old.contains_key(*id)) {
        let class = match op {
            DeltaOp::Remove | DeltaOp::Deprecate => Class::Weaken,
            _ => Class::Confirm,
        };
        out.push(item(class, format!("new delta {op:?} {id}")));
    }
}

/// Every difference between the change's files and its baseline, weaken first.
pub fn drift(project: &Project, top: &Path, config: &Config, change: &Active) -> Result<Vec<Item>> {
    let Some(base) = change.baseline() else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    let loaded = &change.loaded;

    let vowed_branch = base.get("branch").and_then(Value::as_str).unwrap_or("");
    if vowed_branch != loaded.proposal.branch {
        out.push(changed(
            Class::Confirm,
            "branch changed",
            vowed_branch,
            &loaded.proposal.branch,
        ));
    }

    let files = hashes(base, "files");
    let now: BTreeMap<String, String> = loaded
        .texts
        .iter()
        .map(|(rel, text)| (rel.clone(), sha256_hex(text.as_bytes())))
        .collect();
    if files.get(PROPOSAL) != now.get(PROPOSAL) {
        out.push(item(Class::Confirm, "proposal changed"));
    }
    if files.get(DESIGN) != now.get(DESIGN) {
        out.push(item(Class::Free, "design changed"));
    }

    let old_tasks = match files.get(TASKS) {
        Some(h) => parse_tasks(&object(project, h)?).0,
        None => Vec::new(),
    };
    task_items(&old_tasks, &loaded.tasks, &mut out);

    let prefix = format!("{DELTAS}/");
    let mut old_deltas = BTreeMap::new();
    for (rel, h) in files.iter().filter(|(rel, _)| rel.starts_with(&prefix)) {
        old_deltas.insert(rel.clone(), object(project, h)?);
    }
    let new_deltas: BTreeMap<String, String> = loaded
        .texts
        .iter()
        .filter(|(rel, _)| rel.starts_with(&prefix))
        .cloned()
        .collect();
    delta_items(&old_deltas, &new_deltas, &mut out);

    let config_now = sha256_hex(config.canonical().as_bytes());
    let config_then = base.get("config").and_then(Value::as_str).unwrap_or("");
    if config_now != config_then {
        let before = object(project, config_then).unwrap_or_default();
        out.push(changed(
            Class::Weaken,
            "config changed",
            &before,
            &config.canonical(),
        ));
    }

    let protected_then: BTreeMap<String, String> = base
        .get("protected")
        .and_then(Value::as_str)
        .map(|h| object(project, h))
        .transpose()?
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let protected_now = protected_files(top, config)?;
    for (rel, hash) in &protected_then {
        match protected_now.get(rel) {
            None => out.push(item(Class::Weaken, format!("protected file {rel} removed"))),
            Some(path) => {
                let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
                if &sha256_hex(&bytes) != hash {
                    let what = format!("protected file {rel} changed");
                    let old = object(project, hash).ok();
                    let new = String::from_utf8(bytes).ok();
                    out.push(match (old, new) {
                        (Some(o), Some(n)) if o.len() + n.len() <= SHOW_LIMIT => {
                            changed(Class::Weaken, what, &o, &n)
                        }
                        _ => item(Class::Weaken, what),
                    });
                }
            }
        }
    }
    out.extend(crate::guard::drift(project, top, change));
    out.sort_by_key(|i| i.class);
    Ok(out)
}

/// The snapshot of the change as it is now, for an amendment.
pub fn snapshot_now(
    project: &Project,
    top: &Path,
    config: &Config,
    change: &Active,
) -> Result<Map<String, Value>> {
    vow::snapshot(
        project,
        top,
        config,
        &change.loaded,
        &change.loaded.proposal.branch,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change;
    use crate::config;
    use crate::git;
    use crate::proof::{self, tests::accepted};

    fn drift_of(p: &Project, repo: &Path) -> Vec<(Class, String)> {
        let home = p.root.parent().unwrap().parent().unwrap();
        let (living, _) = change::load_living(p).unwrap();
        let changes = proof::active(p, &living).unwrap();
        let top = git::toplevel(repo).unwrap();
        let cfg = config::load(p, home).unwrap();
        drift(p, &top, &cfg, &changes[0])
            .unwrap()
            .into_iter()
            .map(|i| (i.class, i.what))
            .collect()
    }

    fn active_dir(p: &Project) -> std::path::PathBuf {
        std::fs::read_dir(p.root.join("active"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
    }

    fn edit(path: &Path, from: &str, to: &str) {
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains(from), "{from} in {text}");
        std::fs::write(path, text.replace(from, to)).unwrap();
    }

    #[test]
    fn globs_inside_by_text() {
        let scope = vec!["src/**".to_string(), "out.txt".to_string()];
        assert!(inside("src/a.rs", &scope));
        assert!(inside("src/sub/**", &scope));
        assert!(inside("out.txt", &scope));
        assert!(!inside("srcx/a.rs", &scope));
        assert!(!inside("newdir/**", &scope));
        assert!(!inside("**", &scope));
        assert!(inside("anything", &["**".to_string()]));
    }

    #[test]
    fn no_drift_right_after_accept() {
        let (_tmp, p, repo, _) = accepted();
        assert_eq!(drift_of(&p, &repo), []);
    }

    #[test]
    fn classifies_task_and_proposal_edits() {
        let (_tmp, p, repo, task) = accepted();
        let dir = active_dir(&p);
        let tasks = dir.join(TASKS);
        edit(&tasks, "grep -q ok out.txt", "true");
        edit(&tasks, "Write ok", "Write it");
        edit(&dir.join(PROPOSAL), "## Why", "## Why\nbecause");
        let mut text = std::fs::read_to_string(&tasks).unwrap();
        text.push_str("- TSK-new1 Inside\n  verify: true\n  files: out.txt\n");
        text.push_str("- TSK-new2 Outside\n  verify: true\n  files: **\n");
        std::fs::write(&tasks, text).unwrap();
        let got = drift_of(&p, &repo);
        assert_eq!(
            got,
            [
                (Class::Weaken, format!("{task} verify changed")),
                (Class::Confirm, "proposal changed".to_string()),
                (Class::Confirm, format!("{task} title changed")),
                (Class::Confirm, "TSK-new2 added with files **".to_string()),
                (Class::Free, "TSK-new1 added with files out.txt".to_string()),
            ]
        );
    }

    #[test]
    fn classifies_deltas_config_branch_and_protected_files() {
        let (_tmp, p, repo, _) = accepted();
        let dir = active_dir(&p);
        edit(
            &dir.join("specs/out.md"),
            "rvow SHALL write ok.",
            "rvow SHALL write ok twice.",
        );
        let mut text = std::fs::read_to_string(dir.join("specs/out.md")).unwrap();
        text.push_str("## REMOVE REQ-zzzzzz\n");
        std::fs::write(dir.join("specs/out.md"), text).unwrap();
        edit(&dir.join(PROPOSAL), "branch: main", "branch: other");
        std::fs::write(p.root.join("config.toml"), "base = \"trunk\"\n").unwrap();
        let got: Vec<String> = drift_of(&p, &repo)
            .into_iter()
            .map(|(c, w)| format!("{}: {w}", c.name()))
            .collect();
        assert!(
            got.contains(&"weaken: config changed".to_string()),
            "{got:?}"
        );
        assert!(
            got.contains(&"weaken: new delta Remove REQ-zzzzzz".to_string()),
            "{got:?}"
        );
        assert!(
            got.iter()
                .any(|g| g.starts_with("confirm: requirement REQ-")),
            "{got:?}"
        );
        assert!(
            got.contains(&"confirm: branch changed".to_string()),
            "{got:?}"
        );
        assert!(
            got.contains(&"confirm: proposal changed".to_string()),
            "{got:?}"
        );
    }

    #[test]
    fn protected_test_edits_are_weaken() {
        let tmp = crate::git::testrepo::TempDir::new();
        let home = tmp.0.join("home");
        let repo = crate::git::testrepo::repo_with_commit(&tmp.0, "r");
        std::fs::create_dir_all(repo.join("tests")).unwrap();
        std::fs::write(repo.join("tests/t.sh"), "exit 0").unwrap();
        let p = crate::store::init(&repo, &home, crate::store::Mode::Home).unwrap();
        let c = change::new(&p, "T", "main", true).unwrap();
        std::fs::write(
            c.path.join(TASKS),
            "- TSK-new1 T\n  verify: false\n  files: src/**\n",
        )
        .unwrap();
        let prepared = vow::prepare(&p, &home, c, &repo, &mut std::io::sink()).unwrap();
        vow::commit(&p, prepared).unwrap();

        assert_eq!(drift_of(&p, &repo), []);
        std::fs::write(repo.join("tests/t.sh"), "exit 0 # weaker").unwrap();
        std::fs::write(repo.join("tests/new.sh"), "exit 0").unwrap();
        assert_eq!(
            drift_of(&p, &repo),
            [(
                Class::Weaken,
                "protected file tests/t.sh changed".to_string()
            )]
        );
        std::fs::remove_file(repo.join("tests/t.sh")).unwrap();
        assert_eq!(
            drift_of(&p, &repo),
            [(
                Class::Weaken,
                "protected file tests/t.sh removed".to_string()
            )]
        );
    }
}
