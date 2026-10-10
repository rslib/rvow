//! `rvow amend` proposes the current content of a change as its new baseline,
//! and `rvow confirm` lets the user accept or reject each proposal.

use std::path::Path;

use serde_json::{Map, Value, json};

use crate::change::{self, LEDGER};
use crate::config;
use crate::drift::{self, Item};
use crate::error::{Error, Result};
use crate::git;
use crate::hook;
use crate::id::{self, Id, Kind as IdKind};
use crate::ledger::{self, Kind, short_hash};
use crate::proof::{self, Active};
use crate::registry;
use crate::store::Project;
use crate::vow::sha256_hex;

#[derive(Debug, Clone)]
pub struct Pending {
    pub id: Id,
    pub change: Id,
    pub change_path: std::path::PathBuf,
    pub reason: String,
    pub items: Vec<Value>,
    pub snapshot: String,
    /// The task ids the amendment's items name, or `None` for an amendment
    /// recorded before this key existed. `None` releases the whole change.
    pub tasks: Option<Vec<String>>,
}

fn decided(change: &Active, amendment: &str) -> bool {
    change.events.iter().any(|e| e.kind == Kind::Confirm && e.data.get("amendment").and_then(Value::as_str) == Some(amendment))
}

/// Every amendment in the active changes that has no decision yet.
pub fn pending(changes: &[Active]) -> Vec<Pending> {
    let mut out = Vec::new();
    for c in changes {
        for e in c.events.iter().filter(|e| e.kind == Kind::Amend) {
            let Some(id) = e.data.get("amendment").and_then(Value::as_str) else {
                continue;
            };
            let Ok(parsed) = id.parse::<Id>() else {
                continue;
            };
            if decided(c, id) {
                continue;
            }
            let snapshot = e.data.get("snapshot").cloned().unwrap_or(Value::Null);
            out.push(Pending {
                id: parsed,
                change: c.dir.id.clone(),
                change_path: c.dir.path.clone(),
                reason: e.data.get("reason").and_then(Value::as_str).unwrap_or("").to_string(),
                items: e.data.get("items").and_then(Value::as_array).cloned().unwrap_or_default(),
                snapshot: sha256_hex(snapshot.to_string().as_bytes()),
                tasks: e
                    .data
                    .get("tasks")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect()),
            });
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// A short hash of the pending set. `confirm --all <digest>` accepts exactly
/// the set the user read, so an amendment that arrives later is not included.
pub fn digest(pending: &[Pending]) -> String {
    let lines: Vec<String> = pending.iter().map(|p| format!("{} {}", p.id, p.snapshot)).collect();
    short_hash(lines.join("\n").as_bytes())[..8].to_string()
}

#[derive(Debug)]
pub struct Amended {
    pub id: Id,
    pub change: Id,
    pub items: Vec<Item>,
}

/// Records an amendment for `target`, or for every active change on the
/// current branch that has drift.
pub fn amend(project: &Project, home: &Path, cwd: &Path, target: Option<&str>, reason: &str, unlock: Option<&str>) -> Result<Vec<Amended>> {
    if reason.trim().is_empty() {
        return Err(Error::new("reason", "an amendment needs a reason", "say why the plan changed with --reason \"...\""));
    }
    let top = git::toplevel(cwd)?;
    let (living, _) = change::load_living(project)?;
    let changes = proof::active(project, &living)?;
    let wanted: Option<Id> = target.map(|t| change::resolve(project, t)).transpose()?.map(|c| c.id);
    let branch = git::branch(&top)?;
    let config = config::load(project, home)?;
    let unlock = match unlock {
        Some(path) => {
            let rel = hook::rel_path(Path::new(path), cwd, &top)
                .ok_or_else(|| Error::new("unlock", format!("{path} is outside the repository"), "name a file in this repository"))?;
            if !hook::protected(project, &top)?.contains(&rel) {
                return Err(Error::new("unlock", format!("{rel} is not protected"), "edit it without an unlock"));
            }
            Some(rel)
        }
        None => None,
    };
    let unlock = unlock.as_deref();
    let lock = project.lock()?;
    let mut out = Vec::new();
    for c in &changes {
        let selected = match &wanted {
            Some(id) => &c.dir.id == id,
            None => c.branch() == branch.as_deref(),
        };
        if !selected {
            continue;
        }
        let mut data = Map::new();
        let items = match unlock {
            // An unlock keeps the current baseline, so accepting it cannot
            // also accept other drift the user did not see.
            Some(rel) => {
                let base = c.baseline().cloned().unwrap_or_default();
                data.insert("unlock".into(), rel.into());
                data.insert("snapshot".into(), Value::Object(base));
                vec![Item {
                    class: drift::Class::Weaken,
                    what: format!("unlock protected file {rel} for editing"),
                    before: None,
                    after: None,
                }]
            }
            None => {
                let items = drift::drift(project, &top, &config, c)?;
                if items.is_empty() {
                    continue;
                }
                data.insert("snapshot".into(), Value::Object(drift::snapshot_now(project, &top, &config, c)?));
                items
            }
        };
        let tasks: Vec<String> = c
            .loaded
            .tasks
            .iter()
            .map(|t| t.id.to_string())
            .filter(|id| items.iter().any(|i| i.what.starts_with(&format!("{id} "))))
            .collect();
        let id = registry::mint(project, &lock, IdKind::Amd, Some(&c.dir.id))?;
        data.insert("amendment".into(), id.to_string().into());
        data.insert("reason".into(), reason.trim().into());
        data.insert("items".into(), json!(items));
        data.insert("tasks".into(), json!(tasks));
        ledger::append(&c.dir.path.join(LEDGER), &lock, Kind::Amend, data)?;
        out.push(Amended { id, change: c.dir.id.clone(), items });
    }
    if out.is_empty() {
        return Err(Error::new("no-drift", "nothing differs from the accepted plan", "edit the plan first; amend records the edit"));
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Accept,
    Reject,
}

impl Decision {
    fn name(self) -> &'static str {
        match self {
            Decision::Accept => "accept",
            Decision::Reject => "reject",
        }
    }
}

/// Records `decision` for each named amendment, or for the whole pending set
/// when `all_digest` matches it.
pub fn confirm(project: &Project, ids: &[String], decision: Decision, all_digest: Option<&str>) -> Result<Vec<Id>> {
    confirm_signed(project, ids, decision, all_digest, None)
}

/// A key and explicit allowed-signers principal for a confirmation.
pub struct Signing<'a> {
    pub key: &'a Path,
    pub signer: &'a str,
}

/// Selects and records decisions under one lock. All signatures are prepared
/// before appending any confirmation, so a signing failure records nothing.
pub fn confirm_signed(
    project: &Project,
    ids: &[String],
    decision: Decision,
    all_digest: Option<&str>,
    signing: Option<Signing<'_>>,
) -> Result<Vec<Id>> {
    let lock = project.lock()?;
    if signing.as_ref().is_some_and(|s| s.signer.trim().is_empty()) {
        return Err(Error::new("signer", "the signer identity is empty", "pass --signer <identity>"));
    }
    let (living, _) = change::load_living(project)?;
    let changes = proof::active(project, &living)?;
    let pending = pending(&changes);
    let chosen: Vec<&Pending> = match all_digest {
        Some(d) => {
            let now = digest(&pending);
            if d != now {
                return Err(Error::new(
                    "digest",
                    format!("the pending set changed since it was listed (digest now {now})"),
                    "run rvow confirm again and read the new list",
                ));
            }
            pending.iter().collect()
        }
        None => {
            let known: Vec<Id> = pending.iter().map(|p| p.id.clone()).collect();
            let mut chosen = Vec::new();
            for input in ids {
                let input = if input.contains('-') { input.clone() } else { format!("AMD-{input}") };
                let id = id::resolve(&input, &known)
                    .map_err(|e| Error::new("amendment", format!("{input}: {e}"), "run rvow confirm to list the pending amendments"))?;
                let found =
                    pending.iter().find(|p| p.id == id).ok_or_else(|| Error::internal(format!("{id} resolved but is not pending")))?;
                chosen.push(found);
            }
            chosen
        }
    };
    if chosen.is_empty() {
        return Err(Error::new("none", "no amendment chosen", "name amendment ids or pass --all <digest>"));
    }
    let mut prepared = Vec::new();
    for p in &chosen {
        let c = changes.iter().find(|c| c.dir.id == p.change).ok_or_else(|| Error::internal("pending change missing"))?;
        let amendment_id = p.id.to_string();
        let events: Vec<_> = c
            .events
            .iter()
            .filter(|e| e.kind == Kind::Amend && e.data.get("amendment").and_then(Value::as_str) == Some(amendment_id.as_str()))
            .collect();
        if events.len() != 1 {
            return Err(Error::new(
                "amendment",
                format!("{} has ambiguous amendment events", p.id),
                "inspect the change ledger before confirming",
            ));
        }
        let event = events[0];
        let mut data = Map::new();
        data.insert("amendment".into(), p.id.to_string().into());
        data.insert("decision".into(), decision.name().into());
        data.insert("amendment_event".into(), event.hash.clone().into());
        if let Some(signing) = &signing {
            let payload = crate::gate::bound_confirm_payload(project, &p.change, event, decision.name())?;
            data.insert("signer".into(), signing.signer.into());
            data.insert("signature".into(), crate::gate::sign(signing.key, &payload)?.into());
        }
        prepared.push((p.change_path.join(LEDGER), data));
    }
    for (path, data) in prepared {
        ledger::append(&path, &lock, Kind::Confirm, data)?;
    }
    Ok(chosen.iter().map(|p| p.id.clone()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proof::tests::accepted;

    fn active_dir(p: &Project) -> std::path::PathBuf {
        std::fs::read_dir(p.root.join("active")).unwrap().next().unwrap().unwrap().path()
    }

    fn state(p: &Project, home: &Path, repo: &Path) -> (usize, Vec<Pending>) {
        let (living, _) = change::load_living(p).unwrap();
        let changes = proof::active(p, &living).unwrap();
        let cfg = config::load(p, home).unwrap();
        let top = git::toplevel(repo).unwrap();
        let d = drift::drift(p, &top, &cfg, &changes[0]).unwrap();
        (d.len(), pending(&changes))
    }

    #[test]
    fn amend_then_accept_moves_the_baseline() {
        let (tmp, p, repo, _) = accepted();
        let home = tmp.0.join("home");
        assert_eq!(amend(&p, &home, &repo, None, "x", None).unwrap_err().code, "no-drift");
        assert_eq!(amend(&p, &home, &repo, None, " ", None).unwrap_err().code, "reason");

        let tasks = active_dir(&p).join("tasks.md");
        let text = std::fs::read_to_string(&tasks).unwrap();
        std::fs::write(&tasks, text.replace("Write ok", "Write ok now")).unwrap();
        let a = amend(&p, &home, &repo, None, "clearer title", None).unwrap();
        assert_eq!(a.len(), 1);
        let (drift, pend) = state(&p, &home, &repo);
        assert_eq!((drift, pend.len()), (1, 1));

        let id = pend[0].id.to_string();
        confirm(&p, &[id[4..8].to_string()], Decision::Accept, None).unwrap();
        let (drift, pend) = state(&p, &home, &repo);
        assert_eq!((drift, pend.len()), (0, 0));
    }

    #[test]
    fn reject_keeps_the_drift_and_all_needs_the_digest() {
        let (tmp, p, repo, _) = accepted();
        let home = tmp.0.join("home");
        let tasks = active_dir(&p).join("tasks.md");
        let text = std::fs::read_to_string(&tasks).unwrap();
        std::fs::write(&tasks, text.replace("Write ok", "Other")).unwrap();
        amend(&p, &home, &repo, None, "one", None).unwrap();
        let (_, pend) = state(&p, &home, &repo);
        let first = digest(&pend);
        amend(&p, &home, &repo, None, "two", None).unwrap();

        let err = confirm(&p, &[], Decision::Accept, Some(&first)).unwrap_err();
        assert_eq!(err.code, "digest");
        let (_, pend) = state(&p, &home, &repo);
        let ids = confirm(&p, &[], Decision::Reject, Some(&digest(&pend))).unwrap();
        assert_eq!(ids.len(), 2);
        let (drift, pend) = state(&p, &home, &repo);
        assert_eq!((drift, pend.len()), (1, 0));
    }
}
