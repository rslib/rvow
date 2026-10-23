//! `rvow amend` proposes the current content of a change as its new baseline,
//! and `rvow confirm` lets the user accept or reject each proposal.

use std::path::Path;

use serde_json::{Map, Value, json};

use crate::change::{self, LEDGER};
use crate::config;
use crate::drift::{self, Item};
use crate::error::{Error, Result};
use crate::git;
use crate::id::{self, Id, Kind as IdKind};
use crate::ledger::{self, Kind, short_hash};
use crate::proof::{self, Active};
use crate::protected;
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

fn class_rank(v: &Value) -> u8 {
    match v.get("class").and_then(Value::as_str) {
        Some("weaken") => 0,
        Some("confirm") => 1,
        Some("free") => 2,
        _ => 3,
    }
}

impl Pending {
    /// Items weaken first, then confirm, then free, in stored order within
    /// each class, as the confirm listing shows them.
    pub fn ordered_items(&self) -> Vec<&Value> {
        let mut items: Vec<&Value> = self.items.iter().collect();
        items.sort_by_key(|i| class_rank(i));
        items
    }

    /// The confirm listing of this amendment: the header, the items weaken
    /// first, each baseline result, and the reason. `detail` sets how much of
    /// each changed line it shows. The digest line comes from `listing`, so
    /// `--all` decides exactly the set that was read.
    pub fn render(&self, detail: Detail) -> Vec<String> {
        let mut lines = vec![format!("{} for {}", self.id, self.change)];
        let (mut hidden, mut cut) = (0, false);
        for i in self.ordered_items() {
            let class = i.get("class").and_then(|v| v.as_str()).unwrap_or("");
            let what = i.get("what").and_then(|v| v.as_str()).unwrap_or("");
            lines.push(format!("  {class}: {what}"));
            if let (Some(old), Some(new)) = (i.get("before").and_then(|v| v.as_str()), i.get("after").and_then(|v| v.as_str())) {
                let (old, new): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
                let head = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
                let tail = old[head..].iter().rev().zip(new[head..].iter().rev()).take_while(|(a, b)| a == b).count();
                let changed =
                    old[head..old.len() - tail].iter().map(|l| ('-', *l)).chain(new[head..new.len() - tail].iter().map(|l| ('+', *l)));
                for (sign, line) in changed {
                    match detail {
                        Detail::Summary => hidden += 1,
                        Detail::Clipped => {
                            let (shown, was_cut) = clip(line);
                            cut |= was_cut;
                            lines.push(format!("    {sign} {shown}"));
                        }
                        Detail::Full => lines.push(format!("    {sign} {line}")),
                    }
                }
            }
        }
        if hidden > 0 {
            let noun = if hidden == 1 { "line" } else { "lines" };
            lines.push(format!("  diff: {hidden} changed {noun} hidden; rvow confirm {} shows them", self.id));
        }
        if cut {
            lines.push(format!("  cut: long lines are cut; rvow confirm {} --full shows them whole", self.id));
        }
        lines.push(format!("  reason (from the agent): {}", self.reason));
        lines
    }
}

/// How much of each changed line a confirm listing shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// Only the item names and a count of the changed lines.
    Summary,
    /// Each changed line, cut at `CLIP_CHARS`.
    Clipped,
    /// Each changed line whole, as the user reads it before a decision.
    Full,
}

/// The longest changed line that `Detail::Clipped` shows whole.
pub const CLIP_CHARS: usize = 500;

fn clip(line: &str) -> (String, bool) {
    let total = line.chars().count();
    if total <= CLIP_CHARS {
        return (line.to_string(), false);
    }
    let kept: String = line.chars().take(CLIP_CHARS).collect();
    (format!("{kept}... (cut, {total} chars)"), true)
}

/// The confirm listing of every pending amendment, ending with the digest
/// line that `--all` decides with.
pub fn listing(pending: &[Pending], detail: Detail) -> Vec<String> {
    let mut lines = Vec::new();
    for p in pending {
        lines.extend(p.render(detail));
    }
    lines.push(format!("digest {}", digest(pending)));
    lines
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
    let mut changes = proof::active(project, &living)?;
    let wanted: Option<Id> = target.map(|t| change::resolve(project, t)).transpose()?.map(|c| c.id);
    let branch = git::branch(&top)?;
    let config = config::load(project, home)?;
    let unlock = match unlock {
        Some(path) => {
            let rel = protected::rel_path(Path::new(path), cwd, &top)
                .ok_or_else(|| Error::new("unlock", format!("{path} is outside the repository"), "name a file in this repository"))?;
            if !protected::on_branch(project, &top)?.contains(&rel) {
                return Err(Error::new("unlock", format!("{rel} is not protected"), "edit it without an unlock"));
            }
            Some(rel)
        }
        None => None,
    };
    let unlock = unlock.as_deref();
    let lock = project.lock()?;
    let mut out = Vec::new();
    for c in &mut changes {
        let selected = match &wanted {
            Some(id) => &c.dir.id == id,
            None => c.branch() == branch.as_deref(),
        };
        if !selected {
            continue;
        }
        let top = c.branch().and_then(|b| git::top_for_branch(cwd, b).ok().flatten()).unwrap_or_else(|| top.clone());
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
                crate::vow::mint_placeholders(project, &lock, &c.dir, &c.loaded)?;
                c.loaded = change::load(&c.dir.path, &living)?;
                let items = drift::drift_with_home(project, Some(home), &top, &config, c)?;
                if items.is_empty() {
                    continue;
                }
                let snapshot = drift::snapshot_now(project, &top, &config, c)?;
                data.insert("snapshot".into(), Value::Object(snapshot));
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
            let mut decided: Vec<(Id, String)> = Vec::new();
            for c in &changes {
                for e in c.events.iter().filter(|e| e.kind == Kind::Confirm) {
                    let pair = match (e.data.get("amendment").and_then(Value::as_str), e.data.get("decision").and_then(Value::as_str)) {
                        (Some(a), Some(d)) => (a, d),
                        _ => continue,
                    };
                    let Ok(amendment) = pair.0.parse::<Id>() else {
                        continue;
                    };
                    decided.push((amendment, pair.1.to_string()));
                }
            }
            decided.sort();
            decided.dedup();
            let mut chosen = Vec::new();
            for input in ids {
                let input = if input.contains('-') { input.clone() } else { format!("AMD-{input}") };
                let id = match id::resolve(&input, &known) {
                    Ok(id) => id,
                    Err(id::ResolveError::NotFound) => {
                        let decided_ids: Vec<Id> = decided.iter().map(|(id, _)| id.clone()).collect();
                        match id::resolve(&input, &decided_ids) {
                            Ok(found) => {
                                let past = decided
                                    .iter()
                                    .find(|(id, _)| *id == found)
                                    .map(|(_, d)| match d.as_str() {
                                        "accept" => "accepted".to_string(),
                                        "reject" => "rejected".to_string(),
                                        other => other.to_string(),
                                    })
                                    .unwrap_or_else(|| "decided".to_string());
                                return Err(Error::new(
                                    "decided",
                                    format!("{found} is already {past}"),
                                    "run rvow confirm to list the pending amendments",
                                ));
                            }
                            Err(e) => {
                                return Err(Error::new(
                                    "amendment",
                                    format!("{input}: {e}"),
                                    "run rvow confirm to list the pending amendments",
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        return Err(Error::new("amendment", format!("{input}: {e}"), "run rvow confirm to list the pending amendments"));
                    }
                };
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
