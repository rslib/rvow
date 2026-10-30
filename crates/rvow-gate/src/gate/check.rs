use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;
use std::rc::Rc;

use serde_json::Value;

use super::rerun::changed_files;
use super::signature::{amendment_ancestor, bound_confirm_payload, signature_ok};
use super::{Options, Report};
use crate::amend;
use crate::change::{self, CapSpec, LEDGER, PROPOSAL, Stage, TASKS};
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
        r.work(format!("{id}: {} drift: {}; record it with rvow amend --reason and ask the user to confirm", item.class.name(), item.what));
    }
    for p in amend::pending(std::slice::from_ref(c)) {
        r.work(format!("{id}: amendment {} waits for the user's confirmation", p.id));
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
            State::Stale(_) | State::Flaky if !opts.strict => r.soft(line),
            _ => r.work(line),
        }
    }
    Ok(())
}

/// A done change counts toward scope only while its work is not in the base:
/// its archive HEAD, or its vow base when no HEAD was recorded, must not be
/// a strict ancestor of the merge base. No point or an unknown commit counts,
/// so a missing object never breaks the gate. `seen` holds the answer per
/// point, so a run asks git about each one once.
fn done_counts(top: &Path, events: &[ledger::Event], mb: &str, seen: &mut HashMap<String, bool>) -> bool {
    point_counts(top, done_point(events), mb, seen)
}

/// The commit a done change is judged by: its archive HEAD, else its vow base.
fn done_point(events: &[ledger::Event]) -> Option<&str> {
    let head = events.iter().rev().find(|e| e.kind == Kind::Archive).and_then(|e| e.data.get("head")).and_then(Value::as_str);
    let vow_base = events.iter().find(|e| e.kind == Kind::Vow).and_then(|e| e.data.get("base_commit")).and_then(Value::as_str);
    head.or(vow_base)
}

/// `done_counts` for a point already read from the ledger.
fn point_counts(top: &Path, point: Option<&str>, mb: &str, seen: &mut HashMap<String, bool>) -> bool {
    let Some(point) = point else { return true };
    if point == mb {
        return true;
    }
    if let Some(&known) = seen.get(point) {
        return known;
    }
    let counts = git::run(top, &["merge-base", "--is-ancestor", point, mb]).is_err();
    seen.insert(point.to_string(), counts);
    counts
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
    globs.extend(store_globs(project, top, branch, living, mb)?);
    Ok(globs)
}

/// One active or done change as the store-wide scope reads it, with no git
/// fact in it: whether it counts depends on HEAD, the merge base and the other
/// branches, so `store_globs` asks git that on every call.
struct Scanned {
    branch: String,
    active: bool,
    /// The archive HEAD or vow base of a done change.
    point: Option<String>,
    /// The `files:` globs of its tasks.
    files: Vec<String>,
    /// The escaped paths its baseline took in.
    outside: Vec<String>,
}

thread_local! {
    /// The parsed changes of the store, with the key that makes them stale:
    /// one entry, replaced when a change file moves.
    static STORE_SCAN: RefCell<Option<(String, Rc<Vec<Scanned>>)>> = const { RefCell::new(None) };
}

#[cfg(test)]
thread_local! {
    static SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How old a file's mtime must be before its stat may stand for its text, as
/// git does for a racily clean index entry.
const RACY_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

/// The identity of a file the scan reads: its path, size and mtime, or a `-`
/// when it is not there. `false` when the file changed within the racy window,
/// so the same stat could still hide a new text.
fn stamp(out: &mut String, path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) => {
            let modified = meta.modified().ok();
            let at = modified.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
            let _ = write!(out, "{}:{}:{};", path.display(), meta.len(), at);
            modified.and_then(|m| std::time::SystemTime::now().duration_since(m).ok()).is_some_and(|age| age >= RACY_WINDOW)
        }
        Err(_) => {
            out.push_str("-;");
            true
        }
    }
}

/// What the scan reads: every change of the store with the size and mtime of
/// the files a load reads. The flag is `false` when a file is too new for its
/// stat to vouch for it.
fn scan_key(project: &Project) -> Result<(String, bool)> {
    let mut key = String::new();
    let mut settled = true;
    let _ = write!(key, "{}|", project.root.display());
    for d in &change::list(project)?.0 {
        let _ = write!(key, "{:?}|{}|", d.stage, d.path.display());
        for name in [PROPOSAL, TASKS, LEDGER] {
            settled &= stamp(&mut key, &d.path.join(name));
        }
    }
    Ok((key, settled))
}

