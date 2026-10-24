use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use super::rerun::changed_files;
use super::signature::{amendment_ancestor, bound_confirm_payload, signature_ok};
use super::{Options, Report};
use crate::amend;
use crate::change::{self, CapSpec, LEDGER, Stage};
use crate::diag::Severity;
use crate::drift;
use crate::error::Result;
use crate::fingerprint::globset;
use crate::git;
use crate::ledger::{self, Kind};
use crate::proof::{self, Active, Now, State};
use crate::record;
use crate::store::{Mode, Project, REPO_DIR};
use crate::vow::requirement_hash;

const MAX_LISTED: usize = 20;

pub(super) fn list(items: &[String]) -> String {
    let mut s = items.iter().take(MAX_LISTED).cloned().collect::<Vec<_>>().join(", ");
    if items.len() > MAX_LISTED {
        s.push_str(&format!(" and {} more", items.len() - MAX_LISTED));
    }
    s
}

/// A change outside active/ whose ledger does not hold the event that moves it
/// there was moved by hand.
pub fn check_lifecycle(project: &Project, r: &mut Report) -> Result<()> {
    let (all, _) = change::list(project)?;
    for c in all.iter().filter(|c| c.stage != Stage::Active) {
        let (events, _) = ledger::read(&c.path.join(LEDGER))?;
        let has = |k: Kind| events.iter().any(|e| e.kind == k);
        let problem = match c.stage {
            Stage::Backlog if has(Kind::Vow) => "has a vow but sits in backlog/",
            Stage::Done if !has(Kind::Archive) => "sits in archive/done/ without an archive event",
            Stage::Dropped if !has(Kind::Drop) => "sits in archive/dropped/ without a drop event",
            Stage::Superseded if !events.iter().any(|e| e.kind == Kind::Supersede && e.data.contains_key("superseded_by")) => {
                "sits in archive/superseded/ without a supersede event"
            }
            _ => continue,
        };
        r.failures.push(format!("{} {problem}; it was moved by hand", c.id));
    }
    Ok(())
}

/// Every living requirement matches the hash in spec-record.json. A project
/// from before the record existed has none, and the check waits for one.
pub fn check_record(project: &Project, living: &[CapSpec], r: &mut Report) -> Result<()> {
    let mut record = record::load(project)?;
    if record.is_empty() {
        return Ok(());
    }
    for (cap, spec) in living {
        for req in &spec.requirements {
            let id = req.id.to_string();
            let what = match record.remove(&id) {
                None => "is not in spec-record.json",
                Some(e) if e.hash != requirement_hash(req) => "does not match its recorded hash",
                Some(_) => continue,
            };
            r.failures.push(format!("{id} in specs/{cap}/spec.md {what}; a living spec was edited outside a change"));
        }
    }
    for id in record.keys() {
        r.failures.push(format!("{id} is in spec-record.json but in no living spec; a living spec was edited outside a change"));
    }
    Ok(())
}

pub fn check_change(project: &Project, c: &Active, now: &Now, opts: &Options, r: &mut Report) -> Result<()> {
    check_change_only(project, c, now, opts, None, r)
}

