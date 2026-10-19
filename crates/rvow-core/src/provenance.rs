//! Blob provenance: what each tool call changed, so a stop is held only for
//! file content the session produced.
//!
//! Around each tool call that may write, the hooks snapshot the work tree with
//! a private git index and object store, then record each changed path with
//! its new git blob id (or "deleted"). The private store lives under
//! `<project>/provenance/` and lists the repository's own object store as an
//! alternate, so nothing is ever written into the user's `.git`.
//!
//! The Stop hook decides ownership from these records: a session owns a file
//! only when its latest record for that path matches the file's current blob,
//! and no call of another session or a subagent that overlapped that call in
//! time recorded the same (path, blob). A session with no after-call record
//! (an older mod that sent only pre-tool-use) keeps today's text record.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::git;
use crate::ledger::now_ms;
use crate::store::Project;

/// The folder that holds the private store per worktree under the project root.
const DIR: &str = "provenance";
const RECORDS: &str = "records.jsonl";

/// The blob marker of a path a call deleted.
pub const DELETED: &str = ":deleted";

/// A before record with no after report past this age belongs to a call that
/// was cancelled or killed; it no longer holds back the object prune.
const STALE_MS: u64 = 6 * 60 * 60 * 1000;
/// After records are kept as long as the text record of `session_files`.
const KEEP_MS: u64 = 7 * 24 * 60 * 60 * 1000;
/// The private object store is deleted past this size.
const MAX_OBJECTS: u64 = 64 * 1024 * 1024;

/// Exact git config overrides that make `git add` hash the work tree as it is:
/// no racy-time reuse, no split index, no autocrlf, and no LFS smudging.
const SNAPSHOT_CONFIG: [&str; 14] = [
    "-c",
    "core.ignorestat=false",
    "-c",
    "core.splitIndex=false",
    "-c",
    "core.autocrlf=false",
    "-c",
    "filter.lfs.clean=",
    "-c",
    "filter.lfs.smudge=",
    "-c",
    "filter.lfs.process=",
    "-c",
    "filter.lfs.required=false",
];

/// A pre-tool-use record: the before tree and the start time of one call.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Start {
    session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_id: Option<String>,
    tool_use_id: String,
    before: String,
    start: u64,
}

/// The marker that the session sent post-tool-use (an "after-call record").
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Post {
    session: String,
    tool_use_id: String,
    end: u64,
}

/// One after-call record per changed path.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct After {
    session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_id: Option<String>,
    tool_use_id: String,
    path: String,
    /// The new git blob id, or `:deleted`.
    blob: String,
    start: u64,
    end: u64,
}

/// The tree a session's latest recorded call left, which a light call (one
/// with no before snapshot) is compared with.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Last {
    session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_id: Option<String>,
    tree: String,
    end: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Record {
    Start(Start),
    Post(Post),
    After(After),
    Last(Last),
}

/// The `before` of a light call's Start record: compare with the session's last tree.
const LIGHT: &str = ":last";

/// The call id of an After record that the catch-up wrote for a change no call recorded.
const UNRECORDED: &str = ":unrecorded";

/// The private store of one worktree and the object store it reads through.
struct Store {
    /// `<project>/provenance/<key>/`, keyed by the worktree's git dir.
    dir: PathBuf,
    /// The repository's shared objects; a linked worktree has none of its own.
    alternate: PathBuf,
}

/// Resolves and creates the private store of the worktree at `top`.
fn locate(project: &Project, top: &Path) -> Result<Store> {
    let out = git::run(top, &["rev-parse", "--absolute-git-dir", "--path-format=absolute", "--git-common-dir"])?;
    let mut lines = out.lines();
    let (Some(git_dir), Some(common)) = (lines.next(), lines.next()) else {
        return Err(Error::new(
            "git",
            format!("git rev-parse printed no git dir for {}", top.display()),
            "run the command in a git repository",
        ));
    };
    let root = project.root.join(DIR);
    let dir = root.join(&crate::vow::sha256_hex(git_dir.as_bytes())[..16]);
    let objects = dir.join("objects");
    std::fs::create_dir_all(&objects).map_err(|e| Error::io(&objects, e))?;
    // In repo mode the store sits inside the work tree, and `git add -A` would
    // hash the store into its own snapshot.
    let hide = root.join(".gitignore");
    if !hide.is_file() {
        std::fs::write(&hide, "*\n").map_err(|e| Error::io(&hide, e))?;
    }
    Ok(Store { dir, alternate: PathBuf::from(common).join("objects") })
}

