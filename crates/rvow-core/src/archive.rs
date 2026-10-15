//! The end of a change: `rvow archive` merges its deltas into the living specs,
//! `rvow drop` and `rvow supersede` retire it unmerged, and `rvow trace` lists
//! the changes that touched a requirement.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

#[cfg(test)]
use crate::change::DELTAS;
use crate::change::{self, CapSpec, ChangeDir, LEDGER, Stage};
use crate::diag::Severity;
use crate::error::{Error, Result};
use crate::gate;
use crate::git;
use crate::id::{self, Id, Kind as IdKind, Ref};
use crate::ledger::{self, Kind};
use crate::lint::Finding;
use crate::proof;
use crate::record::{self, Entry};
use crate::spec::DeltaOp;
use crate::store::{Lock, Project};
use crate::vow::requirement_hash;

mod merge;
use merge::merge;

#[derive(Debug)]
pub struct Archived {
    pub id: Id,
    pub path: PathBuf,
    /// Each merged requirement with its op, in delta order.
    pub merged: Vec<(DeltaOp, String)>,
}

/// Today's local date as `YYYY-MM-DD`.
pub(crate) fn today() -> String {
    let secs = ledger::now_ms() / 1000;
    #[cfg(unix)]
    {
        let t = secs as libc::time_t;
        // SAFETY: localtime_r only writes the tm it is given, and an all-zero
        // tm is a valid value to overwrite.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        if !unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
            return format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday);
        }
    }
    // UTC from days since the epoch, by Howard Hinnant's civil_from_days
    // (https://howardhinnant.github.io/date_algorithms.html, public domain).
    let z = (secs / 86400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Moves a change into `stage` under a date prefix.
fn retire(project: &Project, dir: &ChangeDir, stage: Stage) -> Result<PathBuf> {
    let parent = project.root.join(stage.dir());
    std::fs::create_dir_all(&parent).map_err(|e| Error::io(&parent, e))?;
    let name = change::file_name(&dir.path)?;
    let dest = parent.join(format!("{}-{name}", today()));
    std::fs::rename(&dir.path, &dest).map_err(|e| Error::io(&dest, e))?;
    Ok(dest)
}

fn open(project: &Project, input: &str, verb: &str) -> Result<ChangeDir> {
    let c = change::resolve(project, input)?;
    if !matches!(c.stage, Stage::Backlog | Stage::Active) {
        return Err(Error::new(
            "stage",
            format!("{} is already in {}", c.id, c.stage.dir()),
            format!("only a backlog or active change can be {verb}"),
        ));
    }
    Ok(c)
}

fn need_reason(reason: &str) -> Result<()> {
    if reason.trim().is_empty() {
        return Err(Error::new("reason", "a reason is required", "say why with --reason \"...\""));
    }
    Ok(())
}

/// `rvow drop`: records the reason and moves the change to `archive/dropped/`.
pub fn drop_change(project: &Project, input: &str, reason: &str) -> Result<PathBuf> {
    need_reason(reason)?;
    let c = open(project, input, "dropped")?;
    let lock = project.lock()?;
    let mut data = Map::new();
    data.insert("reason".into(), reason.trim().into());
    ledger::append(&c.path.join(LEDGER), &lock, Kind::Drop, data)?;
    retire(project, &c, Stage::Dropped)
}

/// `rvow supersede`: links `old` and `by` both ways in their ledgers and moves
/// `old` to `archive/superseded/`.
pub fn supersede(project: &Project, old: &str, by: &str, reason: &str) -> Result<PathBuf> {
    need_reason(reason)?;
    let old = open(project, old, "superseded")?;
    let by = open(project, by, "named by --by")?;
    if old.id == by.id {
        return Err(Error::new("supersede", format!("{} cannot supersede itself", old.id), "name the change that replaces it with --by"));
    }
    let lock = project.lock()?;
    for (dir, key, other) in [(&old, "superseded_by", &by), (&by, "supersedes", &old)] {
        let mut data = Map::new();
        data.insert(key.into(), other.id.to_string().into());
        data.insert("reason".into(), reason.trim().into());
        ledger::append(&dir.path.join(LEDGER), &lock, Kind::Supersede, data)?;
    }
    retire(project, &old, Stage::Superseded)
}

fn error_keys(findings: &[Finding]) -> BTreeSet<String> {
    findings.iter().filter(|(_, d)| d.severity() == Severity::Error).map(|(p, d)| format!("{p}: {}", d.message)).collect()
}

/// `rvow archive`: checks the change with the strict gate, merges its deltas
/// into the living specs by id, records the new hashes and moves the change to
/// `archive/done/`. Nothing is written when a check refuses.
pub fn archive(project: &Project, home: &Path, cwd: &Path, input: &str) -> Result<Archived> {
    let dir = change::resolve(project, input)?;
    if dir.stage != Stage::Active {
        return Err(Error::new(
            "stage",
            format!("{} is in {}, not in active", dir.id, dir.stage.name()),
            "only an accepted change can be archived",
        ));
    }
    let lock = project.lock()?;
    let texts = change::living_texts(project)?;
    let (living, findings) = change::living_from(&texts);
    let changes = proof::active(project, &living)?;
    let c = changes.iter().find(|a| a.dir.id == dir.id).ok_or_else(|| Error::internal(format!("{} is not listed as active", dir.id)))?;

    let (all, _) = change::list(project)?;
    let waiting: Vec<&str> = c
        .loaded
        .proposal
        .after
        .iter()
        .filter(|a| !all.iter().any(|d| d.stage == Stage::Done && d.id.to_string() == **a))
        .map(String::as_str)
        .collect();
    if !waiting.is_empty() {
        return Err(Error::new(
            "after",
            format!("{} comes after {}, which is not archived", dir.id, waiting.join(", ")),
            "archive those changes first",
        ));
    }

    let moved = c.moved_bases(&living);
    if !moved.is_empty() {
        let ids: Vec<String> = moved.into_iter().map(|(id, _)| id).collect();
        return Err(Error::new(
            "conflict",
            format!("the living text of {} changed since {} recorded its base", ids.join(", "), dir.id),
            "update the delta to the new living text, record it with rvow amend --reason and ask the user to confirm",
        ));
    }

    let cwd = &c.branch().and_then(|b| git::top_for_branch(cwd, b).ok().flatten()).unwrap_or_else(|| cwd.to_path_buf());
    let r = gate::change_gate_echo(project, home, cwd, c, &mut std::io::stderr())?;
    if !r.passed() {
        return Err(Error::new(
            "gate",
            format!("the strict gate fails for {}:\n  {}", dir.id, r.failures.join("\n  ")),
            "fix each failure, then run rvow archive again",
        ));
    }

    let (new_caps, merged) = merge(&texts, &living, &dir, &c.loaded)?;
    let new_texts: Vec<(String, String)> = texts
        .iter()
        .filter(|(cap, _)| !new_caps.contains_key(cap))
        .cloned()
        .chain(new_caps.iter().filter_map(|(cap, t)| Some((cap.clone(), t.clone()?))))
        .collect();
    let (new_living, new_findings) = change::living_from(&new_texts);
    let before = error_keys(&findings);
    let fresh: Vec<String> = error_keys(&new_findings).into_iter().filter(|k| !before.contains(k)).collect();
    if !fresh.is_empty() {
        return Err(Error::new(
            "merge",
            format!("the merged specs would not validate:\n  {}", fresh.join("\n  ")),
            "fix the deltas so the merged specs validate, then run rvow archive again",
        ));
    }

    for (cap, text) in &new_caps {
        let cap_dir = project.root.join("specs").join(cap);
        match text {
            Some(t) => {
                std::fs::create_dir_all(&cap_dir).map_err(|e| Error::io(&cap_dir, e))?;
                let path = cap_dir.join("spec.md");
                std::fs::write(&path, t).map_err(|e| Error::io(&path, e))?;
            }
            None => std::fs::remove_dir_all(&cap_dir).map_err(|e| Error::io(&cap_dir, e))?,
        }
    }
    let hashes = write_record(project, &lock, &new_living, &merged)?;

    let mut by_op: Map<String, Value> = Map::new();
    for (op, id) in &merged {
        if let Value::Array(ids) = by_op.entry(op.word()).or_insert_with(|| json!([])) {
            ids.push(id.clone().into());
        }
    }
    let mut data = Map::new();
    data.insert("merged".into(), by_op.into());
    data.insert("record".into(), json!(hashes));
    // Without a checkout or a commit there is no HEAD to record; the gate then reads the vow base.
    if let Ok(top) = git::toplevel(cwd)
        && let Ok(head) = git::run(&top, &["rev-parse", "HEAD"])
    {
        data.insert("head".into(), head.into());
    }
    ledger::append(&dir.path.join(LEDGER), &lock, Kind::Archive, data)?;
    let path = retire(project, &dir, Stage::Done)?;
    Ok(Archived { id: dir.id, path, merged })
}

/// Updates spec-record.json for the merge and returns the new hash of each
/// merged requirement that is still living. An empty record is seeded with
/// every living requirement, so the gate's record check covers them from now.
fn write_record(project: &Project, lock: &Lock, living: &[CapSpec], merged: &[(DeltaOp, String)]) -> Result<BTreeMap<String, String>> {
    let hash_of: BTreeMap<String, String> =
        living.iter().flat_map(|(_, s)| s.requirements.iter()).map(|r| (r.id.to_string(), requirement_hash(r))).collect();
    let mut rec = record::load(project)?;
    if rec.is_empty() {
        for (id, hash) in &hash_of {
            rec.insert(id.clone(), Entry { hash: hash.clone(), legacy: false });
        }
    }
    let mut out = BTreeMap::new();
    for (op, id) in merged {
        if *op == DeltaOp::Remove {
            rec.remove(id);
            continue;
        }
        let hash = hash_of[id].clone();
        let legacy = !op.has_body() && rec.get(id).is_some_and(|e| e.legacy);
        rec.insert(id.clone(), Entry { hash: hash.clone(), legacy });
        out.insert(id.clone(), hash);
    }
    record::save(project, lock, &rec)?;
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Touch {
    pub change: Id,
    pub stage: Stage,
    /// The archive date from the folder name.
    pub date: Option<String>,
    /// The delta ops on the requirement, and `covers` when a task covers it.
    pub ops: Vec<&'static str>,
    pub title: String,
}

/// `rvow trace`: every change that names a requirement in a delta or covers it
/// with a task, oldest first by its first ledger event, unaccepted ones last.
pub fn trace(project: &Project, input: &str) -> Result<(Id, Vec<Touch>)> {
    let (dirs, _) = change::list(project)?;
    let (living, _) = change::load_living(project)?;
    let mut loaded = Vec::new();
    for d in dirs {
        let l = change::load(&d.path, &[])?;
        loaded.push((d, l));
    }
    let mut known: BTreeSet<Id> = BTreeSet::new();
    let mut note = |r: &Ref| {
        if let Ref::Id(id) = r
            && id.kind() == IdKind::Req
        {
            known.insert(id.clone());
        }
    };
    for (_, spec) in &living {
        spec.requirements.iter().for_each(|r| note(&r.id));
    }
    for (_, l) in &loaded {
        l.deltas.iter().flat_map(|(_, f)| f.deltas.iter()).for_each(|d| note(&d.req.id));
        l.tasks.iter().flat_map(|t| t.covers.iter()).for_each(&mut note);
    }
    let input = if input.contains('-') { input.to_string() } else { format!("REQ-{input}") };
    let id = id::resolve(&input, &known)
        .map_err(|e| Error::new("trace", format!("{input}: {e}"), "give a REQ id that a spec or a change names"))?;
    let want = Ref::Id(id.clone());
    let mut out = Vec::new();
    for (d, l) in loaded {
        let mut ops: Vec<&'static str> =
            l.deltas.iter().flat_map(|(_, f)| f.deltas.iter()).filter(|x| x.req.id == want).map(|x| x.op.word()).collect();
        if l.tasks.iter().any(|t| t.covers.contains(&want)) {
            ops.push("covers");
        }
        if ops.is_empty() {
            continue;
        }
        let (events, _) = ledger::read(&d.path.join(LEDGER))?;
        let first = events.first().map_or(u64::MAX, |e| e.time);
        let name = d.path.file_name().unwrap_or_default().to_string_lossy();
        let date = (!matches!(d.stage, Stage::Backlog | Stage::Active)).then(|| name.get(..10).map(String::from)).flatten();
        out.push((first, Touch { change: d.id, stage: d.stage, date, ops, title: l.proposal.title }));
    }
    out.sort_by(|a, b| (a.0, &a.1.change).cmp(&(b.0, &b.1.change)));
    Ok((id, out.into_iter().map(|(_, t)| t).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amend;
    use crate::git::testrepo::*;
    use crate::proof::prove;
    use crate::store::{Mode, init};
    use crate::vow;

    const CORE: &str = "\
# Core

## REQ-k3f9m2 One
rvow SHALL keep one.

### Scenario: s
- WHEN a
- THEN b

## REQ-q8w2c7 Two

rvow SHALL keep two.
### Scenario: s
- WHEN a
- THEN b

## REQ-h4t6v9 Three
rvow SHALL keep three.
### Scenario: s
- WHEN a
- THEN b
";

    fn setup() -> (TempDir, Project, PathBuf, PathBuf) {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &home, Mode::Home).unwrap();
        for (cap, text) in [("core", CORE), ("gone", "# Gone\n\n## REQ-7xq2a5 Last\nrvow SHALL go.\n### Scenario: s\n- WHEN a\n- THEN b\n")]
        {
            let dir = p.root.join("specs").join(cap);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("spec.md"), text).unwrap();
        }
        (tmp, p, repo, home)
    }

    /// Accepts a change with these delta files and one task that checks
    /// `file`, and returns its id and task id.
    fn accept(p: &Project, home: &Path, repo: &Path, deltas: &[(&str, &str)], covers: &str, file: &str) -> (String, String) {
        let c = change::new(p, file, "main", false).unwrap();
        let covers = if covers.is_empty() { String::new() } else { format!("  covers: {covers}\n") };
        std::fs::write(
            c.path.join("tasks.md"),
            format!("- TSK-new1 T\n{covers}  verify: grep -q ok {file} && echo 'test result: ok. 1 passed; 0 failed;'\n  files: {file}\n"),
        )
        .unwrap();
        for (cap, text) in deltas {
            std::fs::write(c.path.join(DELTAS).join(format!("{cap}.md")), text).unwrap();
        }
        let prepared = vow::prepare(p, home, c, repo, &mut std::io::sink()).unwrap();
        let vowed = vow::commit(p, prepared).unwrap();
        let task = vowed.minted.iter().find(|(f, _)| f == "TSK-new1").unwrap();
        (vowed.id.to_string(), task.1.to_string())
    }

    fn prove_ok(p: &Project, home: &Path, repo: &Path, task: &str, file: &str) {
        std::fs::write(repo.join(file), "ok").unwrap();
        assert!(prove(p, home, repo, task, &mut std::io::sink()).unwrap().proved);
    }

    fn spec(p: &Project, cap: &str) -> String {
        std::fs::read_to_string(p.root.join("specs").join(cap).join("spec.md")).unwrap()
    }

    #[test]
    fn archive_merges_every_op_in_place() {
        let (_tmp, p, repo, home) = setup();
        let core = "## ADD REQ-new1 Four\nrvow SHALL keep four.\n### Scenario: s\n- WHEN a\n- THEN b\n\n## DEPRECATE REQ-q8w2c7\n";
        let elsewhere = "## CHANGE REQ-k3f9m2 One again\nrvow SHALL keep one twice.\n### Scenario: s\n- WHEN a\n- THEN c\n";
        let (id, task) = accept(
            &p,
            &home,
            &repo,
            &[("core", core), ("elsewhere", elsewhere), ("other", "## MOVE REQ-h4t6v9\n"), ("gone", "## REMOVE REQ-7xq2a5\n")],
            "REQ-new1, REQ-k3f9m2",
            "out.txt",
        );
        let err = archive(&p, &home, &repo, &id).unwrap_err();
        assert_eq!(err.code, "gate");
        assert!(err.message.contains("is missing"), "{err}");

        prove_ok(&p, &home, &repo, &task, "out.txt");
        let a = archive(&p, &home, &repo, &id).unwrap();
        let added = a.merged[0].1.clone();
        assert_eq!(a.merged.len(), 5);
        assert_eq!(
            spec(&p, "core"),
            format!(
                "# Core\n\n## REQ-k3f9m2 One again\nrvow SHALL keep one twice.\n### Scenario: s\n- WHEN a\n- THEN c\n\n## REQ-q8w2c7 Two\nstatus: deprecated\n\nrvow SHALL keep two.\n### Scenario: s\n- WHEN a\n- THEN b\n\n## {added} Four\nrvow SHALL keep four.\n### Scenario: s\n- WHEN a\n- THEN b\n"
            )
        );
        assert_eq!(spec(&p, "other"), "# other\n\n## REQ-h4t6v9 Three\nrvow SHALL keep three.\n### Scenario: s\n- WHEN a\n- THEN b\n");
        assert!(!p.root.join("specs/gone").exists());
        assert!(!p.root.join("specs/elsewhere").exists());

        let rec = record::load(&p).unwrap();
        let (living, _) = change::load_living(&p).unwrap();
        assert_eq!(rec.len(), 4);
        for r in living.iter().flat_map(|(_, s)| s.requirements.iter()) {
            assert_eq!(rec[&r.id.to_string()].hash, requirement_hash(r));
        }
        let name = a.path.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with(&format!("{}-{id}", today())), "{name}");
        assert!(a.path.starts_with(p.root.join("archive/done")));
        let (events, _) = ledger::read(&a.path.join(LEDGER)).unwrap();
        let last = events.last().unwrap();
        assert_eq!(last.kind, Kind::Archive);
        assert_eq!(last.data["merged"]["REMOVE"], json!(["REQ-7xq2a5"]));
        assert_eq!(last.data["record"].as_object().unwrap().len(), 4);

        let r = gate::gate(&p, &home, &repo, gate::Options::default(), &mut std::io::sink()).unwrap();
        assert!(r.passed(), "{r:?}");
        assert_eq!(archive(&p, &home, &repo, &id).unwrap_err().code, "stage");
    }

    #[test]
    fn a_second_change_on_a_requirement_conflicts_until_rebased() {
        let (_tmp, p, repo, home) = setup();
        let delta = |text: &str| format!("## CHANGE REQ-k3f9m2 One\nrvow SHALL keep {text}.\n### Scenario: s\n- WHEN a\n- THEN b\n");
        let (first, t1) = accept(&p, &home, &repo, &[("core", &delta("one, first"))], "REQ-k3f9m2", "a1.txt");
        let (second, t2) = accept(&p, &home, &repo, &[("core", &delta("one, second"))], "REQ-k3f9m2", "a2.txt");
        prove_ok(&p, &home, &repo, &t1, "a1.txt");
        prove_ok(&p, &home, &repo, &t2, "a2.txt");
        archive(&p, &home, &repo, &first).unwrap();

        let err = archive(&p, &home, &repo, &second).unwrap_err();
        assert_eq!(err.code, "conflict");
        assert!(err.message.contains("REQ-k3f9m2"), "{err}");
        assert!(err.fix.contains("new living text"), "{err}");

        let (dirs, _) = change::list(&p).unwrap();
        let dir = &dirs.iter().find(|d| d.id.to_string() == second).unwrap().path;
        std::fs::write(dir.join("specs/core.md"), delta("one, first and second")).unwrap();
        let a = amend::amend(&p, &home, &repo, None, "rebase on the first", None).unwrap();
        assert!(a[0].items.iter().any(|i| i.what.contains("living REQ-k3f9m2 changed")), "{:?}", a[0].items);
        amend::confirm(&p, &[a[0].id.to_string()], amend::Decision::Accept, None).unwrap();
        prove_ok(&p, &home, &repo, &t2, "a2.txt");
        archive(&p, &home, &repo, &second).unwrap();
        assert!(spec(&p, "core").contains("one, first and second"));
    }

    #[test]
    fn after_waits_for_the_named_change() {
        let (_tmp, p, repo, home) = setup();
        let (first, t1) = accept(&p, &home, &repo, &[], "", "b1.txt");
        let c = change::new(&p, "later", "main", true).unwrap();
        let prop = c.path.join(change::PROPOSAL);
        let text = std::fs::read_to_string(&prop).unwrap();
        std::fs::write(&prop, text.replace("branch: main\n", &format!("branch: main\nafter: {first}\n"))).unwrap();
        std::fs::write(
            c.path.join("tasks.md"),
            "- TSK-new1 T\n  verify: grep -q ok b2.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: b2.txt\n",
        )
        .unwrap();
        let vowed = vow::commit(&p, vow::prepare(&p, &home, c, &repo, &mut std::io::sink()).unwrap()).unwrap();
        let t2 = vowed.minted[0].1.to_string();
        prove_ok(&p, &home, &repo, &t1, "b1.txt");
        prove_ok(&p, &home, &repo, &t2, "b2.txt");

        let err = archive(&p, &home, &repo, &vowed.id.to_string()).unwrap_err();
        assert_eq!(err.code, "after");
        assert!(err.message.contains(&first), "{err}");
        archive(&p, &home, &repo, &first).unwrap();
        archive(&p, &home, &repo, &vowed.id.to_string()).unwrap();
    }

    #[test]
    fn drop_and_supersede_move_and_record() {
        let (_tmp, p, repo, home) = setup();
        let (old, _) = accept(&p, &home, &repo, &[], "", "c1.txt");
        let new = change::new(&p, "New", "main", true).unwrap();
        let backlog = change::new(&p, "Idle", "main", true).unwrap();

        assert_eq!(drop_change(&p, &backlog.id.to_string(), " ").unwrap_err().code, "reason");
        let dropped = drop_change(&p, &backlog.id.to_string(), "not needed").unwrap();
        assert!(dropped.starts_with(p.root.join("archive/dropped")));
        let (events, _) = ledger::read(&dropped.join(LEDGER)).unwrap();
        assert_eq!((events[0].kind, &events[0].data["reason"]), (Kind::Drop, &json!("not needed")));
        let err = drop_change(&p, &backlog.id.to_string(), "again").unwrap_err();
        assert_eq!(err.code, "stage");

        let err = supersede(&p, &old, &old, "x").unwrap_err();
        assert_eq!(err.code, "supersede");
        let err = supersede(&p, &old, &backlog.id.to_string(), "x").unwrap_err();
        assert_eq!(err.code, "stage");
        let gone = supersede(&p, &old, &new.id.to_string(), "a better plan").unwrap();
        assert!(gone.starts_with(p.root.join("archive/superseded")));
        let (events, _) = ledger::read(&gone.join(LEDGER)).unwrap();
        let last = events.last().unwrap();
        assert_eq!(last.kind, Kind::Supersede);
        assert_eq!(last.data["superseded_by"], json!(new.id.to_string()));
        let (events, _) = ledger::read(&new.path.join(LEDGER)).unwrap();
        assert_eq!(events[0].data["supersedes"], json!(old));

        let r = gate::gate(&p, &home, &repo, gate::Options::default(), &mut std::io::sink()).unwrap();
        assert!(r.passed(), "{r:?}");
    }

    #[test]
    fn trace_lists_every_change_oldest_first() {
        let (_tmp, p, repo, home) = setup();
        let change = "## CHANGE REQ-k3f9m2 One\nrvow SHALL keep one more.\n### Scenario: s\n- WHEN a\n- THEN b\n";
        let (first, t1) = accept(&p, &home, &repo, &[("core", change)], "REQ-k3f9m2", "d1.txt");
        let (second, _) = accept(&p, &home, &repo, &[("core", "## REMOVE REQ-k3f9m2\n")], "", "d2.txt");
        let idle = change::new(&p, "Idle", "main", false).unwrap();
        std::fs::write(idle.path.join("specs/core.md"), "## DEPRECATE REQ-k3f9m2\n").unwrap();
        prove_ok(&p, &home, &repo, &t1, "d1.txt");
        archive(&p, &home, &repo, &first).unwrap();

        let (id, touches) = trace(&p, "k3f9").unwrap();
        assert_eq!(id.to_string(), "REQ-k3f9m2");
        let got: Vec<(String, Stage, Option<String>, Vec<&str>)> =
            touches.into_iter().map(|t| (t.change.to_string(), t.stage, t.date, t.ops)).collect();
        assert_eq!(
            got,
            [
                (first, Stage::Done, Some(today()), vec!["CHANGE", "covers"]),
                (second, Stage::Active, None, vec!["REMOVE"]),
                (idle.id.to_string(), Stage::Backlog, None, vec!["DEPRECATE"]),
            ]
        );
        assert_eq!(trace(&p, "zzzzzz").unwrap_err().code, "trace");
    }

    #[test]
    fn a_merge_that_would_not_validate_writes_nothing() {
        let (_tmp, p, repo, home) = setup();
        let (id, task) = accept(&p, &home, &repo, &[], "", "e1.txt");
        let (dirs, _) = change::list(&p).unwrap();
        let dir = &dirs.iter().find(|d| d.id.to_string() == id).unwrap().path;
        std::fs::write(dir.join("specs/core.md"), "## ADD REQ-new2 Late\nrvow SHALL be late.\n### Scenario: s\n- WHEN a\n- THEN b\n")
            .unwrap();
        let tasks = dir.join("tasks.md");
        let text = std::fs::read_to_string(&tasks).unwrap();
        std::fs::write(&tasks, text.replacen("  verify", "  covers: REQ-new2\n  verify", 1)).unwrap();
        let a = amend::amend(&p, &home, &repo, None, "one more", None).unwrap();
        amend::confirm(&p, &[a[0].id.to_string()], amend::Decision::Accept, None).unwrap();
        let specs = std::fs::read_to_string(dir.join("specs/core.md")).unwrap();
        assert!(!specs.contains("REQ-new2"), "amend mints before the snapshot: {specs}");
        prove_ok(&p, &home, &repo, &task, "e1.txt");

        let archived = archive(&p, &home, &repo, &id).unwrap();
        assert_eq!(archived.merged.len(), 1);
        assert!(spec(&p, "core").contains("Late"), "{}", spec(&p, "core"));
    }

    #[test]
    fn today_is_a_date() {
        let d = today();
        assert_eq!(d.len(), 10);
        assert!(d.starts_with("20") && d.as_bytes()[4] == b'-' && d.as_bytes()[7] == b'-');
    }
}