/// Like `check_change`, checking the proofs of the tasks in `only` alone when it is set.
pub(crate) fn check_change_only(
    project: &Project,
    c: &Active,
    now: &Now,
    opts: &Options,
    only: Option<&[String]>,
    r: &mut Report,
) -> Result<()> {
    if let Ok(home) = crate::store::home_dir() {
        drift::fetch_missing_grammars(project, now.top, &home, now.config, c);
    }
    let id = &c.dir.id;
    for (path, d) in c.loaded.errors() {
        r.failures.push(format!("{id}: {path}:{}: {}", d.line, d.message));
    }
    for d in &c.ledger_diags {
        let line = format!("{id}: {LEDGER}:{}: {}", d.line, d.message);
        match d.severity() {
            Severity::Error => r.failures.push(line),
            Severity::Warning => r.warnings.push(line),
        }
    }
    for item in drift::drift(project, now.top, now.config, c)?.into_iter().filter(|i| i.class.blocks()) {
        r.failures.push(format!(
            "{id}: {} drift: {}; record it with rvow amend --reason and ask the user to confirm",
            item.class.name(),
            item.what
        ));
    }
    for p in amend::pending(std::slice::from_ref(c)) {
        r.failures.push(format!("{id}: amendment {} waits for the user's confirmation", p.id));
    }
    if now.config.require_signature {
        for e in c.events.iter().filter(|e| e.kind == Kind::Confirm) {
            let amd = e.data.get("amendment").and_then(Value::as_str).unwrap_or("");
            let signer = e.data.get("signer").and_then(Value::as_str).unwrap_or("");
            let decision = e.data.get("decision").and_then(Value::as_str).unwrap_or("");
            let signature = e.data.get("signature").and_then(Value::as_str).unwrap_or("");
            let allowed = Path::new(&now.config.allowed_signers);
            let matches: Vec<_> =
                c.events.iter().filter(|a| a.kind == Kind::Amend && a.data.get("amendment").and_then(Value::as_str) == Some(amd)).collect();
            let valid = matches.len() == 1 && {
                let amendment = matches[0];
                e.data.get("amendment_event").and_then(Value::as_str) == Some(amendment.hash.as_str())
                    && amendment_ancestor(&c.events, e, amendment)
                    && bound_confirm_payload(project, &c.dir.id, amendment, decision)
                        .is_ok_and(|payload| signature_ok(allowed, signer, &payload, signature))
            };
            if !valid {
                r.failures.push(format!("{id}: confirmation {amd} is not signed with an allowed key"));
            }
        }
    }
    if opts.rerun {
        return Ok(());
    }
    for t in c.loaded.tasks.iter().filter(|t| only.is_none_or(|only| only.contains(&t.id.to_string()))) {
        let s = proof::state(c, t, now)?;
        let line = match &s {
            State::Stale(why) => format!("{} is stale ({})", t.id, why.join(", ")),
            other => format!("{} is {}", t.id, other.name()),
        };
        match s {
            State::Proved => {}
            State::Stale(_) | State::Flaky if !opts.strict => r.warnings.push(line),
            _ => r.failures.push(line),
        }
    }
    Ok(())
}

/// A done change counts toward scope only while its work is not in the base:
/// its archive HEAD, or its vow base when no HEAD was recorded, must not be
/// a strict ancestor of the merge base. No point or an unknown commit counts,
/// so a missing object never breaks the gate.
fn done_counts(top: &Path, events: &[ledger::Event], mb: &str) -> bool {
    let head = events.iter().rev().find(|e| e.kind == Kind::Archive).and_then(|e| e.data.get("head")).and_then(Value::as_str);
    let vow_base = events.iter().find(|e| e.kind == Kind::Vow).and_then(|e| e.data.get("base_commit")).and_then(Value::as_str);
    let point = match head.or(vow_base) {
        Some(p) => p,
        None => return true,
    };
    if point == mb {
        return true;
    }
    git::run(top, &["merge-base", "--is-ancestor", point, mb]).is_err()
}

/// Whether the work of a change of another branch is part of this stacked
/// branch: the tip of that branch is an ancestor of HEAD and not an ancestor
/// of the merge base `mb`. A branch with no tip does not count. `seen` holds
/// the answer per branch, so each branch is resolved once.
fn stacked_counts(top: &Path, other: &str, mb: &str, seen: &mut HashMap<String, bool>) -> bool {
    if let Some(&known) = seen.get(other) {
        return known;
    }
    let counts = git::branch_tip(top, other).is_some_and(|tip| {
        git::run(top, &["merge-base", "--is-ancestor", &tip, "HEAD"]).is_ok()
            && git::run(top, &["merge-base", "--is-ancestor", &tip, mb]).is_err()
    });
    seen.insert(other.to_string(), counts);
    counts
}

/// The `files:` globs of every task in scope: the queue's tasks, the files
/// their baseline took in, and the tasks and taken-in files of every done
/// change whose work is not in the base yet, plus those of another branch's
/// change stacked under this one.
pub(crate) fn scope_globs(
    project: &Project,
    top: &Path,
    branch: &str,
    living: &[CapSpec],
    queue: &[&Active],
    mb: &str,
) -> Result<Vec<String>> {
    let mut globs: Vec<String> = queue.iter().flat_map(|c| c.loaded.tasks.iter().flat_map(|t| t.files.iter().cloned())).collect();
    for c in queue {
        globs.extend(drift::outside_of(c.baseline()).iter().map(|p| globset::escape(p)));
    }
    let (all, _) = change::list(project)?;
    let mut seen: HashMap<String, bool> = HashMap::new();
    for d in all.iter().filter(|d| matches!(d.stage, Stage::Done | Stage::Active)) {
        let loaded = change::load(&d.path, living)?;
        let counts = if loaded.proposal.branch == branch {
            // Every active change of the branch counts, also one outside `queue`,
            // such as a sibling of the one change whose drift asks.
            let events = ledger::read(&d.path.join(LEDGER))?.0;
            let counts = d.stage == Stage::Active || done_counts(top, &events, mb);
            if counts {
                globs.extend(drift::outside_of(proof::baseline_of(&events)).iter().map(|p| globset::escape(p)));
            }
            counts
        } else {
            stacked_counts(top, &loaded.proposal.branch, mb, &mut seen)
        };
        if counts {
            globs.extend(loaded.tasks.iter().flat_map(|t| t.files.iter().cloned()));
        }
    }
    Ok(globs)
}