/// Runs `git` in `top` with the private index and object store set, the
/// repository's objects as an alternate, and the harness's git env removed.
fn git_private(top: &Path, store: &Store, args: &[&str]) -> Result<String> {
    let out = Command::new(git::bin())
        .arg("-C")
        .arg(top)
        .args(SNAPSHOT_CONFIG)
        .args(args)
        .env("GIT_INDEX_FILE", store.dir.join("index"))
        .env("GIT_OBJECT_DIRECTORY", store.dir.join("objects"))
        .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", &store.alternate)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_CONFIG_GLOBAL")
        .env_remove("GIT_CONFIG_NOSYSTEM")
        .output()
        .map_err(|e| Error::new("git", format!("cannot run git: {e}"), "install git"))?;
    if !out.status.success() {
        return Err(Error::new(
            "git",
            format!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()),
            "run the command in a git repository",
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The tree the work tree would commit, hashed through the private store. The
/// caller holds the project lock, since one private index cannot be shared.
fn tree(top: &Path, store: &Store) -> Result<String> {
    git_private(top, store, &["add", "-A"])?;
    Ok(git_private(top, store, &["write-tree"])?.trim_end().to_string())
}

fn is_zero(sha: &str) -> bool {
    !sha.is_empty() && sha.bytes().all(|b| b == b'0')
}

/// The changed paths between `before` and `after`, each with its new blob id
/// or `DELETED`. `-z` keeps paths unquoted and unmodified.
fn changed(top: &Path, store: &Store, before: &str, after: &str) -> Result<Vec<(String, String)>> {
    let out = git_private(top, store, &["diff-tree", "-r", "-z", "--no-renames", before, after])?;
    let mut res = Vec::new();
    let mut fields = out.split('\0');
    while let Some(header) = fields.next() {
        if header.is_empty() {
            continue;
        }
        let Some(path) = fields.next() else {
            break;
        };
        if path.is_empty() {
            continue;
        }
        // ":<oldmode> <newmode> <oldblob> <newblob> <status>"
        let new = header.split(' ').nth(3).unwrap_or("");
        res.push((path.to_string(), if is_zero(new) { DELETED.to_string() } else { new.to_string() }));
    }
    Ok(res)
}

/// The blob id of each path in `tree`. Submodule content is not seen.
fn blobs(top: &Path, store: &Store, tree: &str) -> Result<BTreeMap<String, String>> {
    let out = git_private(top, store, &["ls-tree", "-r", "-z", tree])?;
    let mut map = BTreeMap::new();
    for entry in out.split('\0') {
        // "<mode> <type> <sha>\t<path>"
        let Some((meta, path)) = entry.split_once('\t') else {
            continue;
        };
        let mut parts = meta.split(' ');
        let ty = parts.nth(1).unwrap_or("");
        let sha = parts.next().unwrap_or("");
        if ty == "blob" && !path.is_empty() {
            map.insert(path.to_string(), sha.to_string());
        }
    }
    Ok(map)
}

fn load(dir: &Path) -> Vec<Record> {
    let Ok(text) = std::fs::read_to_string(dir.join(RECORDS)) else {
        return Vec::new();
    };
    text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
}

fn append(dir: &Path, records: &[Record]) -> Result<()> {
    append_to(&dir.join(RECORDS), records)
}

/// Replaces the records file in one rename, so a reader never sees half of it.
fn rewrite(dir: &Path, records: &[Record]) -> Result<()> {
    let tmp = dir.join(format!("{RECORDS}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    append_to(&tmp, records)?;
    let path = dir.join(RECORDS);
    std::fs::rename(&tmp, &path).map_err(|e| Error::io(&path, e))
}

fn append_to(path: &Path, records: &[Record]) -> Result<()> {
    let mut out = std::fs::OpenOptions::new().create(true).append(true).open(path).map_err(|e| Error::io(path, e))?;
    for record in records {
        let text = serde_json::to_string(record).map_err(|e| Error::internal(format!("provenance record does not serialize: {e}")))?;
        writeln!(out, "{text}").map_err(|e| Error::io(path, e))?;
    }
    Ok(())
}

fn is_call(s: &Start, session: &str, agent_id: Option<&str>, tool_use_id: &str) -> bool {
    s.session == session && s.agent_id.as_deref() == agent_id && s.tool_use_id == tool_use_id
}

/// The pre-tool-use handler: snapshots the work tree and stores the before
/// tree and start time of the call. Fails silently, like every hook.
pub(crate) fn before_call(project: &Project, top: &Path, session: &str, agent_id: Option<&str>, tool_use_id: &str) {
    let Ok(store) = locate(project, top) else {
        return;
    };
    let Ok(_lock) = project.lock() else {
        return;
    };
    let Ok(before) = tree(top, &store) else {
        return;
    };
    let _ = catch_up(top, &store, session, agent_id, &before);
    let start = Start {
        session: session.to_string(),
        agent_id: agent_id.map(String::from),
        tool_use_id: tool_use_id.to_string(),
        before,
        start: now_ms(),
    };
    let _ = append(&store.dir, &[Record::Start(start)]);
}

/// Records each path that differs between the session's last recorded tree and
/// `now`, a change that no call of the session recorded, such as one from a
/// background job, and makes `now` the session's last tree. A write that a
/// call of another session or agent recorded in the window is not the session's.
fn catch_up(top: &Path, store: &Store, session: &str, agent_id: Option<&str>, now: &str) -> Result<()> {
    let mut records = load(&store.dir);
    let mine = |s: &str, a: &Option<String>| s == session && a.as_deref() == agent_id;
    let Some(last) = records.iter().rev().find_map(|r| match r {
        Record::Last(l) if mine(&l.session, &l.agent_id) => Some(l.clone()),
        _ => None,
    }) else {
        return Ok(());
    };
    if last.tree == now {
        return Ok(());
    }
    let theirs = recorded_by_others(&records, session, agent_id, last.end);
    let changes = changed(top, store, &last.tree, now)?;
    let end = now_ms();
    records.retain(|r| !matches!(r, Record::Last(l) if mine(&l.session, &l.agent_id)));
    let changes = changes.into_iter().filter(|c| !theirs.contains(c));
    records.extend(changes.map(|(path, blob)| {
        Record::After(After {
            session: session.to_string(),
            agent_id: agent_id.map(String::from),
            tool_use_id: UNRECORDED.to_string(),
            path,
            blob,
            start: last.end,
            end,
        })
    }));
    records.push(Record::Last(Last { session: session.to_string(), agent_id: agent_id.map(String::from), tree: now.to_string(), end }));
    rewrite(&store.dir, &records)
}

/// The Stop hook's catch-up: records the changes since the session's last
/// recorded tree. Fails silently, like every hook.
/// The (path, blob) pairs that a call of another session or agent recorded
/// after `since`. An unrecorded change is no call, so it never counts.
fn recorded_by_others(records: &[Record], session: &str, agent_id: Option<&str>, since: u64) -> BTreeSet<(String, String)> {
    records
        .iter()
        .filter_map(|r| match r {
            Record::After(a)
                if !(a.session == session && a.agent_id.as_deref() == agent_id) && a.tool_use_id != UNRECORDED && a.end >= since =>
            {
                Some((a.path.clone(), a.blob.clone()))
            }
            _ => None,
        })
        .collect()
}

pub(crate) fn catch_up_at_stop(project: &Project, top: &Path, session: &str, agent_id: Option<&str>) {
    let Ok(store) = locate(project, top) else {
        return;
    };
    let Ok(_lock) = project.lock() else {
        return;
    };
    if let Ok(now) = tree(top, &store) {
        let _ = catch_up(top, &store, session, agent_id, &now);
    }
}

/// The pre-tool-use handler of a tool that may write but is neither an editor
/// tool nor a shell: it records the call with no snapshot, so post-tool-use
/// compares one snapshot with the session's last recorded tree. With no last
/// tree yet, it snapshots like an editor call, so the first call is seen too.
pub(crate) fn before_call_light(project: &Project, top: &Path, session: &str, agent_id: Option<&str>, tool_use_id: &str) {
    let Ok(store) = locate(project, top) else {
        return;
    };
    let Ok(_lock) = project.lock() else {
        return;
    };
    let has_last =
        load(&store.dir).iter().any(|r| matches!(r, Record::Last(l) if l.session == session && l.agent_id.as_deref() == agent_id));
    let before = if has_last {
        LIGHT.to_string()
    } else {
        let Ok(tree) = tree(top, &store) else {
            return;
        };
        tree
    };
    let start = Start {
        session: session.to_string(),
        agent_id: agent_id.map(String::from),
        tool_use_id: tool_use_id.to_string(),
        before,
        start: now_ms(),
    };
    let _ = append(&store.dir, &[Record::Start(start)]);
}

/// The post-tool-use handler: with a before record for the call, snapshots the
/// tree again and records a Post marker plus one After record per changed
/// path. A call with no before record (a read) takes no snapshot at all.
pub(crate) fn after_call(project: &Project, top: &Path, session: &str, agent_id: Option<&str>, tool_use_id: &str) {
    let Ok(store) = locate(project, top) else {
        return;
    };
    let Ok(_lock) = project.lock() else {
        return;
    };
    let mut records = load(&store.dir);
    let Some(start) = records.iter().rev().find_map(|r| match r {
        Record::Start(s) if is_call(s, session, agent_id, tool_use_id) => Some(s.clone()),
        _ => None,
    }) else {
        return;
    };
    let mine = |session_: &str, agent: &Option<String>| session_ == session && agent.as_deref() == agent_id;
    // A light call has no before snapshot: it starts from the session's last tree,
    // so its window opens there and a write that another call recorded in it is not its own.
    let (before, from, theirs) = if start.before == LIGHT {
        let last = records.iter().rev().find_map(|r| match r {
            Record::Last(l) if mine(&l.session, &l.agent_id) => Some(l.clone()),
            _ => None,
        });
        let from = last.as_ref().map_or(start.start, |l| l.end.min(start.start));
        (last.map(|l| l.tree), from, recorded_by_others(&records, session, agent_id, from))
    } else {
        (Some(start.before.clone()), start.start, BTreeSet::new())
    };
    let after = tree(top, &store).ok();
    if let (Some(before), Some(after)) = (&before, &after)
        && let Ok(changes) = changed(top, &store, before, after)
    {
        let changes = changes.into_iter().filter(|c| !theirs.contains(c));
        let end = now_ms();
        records.push(Record::Post(Post { session: session.to_string(), tool_use_id: tool_use_id.to_string(), end }));
        records.extend(changes.map(|(path, blob)| {
            Record::After(After {
                session: session.to_string(),
                agent_id: agent_id.map(String::from),
                tool_use_id: tool_use_id.to_string(),
                path,
                blob,
                start: from,
                end,
            })
        }));
    }
    let now = now_ms();
    records.retain(|r| match r {
        Record::Start(s) => !is_call(s, session, agent_id, tool_use_id) && now.saturating_sub(s.start) < STALE_MS,
        Record::Post(p) => now.saturating_sub(p.end) < KEEP_MS,
        Record::After(a) => now.saturating_sub(a.end) < KEEP_MS,
        Record::Last(l) => !mine(&l.session, &l.agent_id) && now.saturating_sub(l.end) < KEEP_MS,
    });
    if let Some(after) = after {
        records.push(Record::Last(Last { session: session.to_string(), agent_id: agent_id.map(String::from), tree: after, end: now }));
    }
    let _ = rewrite(&store.dir, &records);
    // A live call of another session still needs its before tree. A pruned
    // last tree is gone, so the next call of each session snapshots afresh.
    if !records.iter().any(|r| matches!(r, Record::Start(_))) && prune_objects(&store.dir) {
        records.retain(|r| !matches!(r, Record::Last(_)));
        let _ = rewrite(&store.dir, &records);
    }
}

/// The private object store grows with every distinct blob version hashed.
/// The records keep only blob ids, so past a bound the whole objects folder is
/// deleted and a later snapshot rebuilds what it needs.
fn prune_objects(dir: &Path) -> bool {
    let objects = dir.join("objects");
    if dir_size(&objects) <= MAX_OBJECTS {
        return false;
    }
    let _ = std::fs::remove_dir_all(&objects);
    let _ = std::fs::remove_file(dir.join("index"));
    true
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                total += dir_size(&p);
            } else if let Ok(m) = e.metadata() {
                total += m.len();
            }
        }
    }
    total
}

/// Two time ranges overlap when one starts before the other ends. A call with
/// `end == start` overlaps nothing.
fn overlaps(a_start: u64, a_end: u64, b_start: u64, b_end: u64) -> bool {
    a_start < b_end && b_start < a_end
}

/// The paths `session` owns right now, or `None` when the session has no
/// after-call record at all, so the caller keeps today's text record.
///
/// The session owns a path when its latest call on that path produced the
/// file's current blob (or deleted a file that is still gone), and no call of
/// another session or of a subagent that overlapped that call recorded the
/// same (path, blob). A subagent's own records never count for the session,
/// and a change no call recorded never takes a path from a call that did.
pub(crate) fn owned(project: &Project, top: &Path, session: &str) -> Result<Option<BTreeSet<String>>> {
    let store = locate(project, top)?;
    let _lock = project.lock()?;
    let records = load(&store.dir);
    let had_after = records.iter().any(|r| match r {
        Record::Post(p) => p.session == session,
        Record::After(a) => a.session == session,
        Record::Start(_) | Record::Last(_) => false,
    });
    if !had_after {
        return Ok(None);
    }
    let afters: Vec<&After> = records
        .iter()
        .filter_map(|r| match r {
            Record::After(a) => Some(a),
            _ => None,
        })
        .collect();
    let mine = |a: &After| a.session == session && a.agent_id.is_none();
    let mut latest: BTreeMap<&str, &After> = BTreeMap::new();
    for a in afters.iter().copied().filter(|a| mine(a)) {
        if latest.get(a.path.as_str()).is_none_or(|l| a.end >= l.end) {
            latest.insert(a.path.as_str(), a);
        }
    }
    if latest.is_empty() {
        return Ok(Some(BTreeSet::new()));
    }
    let Ok(current) = tree(top, &store).and_then(|t| blobs(top, &store, &t)) else {
        return Ok(Some(BTreeSet::new()));
    };
    let owned = latest
        .into_iter()
        .filter(|(path, rec)| {
            let now = current.get(*path).map(String::as_str);
            let same = if rec.blob == DELETED { now.is_none() } else { now == Some(rec.blob.as_str()) };
            same && !afters.iter().any(|o| {
                !mine(o)
                    && o.tool_use_id != UNRECORDED
                    && o.path == *path
                    && o.blob == rec.blob
                    && overlaps(rec.start, rec.end, o.start, o.end)
            })
        })
        .map(|(path, _)| path.to_string())
        .collect();
    Ok(Some(owned))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::TempDir;
    use crate::store::Project;

    fn setup() -> (TempDir, Project, PathBuf) {
        let tmp = TempDir::new();
        let repo = crate::git::testrepo::repo_with_commit(&tmp.0, "r");
        let home = tmp.0.join("home");
        let p = crate::store::init(&repo, &home, crate::store::Mode::Home).unwrap();
        (tmp, p, repo)
    }

    fn dir(p: &Project, repo: &Path) -> PathBuf {
        locate(p, repo).unwrap().dir
    }

    fn records(p: &Project, repo: &Path) -> String {
        std::fs::read_to_string(dir(p, repo).join(RECORDS)).unwrap_or_default()
    }

    /// Runs one write call of `session`: the before snapshot, the work-tree
    /// change, then the after snapshot and record.
    fn run_call(p: &Project, repo: &Path, session: &str, agent: Option<&str>, id: &str, write: impl FnOnce()) {
        before_call(p, repo, session, agent, id);
        write();
        after_call(p, repo, session, agent, id);
    }

    fn set(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|s| s.to_string()).collect()
    }

    fn run_light(p: &Project, repo: &Path, session: &str, id: &str, write: impl FnOnce()) {
        before_call_light(p, repo, session, None, id);
        write();
        after_call(p, repo, session, None, id);
    }

    #[test]
    fn unrecorded_change_from_a_background_job_counts_at_stop() {
        let (_tmp, p, repo) = setup();
        run_call(&p, &repo, "s1", None, "c1", || std::fs::write(repo.join("a.txt"), "first").unwrap());
        std::fs::write(repo.join("bg.txt"), "from a background job").unwrap();
        catch_up_at_stop(&p, &repo, "s1", None);
        assert_eq!(owned(&p, &repo, "s1").unwrap().unwrap(), set(&["a.txt", "bg.txt"]));
    }

    #[test]
    fn unrecorded_change_counts_at_the_next_call() {
        // `rvow prove` runs inside a shell call, whose before snapshot catches up first.
        let (_tmp, p, repo) = setup();
        run_call(&p, &repo, "s1", None, "c1", || std::fs::write(repo.join("a.txt"), "first").unwrap());
        std::fs::write(repo.join("bg.txt"), "from a background job").unwrap();
        run_call(&p, &repo, "s1", None, "c2", || {});
        assert_eq!(owned(&p, &repo, "s1").unwrap().unwrap(), set(&["a.txt", "bg.txt"]));
    }

    #[test]
    fn unrecorded_change_of_nothing_records_nothing() {
        let (_tmp, p, repo) = setup();
        run_call(&p, &repo, "s1", None, "c1", || std::fs::write(repo.join("a.txt"), "first").unwrap());
        let before = records(&p, &repo);
        catch_up_at_stop(&p, &repo, "s1", None);
        assert_eq!(records(&p, &repo), before, "an unchanged tree writes no record");
    }

    #[test]
    fn any_tool_change_a_light_call_compares_with_the_last_tree() {
        let (_tmp, p, repo) = setup();
        run_call(&p, &repo, "s1", None, "c1", || std::fs::write(repo.join("a.txt"), "first").unwrap());
        run_light(&p, &repo, "s1", "c2", || std::fs::write(repo.join("b.txt"), "mcp").unwrap());
        assert_eq!(owned(&p, &repo, "s1").unwrap().unwrap(), set(&["a.txt", "b.txt"]), "the light call's write counts");
        let text = records(&p, &repo);
        assert_eq!(text.matches("\"kind\":\"last\"").count(), 1, "one last tree per session: {text}");
    }

    #[test]
    fn any_tool_change_a_first_light_call_snapshots_before() {
        let (_tmp, p, repo) = setup();
        run_light(&p, &repo, "s1", "c1", || std::fs::write(repo.join("b.txt"), "mcp").unwrap());
        assert_eq!(owned(&p, &repo, "s1").unwrap().unwrap(), set(&["b.txt"]), "the first light call's write counts");
        run_light(&p, &repo, "s1", "c2", || std::fs::write(repo.join("c.txt"), "next").unwrap());
        assert_eq!(owned(&p, &repo, "s1").unwrap().unwrap(), set(&["b.txt", "c.txt"]), "the next light call compares with the last tree");
    }

    #[test]
    fn any_tool_change_a_light_call_leaves_another_sessions_recorded_write() {
        let (_tmp, p, repo) = setup();
        run_call(&p, &repo, "s1", None, "c1", || std::fs::write(repo.join("a.txt"), "first").unwrap());
        run_call(&p, &repo, "s2", None, "d1", || std::fs::write(repo.join("b.txt"), "theirs").unwrap());
        run_light(&p, &repo, "s1", "c2", || {});
        assert_eq!(owned(&p, &repo, "s1").unwrap().unwrap(), set(&["a.txt"]), "s2's write is not s1's");
        assert_eq!(owned(&p, &repo, "s2").unwrap().unwrap(), set(&["b.txt"]), "s2 keeps its write");
    }

    #[test]
    fn any_tool_change_a_light_call_after_a_prune_still_records() {
        let (_tmp, p, repo) = setup();
        run_call(&p, &repo, "s1", None, "c1", || std::fs::write(repo.join("a.txt"), "first").unwrap());
        grow(&dir(&p, &repo));
        run_call(&p, &repo, "s1", None, "c2", || {});
        run_light(&p, &repo, "s1", "c3", || std::fs::write(repo.join("c.txt"), "mcp").unwrap());
        assert_eq!(owned(&p, &repo, "s1").unwrap().unwrap(), set(&["a.txt", "c.txt"]), "the pruned last tree is not used");
    }

    #[test]
    fn unrecorded_change_leaves_another_sessions_recorded_write() {
        let (_tmp, p, repo) = setup();
        run_call(&p, &repo, "s2", None, "d1", || {});
        run_call(&p, &repo, "s1", None, "c1", || std::fs::write(repo.join("a.txt"), "first").unwrap());
        catch_up_at_stop(&p, &repo, "s2", None);
        assert_eq!(owned(&p, &repo, "s1").unwrap().unwrap(), set(&["a.txt"]), "s2's catch-up does not take s1's write");
        assert!(owned(&p, &repo, "s2").unwrap().is_none_or(|o| o.is_empty()), "s2 does not claim it");
    }

    #[test]
    fn unrecorded_change_never_takes_a_path_from_a_recorded_call() {
        // The catch-up of s2 runs while s1's call is still open, so it cannot see s1's record.
        let (_tmp, p, repo) = setup();
        run_call(&p, &repo, "s2", None, "d1", || {});
        before_call(&p, &repo, "s1", None, "c1");
        std::fs::write(repo.join("a.txt"), "first").unwrap();
        catch_up_at_stop(&p, &repo, "s2", None);
        after_call(&p, &repo, "s1", None, "c1");
        assert_eq!(owned(&p, &repo, "s1").unwrap().unwrap(), set(&["a.txt"]));
    }

    #[test]
    fn any_tool_change_a_light_call_that_writes_nothing_records_nothing() {
        let (_tmp, p, repo) = setup();
        run_call(&p, &repo, "s1", None, "c1", || std::fs::write(repo.join("a.txt"), "first").unwrap());
        run_light(&p, &repo, "s1", "c2", || {});
        assert_eq!(owned(&p, &repo, "s1").unwrap().unwrap(), set(&["a.txt"]));
    }

    #[test]
    fn provenance_records_the_blob_a_call_wrote() {
        let (_tmp, p, repo) = setup();
        std::fs::write(repo.join("out.txt"), "no").unwrap();
        run_call(&p, &repo, "s1", None, "t1", || std::fs::write(repo.join("out.txt"), "ok").unwrap());
        assert_eq!(owned(&p, &repo, "s1").unwrap().unwrap(), set(&["out.txt"]), "the session wrote the current content");
        assert!(records(&p, &repo).contains("out.txt"));
    }

    #[test]
    fn provenance_an_unknown_program_beside_readers_records_nothing() {
        let (_tmp, p, repo) = setup();
        std::fs::write(repo.join("out.txt"), "x").unwrap();
        // A call that changed nothing writes only a Post marker and no file.
        run_call(&p, &repo, "s1", None, "t1", || {});
        assert_eq!(owned(&p, &repo, "s1").unwrap().unwrap(), BTreeSet::new(), "a call that changed nothing records no file");
    }

    #[test]
    fn provenance_a_worker_that_rewrote_the_file_steals_ownership() {
        let (_tmp, p, repo) = setup();
        std::fs::write(repo.join("out.txt"), "no").unwrap();
        run_call(&p, &repo, "s1", None, "t1", || std::fs::write(repo.join("out.txt"), "ok").unwrap());
        assert_eq!(owned(&p, &repo, "s1").unwrap().unwrap(), set(&["out.txt"]));
        std::fs::write(repo.join("out.txt"), "worker").unwrap();
        assert_eq!(owned(&p, &repo, "s1").unwrap().unwrap(), BTreeSet::new(), "s1 no longer owns out.txt");
    }

    #[test]
    fn provenance_overlapping_calls_with_the_same_blob_have_no_owner() {
        let (_tmp, p, repo) = setup();
        before_call(&p, &repo, "s1", None, "t1");
        before_call(&p, &repo, "s2", None, "t2");
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        after_call(&p, &repo, "s1", None, "t1");
        after_call(&p, &repo, "s2", None, "t2");
        assert_eq!(owned(&p, &repo, "s1").unwrap().unwrap(), BTreeSet::new(), "overlapping same-blob calls leave no owner");
        assert_eq!(owned(&p, &repo, "s2").unwrap().unwrap(), BTreeSet::new());
    }

    #[test]
    fn provenance_a_subagent_write_inside_a_main_call_is_not_the_sessions() {
        let (_tmp, p, repo) = setup();
        before_call(&p, &repo, "s1", None, "main");
        run_call(&p, &repo, "s1", Some("a1"), "sub", || std::fs::write(repo.join("out.txt"), "sub").unwrap());
        after_call(&p, &repo, "s1", None, "main");
        assert_eq!(owned(&p, &repo, "s1").unwrap().unwrap(), BTreeSet::new(), "the subagent wrote out.txt during the main call");
    }

    #[test]
    fn provenance_a_session_that_deleted_a_file_owns_the_delete() {
        let (_tmp, p, repo) = setup();
        run_call(&p, &repo, "s1", None, "t1", || std::fs::remove_file(repo.join("a.txt")).unwrap());
        assert_eq!(owned(&p, &repo, "s1").unwrap().unwrap(), set(&["a.txt"]), "the file is still gone");
        std::fs::write(repo.join("a.txt"), "back").unwrap();
        assert_eq!(owned(&p, &repo, "s1").unwrap().unwrap(), BTreeSet::new(), "someone wrote it again");
    }

    #[test]
    fn provenance_a_session_without_after_records_falls_back() {
        let (_tmp, p, repo) = setup();
        before_call(&p, &repo, "s1", None, "t1");
        assert_eq!(owned(&p, &repo, "s1").unwrap(), None, "no after-call record means keep today's text record");
    }

    #[test]
    fn provenance_an_after_report_with_no_before_record_takes_no_snapshot() {
        let (_tmp, p, repo) = setup();
        after_call(&p, &repo, "s1", None, "read");
        assert!(!dir(&p, &repo).join("index").exists(), "a read never runs git add");
        assert_eq!(records(&p, &repo), "");
    }

    #[test]
    fn provenance_a_reused_call_id_diffs_against_its_latest_before() {
        let (_tmp, p, repo) = setup();
        before_call(&p, &repo, "s1", None, "t1");
        std::fs::write(repo.join("other.txt"), "x").unwrap();
        run_call(&p, &repo, "s1", None, "t1", || std::fs::write(repo.join("out.txt"), "ok").unwrap());
        assert_eq!(owned(&p, &repo, "s1").unwrap().unwrap(), set(&["out.txt"]), "other.txt changed before the second call");
    }

    #[test]
    fn provenance_the_users_repository_stays_untouched() {
        let (_tmp, p, repo) = setup();
        std::fs::write(repo.join("new.txt"), "x").unwrap();
        let index_before = std::fs::read(repo.join(".git/index")).unwrap();
        let objects_before = walk(&repo.join(".git/objects"));
        run_call(&p, &repo, "s1", None, "t1", || std::fs::write(repo.join("new.txt"), "y").unwrap());
        assert_eq!(std::fs::read(repo.join(".git/index")).unwrap(), index_before, "the repository index must not change");
        assert_eq!(walk(&repo.join(".git/objects")), objects_before, "the repository object store must not change");
    }

    #[test]
    fn provenance_a_linked_worktree_reads_the_shared_object_store() {
        let (tmp, p, repo) = setup();
        let wt = tmp.0.join("wt");
        crate::git::testrepo::git(&repo, &["worktree", "add", "-q", wt.to_str().unwrap()]);
        before_call(&p, &wt, "s1", None, "t1");
        assert_eq!(walk(&dir(&p, &wt).join("objects")), BTreeSet::new(), "committed content is read from the shared store, not copied");
    }

    fn walk(dir: &Path) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    out.extend(walk(&p));
                } else {
                    out.insert(p.display().to_string());
                }
            }
        }
        out
    }

    #[test]
    fn provenance_in_repo_mode_never_records_the_private_store_itself() {
        let tmp = TempDir::new();
        let repo = crate::git::testrepo::repo_with_commit(&tmp.0, "r");
        let home = tmp.0.join("home");
        let p = crate::store::init(&repo, &home, crate::store::Mode::Repo).unwrap();
        std::fs::write(repo.join("out.txt"), "no").unwrap();
        run_call(&p, &repo, "s1", None, "t1", || std::fs::write(repo.join("out.txt"), "ok").unwrap());
        let text = records(&p, &repo);
        assert!(text.contains("out.txt"), "{text}");
        let store_prefix = format!("{}/provenance/", crate::store::REPO_DIR);
        assert!(!text.contains(&store_prefix), "the private store must not record itself: {text}");
        assert_eq!(owned(&p, &repo, "s1").unwrap().unwrap(), set(&["out.txt"]), "out.txt is still owned in repo mode");
    }

    fn grow(dir: &Path) -> PathBuf {
        let big = dir.join("objects/pack/big");
        std::fs::create_dir_all(big.parent().unwrap()).unwrap();
        std::fs::File::create(&big).unwrap().set_len(2 * MAX_OBJECTS).unwrap();
        big
    }

    #[test]
    fn provenance_prune_drops_a_grown_private_store_but_keeps_records() {
        let (_tmp, p, repo) = setup();
        std::fs::write(repo.join("out.txt"), "no").unwrap();
        run_call(&p, &repo, "s1", None, "t1", || std::fs::write(repo.join("out.txt"), "ok").unwrap());
        let d = dir(&p, &repo);
        let big = grow(&d);
        run_call(&p, &repo, "s1", None, "t2", || {});
        assert!(!big.exists(), "the grown object store is pruned");
        assert_eq!(owned(&p, &repo, "s1").unwrap().unwrap(), set(&["out.txt"]), "ownership survives a prune");
    }

    #[test]
    fn provenance_a_live_call_of_another_session_holds_the_prune() {
        let (_tmp, p, repo) = setup();
        before_call(&p, &repo, "s2", None, "live");
        let big = grow(&dir(&p, &repo));
        run_call(&p, &repo, "s1", None, "t1", || {});
        assert!(big.exists(), "s2 still needs its before tree");
    }

    #[test]
    fn provenance_old_records_and_finished_or_stale_calls_are_dropped() {
        let (_tmp, p, repo) = setup();
        let d = dir(&p, &repo);
        let now = now_ms();
        let stale =
            Start { session: "s2".into(), agent_id: None, tool_use_id: "gone".into(), before: "x".into(), start: now - STALE_MS - 1 };
        let old = After {
            session: "s3".into(),
            agent_id: None,
            tool_use_id: "old".into(),
            path: "old.txt".into(),
            blob: "x".into(),
            start: now - KEEP_MS - 2,
            end: now - KEEP_MS - 1,
        };
        append(&d, &[Record::Start(stale), Record::After(old)]).unwrap();
        let big = grow(&d);
        run_call(&p, &repo, "s1", None, "t1", || std::fs::write(repo.join("out.txt"), "ok").unwrap());
        let text = records(&p, &repo);
        assert!(!text.contains("\"start\",") && !text.contains("\"kind\":\"start\""), "no before record is left: {text}");
        assert!(!text.contains("old.txt"), "a record past the keep window is dropped: {text}");
        assert!(text.contains("out.txt"), "{text}");
        assert!(!big.exists(), "a stale call does not hold the prune");
    }
}