/// The store-wide part of `scope_globs`: the done changes of the branch whose
/// work is not in the base, and the changes of another branch that stack
/// here. The parse of the store is kept while no change file moves; the git
/// questions run on each call.
pub(crate) fn store_globs(project: &Project, top: &Path, branch: &str, living: &[CapSpec], mb: &str) -> Result<Vec<String>> {
    let scanned = scanned(project, living)?;
    let mut globs = Vec::new();
    let mut points: HashMap<String, bool> = HashMap::new();
    let mut seen: HashMap<String, bool> = HashMap::new();
    for c in scanned.iter() {
        let counts = if c.branch == branch {
            c.active || point_counts(top, c.point.as_deref(), mb, &mut points)
        } else {
            stacked_counts(top, &c.branch, mb, &mut seen)
        };
        if counts {
            if c.branch == branch {
                globs.extend(c.outside.iter().cloned());
            }
            globs.extend(c.files.iter().cloned());
        }
    }
    Ok(globs)
}

/// The parsed active and done changes, from this thread's last scan when no
/// change file moved since.
fn scanned(project: &Project, living: &[CapSpec]) -> Result<Rc<Vec<Scanned>>> {
    let (key, settled) = scan_key(project)?;
    let known = STORE_SCAN.with(|cell| match &*cell.borrow() {
        Some((stored, scanned)) if *stored == key => Some(Rc::clone(scanned)),
        _ => None,
    });
    if let Some(scanned) = known {
        return Ok(scanned);
    }
    let scanned = Rc::new(scan_store(project, living)?);
    if settled {
        STORE_SCAN.with(|cell| *cell.borrow_mut() = Some((key, Rc::clone(&scanned))));
    }
    Ok(scanned)
}