pub(super) fn check_scope(
    project: &Project,
    top: &Path,
    base: &str,
    branch: &str,
    living: &[CapSpec],
    queue: &[&Active],
    r: &mut Report,
) -> Result<()> {
    let Some(base_ref) = git::base_ref(top, base) else {
        r.failures.push(format!("neither origin/{base} nor {base} exists; set base in config.toml"));
        return Ok(());
    };
    let mb = git::run(top, &["merge-base", "HEAD", &base_ref])?;
    for c in queue {
        let vowed = c.vow().and_then(|v| v.data.get("base_commit")).and_then(Value::as_str).unwrap_or("");
        if !vowed.is_empty() && git::run(top, &["merge-base", "--is-ancestor", vowed, &mb]).is_err() {
            r.failures.push(format!(
                "{}: the merge base with {base_ref} is not after the base recorded at accept; {base} moved backwards",
                c.dir.id
            ));
        }
    }
    let set = globset(&scope_globs(project, top, branch, living, queue, &mb)?)?;
    let repo_dir = format!("{REPO_DIR}/");
    let outside: Vec<String> = changed_files(top, &mb)?
        .into_iter()
        .filter(|f| !(project.meta.mode == Mode::Repo && f.starts_with(&repo_dir)))
        .filter(|f| !set.is_match(f))
        .collect();
    if !outside.is_empty() {
        r.failures.push(format!("{} changed files lie outside every task's files: {}", outside.len(), list(&outside)));
    }
    Ok(())
}

/// Work written before any change was accepted: while a backlog plan names
/// this branch and no change is active on it, every file that differs from
/// the base outside the rvow tree and outside every done change of the branch
/// fails, naming the waiting plan. Shell work done before accept must not
/// push clean.
pub(super) fn check_backlog_scope(
    project: &Project,
    top: &Path,
    base: &str,
    branch: &str,
    living: &[CapSpec],
    r: &mut Report,
) -> Result<()> {
    let (all, _) = change::list(project)?;
    let mut waiting: Vec<String> = Vec::new();
    let mut done: Vec<&change::ChangeDir> = Vec::new();
    let mut other: Vec<(&change::ChangeDir, String)> = Vec::new();
    for d in &all {
        let loaded = change::load(&d.path, living)?;
        if loaded.proposal.branch != branch {
            if matches!(d.stage, Stage::Done | Stage::Active) {
                other.push((d, loaded.proposal.branch));
            }
            continue;
        }
        match d.stage {
            Stage::Backlog => waiting.push(d.id.to_string()),
            Stage::Done => done.push(d),
            _ => {}
        }
    }
    if waiting.is_empty() {
        return Ok(());
    }
    waiting.sort();
    let plans = waiting.join(", ");
    let Some(base_ref) = git::base_ref(top, base) else {
        r.failures.push(format!("neither origin/{base} nor {base} exists; set base in config.toml"));
        return Ok(());
    };
    let mb = git::run(top, &["merge-base", "HEAD", &base_ref])?;
    let mut done_globs: Vec<String> = Vec::new();
    for d in done {
        let loaded = change::load(&d.path, living)?;
        let (events, _) = ledger::read(&d.path.join(LEDGER))?;
        if done_counts(top, &events, &mb) {
            done_globs.extend(loaded.tasks.iter().flat_map(|t| t.files.iter().cloned()));
        }
    }
    let mut seen: HashMap<String, bool> = HashMap::new();
    for (d, other_branch) in other {
        if stacked_counts(top, &other_branch, &mb, &mut seen) {
            let loaded = change::load(&d.path, living)?;
            done_globs.extend(loaded.tasks.iter().flat_map(|t| t.files.iter().cloned()));
        }
    }
    let set = globset(&done_globs)?;
    let repo_dir = format!("{REPO_DIR}/");
    let mut differed: Vec<String> = changed_files(top, &mb)?
        .into_iter()
        .filter(|f| !(project.meta.mode == Mode::Repo && f.starts_with(&repo_dir)))
        .filter(|f| !set.is_match(f))
        .collect();
    differed.sort();
    for f in differed.iter().take(MAX_LISTED) {
        r.failures.push(format!("{f} differs from {base} while {plans} waits for accept; accept the plan first, then write the code"));
    }
    if differed.len() > MAX_LISTED {
        r.failures.push(format!("and {} more files differ from {base} while {plans} waits for accept", differed.len() - MAX_LISTED));
    }
    Ok(())
}