/// Loads every active and done change of the store once.
fn scan_store(project: &Project, living: &[CapSpec]) -> Result<Vec<Scanned>> {
    #[cfg(test)]
    SCANS.with(|n| n.set(n.get() + 1));
    let mut out = Vec::new();
    for d in change::list(project)?.0.iter().filter(|d| matches!(d.stage, Stage::Done | Stage::Active)) {
        let loaded = change::load(&d.path, living)?;
        let events = ledger::read(&d.path.join(LEDGER))?.0;
        out.push(Scanned {
            branch: loaded.proposal.branch.clone(),
            active: d.stage == Stage::Active,
            point: done_point(&events).map(str::to_string),
            files: loaded.tasks.iter().flat_map(|t| t.files.iter().cloned()).collect(),
            outside: drift::outside_of(proof::baseline_of(&events)).iter().map(|p| globset::escape(p)).collect(),
        });
    }
    Ok(out)
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
        r.work(format!("neither origin/{base} nor {base} exists; set base in config.toml"));
        return Ok(());
    };
    let mb = git::run(top, &["merge-base", "HEAD", &base_ref])?;
    for c in queue {
        let vowed = c.vow().and_then(|v| v.data.get("base_commit")).and_then(Value::as_str).unwrap_or("");
        if !vowed.is_empty() && git::run(top, &["merge-base", "--is-ancestor", vowed, &mb]).is_err() {
            r.work(format!(
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
        r.work(format!("{} changed files lie outside every task's files: {}", outside.len(), list(&outside)));
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
    let mut done: Vec<(&change::ChangeDir, change::Loaded)> = Vec::new();
    let mut other: Vec<(&change::ChangeDir, change::Loaded)> = Vec::new();
    for d in &all {
        let loaded = change::load(&d.path, living)?;
        if loaded.proposal.branch != branch {
            if matches!(d.stage, Stage::Done | Stage::Active) {
                other.push((d, loaded));
            }
            continue;
        }
        match d.stage {
            Stage::Backlog => waiting.push(d.id.to_string()),
            Stage::Done => done.push((d, loaded)),
            _ => {}
        }
    }
    if waiting.is_empty() {
        return Ok(());
    }
    waiting.sort();
    let plans = waiting.join(", ");
    let Some(base_ref) = git::base_ref(top, base) else {
        r.work(format!("neither origin/{base} nor {base} exists; set base in config.toml"));
        return Ok(());
    };
    let mb = git::run(top, &["merge-base", "HEAD", &base_ref])?;
    let mut done_globs: Vec<String> = Vec::new();
    let mut points: HashMap<String, bool> = HashMap::new();
    for (d, loaded) in &done {
        let (events, _) = ledger::read(&d.path.join(LEDGER))?;
        if done_counts(top, &events, &mb, &mut points) {
            done_globs.extend(loaded.tasks.iter().flat_map(|t| t.files.iter().cloned()));
        }
    }
    let mut seen: HashMap<String, bool> = HashMap::new();
    for (_, loaded) in &other {
        if stacked_counts(top, &loaded.proposal.branch, &mb, &mut seen) {
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
        r.work(format!("{f} differs from {base} while {plans} waits for accept; accept the plan first, then write the code"));
    }
    if differed.len() > MAX_LISTED {
        r.work(format!("and {} more files differ from {base} while {plans} waits for accept", differed.len() - MAX_LISTED));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;
    use crate::store::{Mode, init};
    use std::path::PathBuf;

    /// A vowed change with one task over `files`, moved to archive/done when
    /// `done` asks for it.
    fn vow_change(p: &Project, home: &Path, repo: &Path, name: &str, files: &str, mode: &str, done: bool) -> PathBuf {
        let c = change::new(p, name, "main", false).unwrap();
        std::fs::write(c.path.join(TASKS), format!("- TSK-new1 T\n  verify: true\n  files: {files}\n  mode: {mode}\n")).unwrap();
        let prepared = crate::vow::prepare(p, home, c, repo, &mut std::io::sink()).unwrap();
        // The vow moves the change from backlog/ to active/.
        let dir = crate::vow::commit(p, prepared).unwrap().path;
        if done {
            let to = p.root.join("archive/done");
            std::fs::create_dir_all(&to).unwrap();
            std::fs::rename(&dir, to.join(dir.file_name().unwrap())).unwrap();
        }
        dir
    }

    /// A store of `n` done changes of main and one active change, the done ones
    /// vowed before one more commit, so git is asked about their point.
    fn store_with_done(n: usize) -> (TempDir, Project, PathBuf) {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &home, Mode::Home).unwrap();
        for i in 0..n {
            vow_change(&p, &home, &repo, &format!("D{i}"), &format!("f{i}.txt"), "strict", true);
        }
        std::fs::write(repo.join("a.txt"), "b").unwrap();
        git(&repo, &["add", "a.txt"]);
        git(&repo, &["commit", "-q", "-m", "next"]);
        vow_change(&p, &home, &repo, "Q", "q.txt", "strict", false);
        (tmp, p, repo)
    }

    /// The globs of a gate scope check over a store of `n` done changes, and
    /// the git processes it started.
    fn scope_spawns(n: usize) -> (Vec<String>, usize) {
        let (_tmp, p, repo) = store_with_done(n);
        let (living, _) = change::load_living(&p).unwrap();
        let all = proof::active(&p, &living).unwrap();
        let queue: Vec<&Active> = all.iter().filter(|c| c.loaded.proposal.branch == "main").collect();
        assert_eq!(queue.len(), 1, "one active change of main");
        let mb = git::run(&repo, &["merge-base", "HEAD", "main"]).unwrap();
        let before = git::spawns();
        let globs = scope_globs(&p, &repo, "main", &living, &queue, &mb).unwrap();
        (globs, git::spawns() - before)
    }

    /// The scope of a gate check counts the done changes once per point, so the
    /// check over 8 of them starts as many git processes as the check over 2.
    #[test]
    fn scale_listing_two_and_eight_done_changes_start_the_same_git_processes() {
        let (two, spawns_two) = scope_spawns(2);
        let (eight, spawns_eight) = scope_spawns(8);
        assert_eq!(two.len(), eight.len(), "the same globs over 2 and over 8 done changes: {two:?} against {eight:?}");
        assert_eq!(spawns_two, 1, "the 2 done changes share one point, so the scan asks git about it once: {spawns_two} processes");
        assert_eq!(spawns_eight, spawns_two, "8 done changes start {spawns_eight} git processes against {spawns_two} for 2");
    }

    /// The drift of every change of a run asks for the store's scope globs and
    /// the run loads the store once: the first drift of the run scans it, the
    /// next ones answer from what it read.
    /// Sets every file under `dir` to an mtime 10 s ago.
    fn settle(dir: &Path) {
        let at = std::time::SystemTime::now() - std::time::Duration::from_secs(10);
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                settle(&path);
            } else {
                std::fs::File::options().write(true).open(&path).unwrap().set_modified(at).unwrap();
            }
        }
    }

    #[test]
    fn scale_listing_a_moved_branch_tip_changes_the_scope_with_no_store_edit() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &home, Mode::Home).unwrap();
        let dir = vow_change(&p, &home, &repo, "F", "feat.txt", "strict", false);
        let proposal = std::fs::read_to_string(dir.join(PROPOSAL)).unwrap().replacen("branch: main", "branch: feat", 1);
        std::fs::write(dir.join(PROPOSAL), proposal).unwrap();
        settle(&p.root);
        let (living, _) = change::load_living(&p).unwrap();
        let mb = git::run(&repo, &["rev-parse", "HEAD~0"]).unwrap();
        let has = |globs: &[String]| globs.iter().any(|g| g == "feat.txt");
        assert!(!has(&store_globs(&p, &repo, "main", &living, &mb).unwrap()), "a branch with no tip stacks nothing");
        // A new tip that HEAD contains past the merge base: only git moved.
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "feat"]);
        git(&repo, &["branch", "feat"]);
        assert!(has(&store_globs(&p, &repo, "main", &living, &mb).unwrap()), "the stacked branch counts once its tip moves");
    }

    #[test]
    fn scale_listing_a_fresh_store_file_is_scanned_again() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &home, Mode::Home).unwrap();
        vow_change(&p, &home, &repo, "A", "a.txt", "relaxed", false);
        let (living, _) = change::load_living(&p).unwrap();
        let fresh = scanned(&p, &living).unwrap();
        let again = scanned(&p, &living).unwrap();
        assert!(!Rc::ptr_eq(&fresh, &again), "a scan of fresh files is not kept");
        settle(&p.root);
        let second = scanned(&p, &living).unwrap();
        let third = scanned(&p, &living).unwrap();
        assert!(Rc::ptr_eq(&second, &third), "a scan of settled files is kept");
    }

    #[test]
    fn scale_listing_drift_of_three_tasks_loads_the_changes_once() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &home, Mode::Home).unwrap();
        // One done change of main, whose point git is asked about, then three
        // active changes of main with a relaxed task each.
        vow_change(&p, &home, &repo, "Done", "d.txt", "strict", true);
        std::fs::write(repo.join("a.txt"), "b").unwrap();
        git(&repo, &["add", "a.txt"]);
        git(&repo, &["commit", "-q", "-m", "next"]);
        for i in 0..3 {
            vow_change(&p, &home, &repo, &format!("A{i}"), &format!("a{i}.txt"), "relaxed", false);
        }
        // A file outside every task, so the drift of each change asks the scope.
        std::fs::write(repo.join("stray.txt"), "x").unwrap();
        // Store files written within the racy window are scanned again, so age them.
        settle(&p.root);
        let (living, _) = change::load_living(&p).unwrap();
        let all = proof::active(&p, &living).unwrap();
        let config = crate::config::load(&p, &home).unwrap();
        // The base ref is a process-wide answer, so ask it before the count.
        let _ = git::base_ref(&repo, &config.base);
        let mut counts = Vec::new();
        let scans = SCANS.with(std::cell::Cell::get);
        for c in all.iter().filter(|c| c.loaded.tasks.iter().any(|t| t.mode == crate::tasks::Mode::Relaxed)) {
            let before = git::spawns();
            let items = drift::drift(&p, &repo, &config, c).unwrap();
            assert!(items.iter().any(|i| i.what.contains(drift::OUTSIDE)), "{} has no outside item: {items:?}", c.dir.id);
            counts.push(git::spawns() - before);
        }
        assert_eq!(counts.len(), 3, "three relaxed changes: {counts:?}");
        assert_eq!(SCANS.with(std::cell::Cell::get) - scans, 1, "the drift of three changes loads the store once");
        assert!(counts.iter().all(|&n| n == counts[0]), "each drift asks git the same, since HEAD may move between calls: {counts:?}");
    }
}
