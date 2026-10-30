//! The append-only JSON Lines ledger of a change. Each event names the events
//! it follows by hash, so the file forms a DAG that survives git's union merge,
//! which keeps lines in arbitrary order.

use std::borrow::Cow;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path};

use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::diag::{Diagnostic, Rule};
use crate::error::{Error, Result};
use crate::store::Lock;

pub use rvow_base::time::now_ms;

const HASH_LEN: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Vow,
    Prove,
    Amend,
    Confirm,
    Block,
    Note,
    /// The user's decision about a stale relaxed task: `now`, `archive` or
    /// `keep`, recorded with the task's input hash at the time.
    Decide,
    Archive,
    Drop,
    Supersede,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub kind: Kind,
    /// Unix time in milliseconds.
    pub time: u64,
    pub parents: Vec<String>,
    #[serde(flatten)]
    pub data: Map<String, Value>,
    /// The hash of the line as written. Not stored in the line itself.
    #[serde(skip)]
    pub hash: String,
    #[serde(skip)]
    pub line: usize,
}

/// The lowercase hex digit of each half byte.
const HEX: &[u8; 16] = b"0123456789abcdef";

/// The first 16 hex characters of the SHA-256 of `bytes`.
pub fn short_hash(bytes: &[u8]) -> String {
    hex_str(&short_hex(bytes)).to_string()
}

/// The first 16 hex characters of the SHA-256 of `bytes`, as the bytes a
/// caller that only compares or shows them needs.
fn short_hex(bytes: &[u8]) -> [u8; HASH_LEN] {
    let digest = Sha256::digest(bytes);
    let mut hex = [0u8; HASH_LEN];
    for (i, b) in digest.iter().take(HASH_LEN / 2).enumerate() {
        hex[i * 2] = HEX[usize::from(b >> 4)];
        hex[i * 2 + 1] = HEX[usize::from(b & 0x0f)];
    }
    hex
}

/// `short_hex` as text. The bytes are hex digits, so this is always valid ASCII.
fn hex_str(hex: &[u8; HASH_LEN]) -> &str {
    std::str::from_utf8(hex).unwrap_or_default()
}

/// The parents of most lines: so few that they live on the stack, which keeps
/// reading one line free of heap work.
const PARENTS_INLINE: usize = 4;

/// The `parents` of one ledger line, borrowed from the text; a parent with a
/// JSON escape cannot be borrowed and is copied, so it reads as `parse` reads it.
struct Parents<'a> {
    inline: [Cow<'a, str>; PARENTS_INLINE],
    n: usize,
    spill: Vec<Cow<'a, str>>,
}

impl Parents<'_> {
    fn iter(&self) -> impl Iterator<Item = &str> {
        self.inline[..self.n].iter().chain(self.spill.iter()).map(|p| p.as_ref())
    }
}

impl<'de: 'a, 'a> Deserialize<'de> for Parents<'a> {
    fn deserialize<D: Deserializer<'de>>(de: D) -> std::result::Result<Self, D::Error> {
        de.deserialize_seq(ParentsVisitor(std::marker::PhantomData))
    }
}

/// One parent, borrowed when the text holds no escape.
#[derive(Deserialize)]
struct Parent<'a>(#[serde(borrow)] Cow<'a, str>);

struct ParentsVisitor<'a>(std::marker::PhantomData<&'a str>);

impl<'de: 'a, 'a> Visitor<'de> for ParentsVisitor<'a> {
    type Value = Parents<'a>;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a list of parent hashes")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Parents<'a>, A::Error> {
        let mut out = Parents { inline: [const { Cow::Borrowed("") }; PARENTS_INLINE], n: 0, spill: Vec::new() };
        while let Some(Parent(parent)) = seq.next_element::<Parent<'a>>()? {
            if out.n < PARENTS_INLINE {
                out.inline[out.n] = parent;
                out.n += 1;
            } else {
                out.spill.push(parent);
            }
        }
        Ok(out)
    }
}

/// One ledger line as the heads read it: the fields that make it the event
/// `parse` would build, so a line it rejects is skipped here too, with the data
/// of the event left unread.
#[derive(Deserialize)]
struct Line<'a> {
    #[serde(rename = "kind")]
    _kind: Kind,
    #[serde(rename = "time")]
    _time: u64,
    #[serde(borrow)]
    parents: Parents<'a>,
}

/// The events of a ledger text as (line, parents), in file order, without the
/// partial last line of a crashed write and without a repeated line. Parsing
/// the data of each event is what this leaves out.
fn event_lines(text: &str) -> Vec<(&str, Parents<'_>)> {
    let complete = text.ends_with('\n');
    let lines: Vec<&str> = text.lines().collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() || (i + 1 == lines.len() && !complete) {
            continue;
        }
        // A repeated line holds the same hash, and `parse` keeps one event per hash.
        if seen.insert(*line)
            && let Ok(parsed) = serde_json::from_str::<Line>(line)
        {
            out.push((*line, parsed.parents));
        }
    }
    out
}

/// The heads of a ledger text: the hashes of its events that no event names as
/// a parent. `parse` and `heads` answer the same over the events.
fn heads_of(text: &str) -> Vec<String> {
    let events = event_lines(text);
    let mut named: HashSet<&str> = HashSet::new();
    for (_, parents) in &events {
        named.extend(parents.iter());
    }
    let mut heads = Vec::new();
    for (line, _) in &events {
        let hex = short_hex(line.as_bytes());
        if !named.contains(hex_str(&hex)) {
            heads.push(hex_str(&hex).to_string());
        }
    }
    heads.sort();
    heads.dedup();
    heads
}

/// Reads a ledger in event order. A missing file is an empty ledger.
///
/// The order is topological over `parents`, with time and then hash breaking
/// ties. A last line with no newline is a crashed write and is skipped with a
/// warning. Malformed lines and parents that name no line are errors.
pub fn read(path: &Path) -> Result<(Vec<Event>, Vec<Diagnostic>)> {
    Ok(parse(&read_text(path)?))
}

/// The text of a ledger, where a missing file is an empty one.
fn read_text(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(Error::io(path, e)),
    }
}

/// The events of a ledger text in event order, with the diagnostics.
fn parse(text: &str) -> (Vec<Event>, Vec<Diagnostic>) {
    let mut diags = Vec::new();
    let mut events: Vec<Event> = Vec::new();
    let mut seen = HashSet::new();
    let complete = text.ends_with('\n');
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let n = i + 1;
        if line.trim().is_empty() {
            continue;
        }
        if i + 1 == lines.len() && !complete {
            diags.push(Diagnostic::new(Rule::LedgerTail, n, "the last line has no newline, a write did not finish, skipped"));
            continue;
        }
        let hex = short_hex(line.as_bytes());
        if !seen.insert(hex) {
            continue;
        }
        match serde_json::from_str::<Event>(line) {
            Ok(mut ev) => {
                ev.hash = hex_str(&hex).to_string();
                ev.line = n;
                events.push(ev);
            }
            Err(e) => diags.push(Diagnostic::new(Rule::Ledger, n, format!("not a ledger event: {e}"))),
        }
    }
    let known: HashSet<&str> = events.iter().map(|e| e.hash.as_str()).collect();
    for ev in &events {
        for p in ev.parents.iter().filter(|p| !known.contains(p.as_str())) {
            diags.push(Diagnostic::new(Rule::Ledger, ev.line, format!("parent {p} names no line, an event was edited or removed")));
        }
    }
    (order(events), diags)
}

fn order(events: Vec<Event>) -> Vec<Event> {
    let index: HashMap<&str, usize> = events.iter().enumerate().map(|(i, e)| (e.hash.as_str(), i)).collect();
    let mut waiting = vec![0usize; events.len()];
    let mut children: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, ev) in events.iter().enumerate() {
        for p in &ev.parents {
            if let Some(&pi) = index.get(p.as_str()) {
                waiting[i] += 1;
                children.entry(pi).or_default().push(i);
            }
        }
    }
    let mut out = Vec::with_capacity(events.len());
    // The heap holds borrows of the events, so it is out of scope before they move.
    {
        let key = |i: usize| Reverse((events[i].time, events[i].hash.as_str(), i));
        let mut ready: BinaryHeap<_> = (0..events.len()).filter(|&i| waiting[i] == 0).map(key).collect();
        while let Some(Reverse((_, _, i))) = ready.pop() {
            out.push(i);
            for &c in children.get(&i).into_iter().flatten() {
                waiting[c] -= 1;
                if waiting[c] == 0 {
                    ready.push(key(c));
                }
            }
        }
    }
    let mut slots: Vec<Option<Event>> = events.into_iter().map(Some).collect();
    out.into_iter().filter_map(|i| slots[i].take()).collect()
}

/// Appends `line` and a newline in one write. A partial last line from a
/// crashed write is removed first; the caller's lock means no writer is still
/// working on it.
pub fn append_line(path: &Path, line: &str) -> Result<()> {
    let mut file = open_append(path)?;
    // One byte at the end decides whether the last line finished; only a torn
    // tail, which a crashed write leaves, needs the rest of the file.
    if ends_with_newline(&mut file, path)? {
        return write_line(&mut file, path, line);
    }
    let old = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    write_after(&mut file, path, line, &old)
}

fn open_append(path: &Path) -> Result<std::fs::File> {
    std::fs::OpenOptions::new().create(true).append(true).read(true).open(path).map_err(|e| Error::io(path, e))
}

/// Whether the file ends with a newline, or holds nothing at all: one byte read.
fn ends_with_newline(file: &mut std::fs::File, path: &Path) -> Result<bool> {
    let end = file.metadata().map_err(|e| Error::io(path, e))?.len();
    if end == 0 {
        return Ok(true);
    }
    let mut byte = [0u8; 1];
    file.seek(SeekFrom::Start(end - 1)).map_err(|e| Error::io(path, e))?;
    file.read_exact(&mut byte).map_err(|e| Error::io(path, e))?;
    Ok(byte[0] == b'\n')
}

/// Removes the partial last line `old` ends with, then appends `line` and a newline.
fn write_after(file: &mut std::fs::File, path: &Path, line: &str, old: &[u8]) -> Result<()> {
    if !old.is_empty() && !old.ends_with(b"\n") {
        let keep = old.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        file.set_len(keep as u64).map_err(|e| Error::io(path, e))?;
    }
    write_line(file, path, line)
}

/// Appends `line` and a newline in one write.
fn write_line(file: &mut std::fs::File, path: &Path, line: &str) -> Result<()> {
    let mut buf = Vec::with_capacity(line.len() + 1);
    buf.extend_from_slice(line.as_bytes());
    buf.push(b'\n');
    file.write_all(&buf).map_err(|e| Error::io(path, e))
}

/// A proof run that `gate --out` writes and `rvow ingest` reads: the Prove
/// event data plus the change it proves and the project folder (under the
/// home store) that locates that change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub project: String,
    pub change: String,
    #[serde(flatten)]
    pub data: Map<String, Value>,
}

/// Rejects absolute paths and parent traversal in a run's home-store locator.
pub fn safe_ingest_project(folder: &str) -> bool {
    !folder.is_empty() && !folder.contains('\\') && Path::new(folder).components().all(|c| matches!(c, Component::Normal(_)))
}

/// Records an imported run in the ledger of the active change it names in
/// the project at `project_root`.
pub fn ingest_run(project_root: &Path, run: &Run) -> Result<Event> {
    if ["kind", "time", "parents", "hash", "line", "seal"].iter().any(|name| run.data.contains_key(*name)) {
        return Err(Error::new("run-data", "a proof result contains reserved event fields", "ingest the original CI result file"));
    }
    if !safe_ingest_project(&run.project) {
        return Err(Error::new(
            "run-project",
            format!("{} is not a relative project locator", run.project),
            "ingest the original CI result file",
        ));
    }
    if let Some(identity) = run.data.get("project_identity") {
        let path = project_root.join(crate::store::PROJECT_FILE);
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        let actual: crate::store::Meta =
            toml::from_str(&text).map_err(|e| Error::new("project-file", format!("{}: {e}", path.display()), "run rvow rebind"))?;
        let expected: crate::store::Meta = serde_json::from_value(identity.clone())
            .map_err(|e| Error::new("run-project", e.to_string(), "ingest the original CI result file"))?;
        if actual != expected {
            return Err(Error::new(
                "run-project",
                format!("{} does not match the run's project identity", project_root.display()),
                "ingest into the matching project",
            ));
        }
    }
    let project = crate::store::Project {
        root: project_root.to_path_buf(),
        meta: crate::store::Meta { roots: Vec::new(), remote: None, mode: crate::store::Mode::Home },
    };
    let change = crate::change::resolve(&project, &run.change).map_err(|_| {
        Error::new(
            "no-change",
            format!("{} is not a change in {}", run.change, project_root.display()),
            "run ingest where the project's store holds that change",
        )
    })?;
    if change.stage != crate::change::Stage::Active {
        return Err(Error::new(
            "no-change",
            format!("{} is not active in this project", run.change),
            "ingest proves into an active change",
        ));
    }
    verify_run_matches_plan(&project, &change, run)?;
    let mut data = run.data.clone();
    let objects = data.remove("test_objects");
    if let Some(objects) = &objects {
        let objects = objects
            .as_object()
            .ok_or_else(|| Error::new("run-objects", "test_objects is not an object", "ingest the original CI result file"))?;
        for (hash, text) in objects {
            if text.as_str().is_none_or(|text| crate::vow::sha256_hex(text.as_bytes()) != *hash) {
                return Err(Error::new(
                    "run-objects",
                    format!("test object {hash} has invalid content"),
                    "ingest the original CI result file",
                ));
            }
        }
    }
    let lock = project.lock()?;
    if let Some(objects) = objects.and_then(|v| v.as_object().cloned()) {
        for text in objects.values().filter_map(serde_json::Value::as_str) {
            crate::vow::put_object(&project, text.as_bytes())?;
        }
    }
    let key = home_for(project_root, &run.project).map(|home| crate::seal::key(&home)).transpose()?;
    append_sealed(&change.path.join(crate::change::LEDGER), &lock, Kind::Prove, data, key.as_deref())
}

/// The rvow home that holds `project_root`, which `ingest` builds as
/// `home.join(run.project)`.
fn home_for(project_root: &Path, locator: &str) -> Option<std::path::PathBuf> {
    project_root.ancestors().nth(Path::new(locator).components().count()).map(Path::to_path_buf)
}

/// A run proves only a task that exists in the change, as it is written now:
/// the task hash and the covered-requirement hashes must be the plan's own.
fn verify_run_matches_plan(project: &crate::store::Project, change: &crate::change::ChangeDir, run: &Run) -> Result<()> {
    let refuse =
        |what: String| Error::new("run-task", what, "ingest the file that rvow gate --out wrote for this change, or run rvow prove");
    let id = run.data.get("task").and_then(Value::as_str).unwrap_or("");
    let (living, _) = crate::change::load_living(project)?;
    let loaded = crate::change::load(&change.path, &living)?;
    let Some(task) = loaded.tasks.iter().find(|t| t.id.to_string() == id) else {
        return Err(refuse(format!("task {id:?} is not in {}", run.change)));
    };
    let want = crate::vow::task_hash(task);
    if run.data.get("task_hash").and_then(Value::as_str) != Some(want.as_str()) {
        return Err(refuse(format!("task_hash of {id} does not match the plan of {}", run.change)));
    }
    let reqs = serde_json::json!(crate::change::covered(task, &loaded, &living));
    if run.data.get("requirements") != Some(&reqs) {
        return Err(refuse(format!("requirements of {id} do not match the covered requirements of {}", run.change)));
    }
    Ok(())
}

/// The events no other event names as a parent. `append` writes these as the
/// parents of the next event, so an event whose parents are not the heads of
/// the events before it was not appended in place.
pub fn heads(events: &[Event]) -> Vec<String> {
    let named: HashSet<&str> = events.iter().flat_map(|e| e.parents.iter().map(String::as_str)).collect();
    let mut heads: Vec<String> = events.iter().filter(|e| !named.contains(e.hash.as_str())).map(|e| e.hash.clone()).collect();
    heads.sort();
    heads
}

/// Appends one event after the current heads and returns it with its hash.
/// The lock proves the caller holds the project's write lock.
pub fn append(path: &Path, lock: &Lock, kind: Kind, data: Map<String, Value>) -> Result<Event> {
    append_sealed(path, lock, kind, data, None)
}

/// Like `append`, but a `key` seals the event under the rerun cache key. The
/// seal covers the kind, time and parents that this call picks, so a sealed
/// event copied after a later one no longer checks.
pub fn append_sealed(path: &Path, _lock: &Lock, kind: Kind, data: Map<String, Value>, key: Option<&[u8]>) -> Result<Event> {
    // One read of the ledger: it tells the parents and the partial last line.
    let text = read_text(path)?;
    let time = now_ms();
    let parents = heads_of(&text);
    let mut data = data;
    if let Some(key) = key {
        let value = crate::seal::seal(key, kind, time, &parents, &data)?;
        data.insert(crate::seal::SEAL_FIELD.into(), value.into());
    }
    let mut ev = Event { kind, time, parents, data, hash: String::new(), line: 0 };
    let line = serde_json::to_string(&ev).map_err(|e| Error::internal(format!("ledger event does not serialize: {e}")))?;
    let mut file = open_append(path)?;
    write_after(&mut file, path, &line, text.as_bytes())?;
    ev.hash = short_hash(line.as_bytes());
    Ok(ev)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;
    use crate::store::{Meta, Mode, Project};

    fn project(dir: &Path) -> Project {
        Project { root: dir.to_path_buf(), meta: Meta { roots: vec![], remote: None, mode: Mode::Home } }
    }

    fn data(note: &str) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("text".into(), note.into());
        m
    }

    #[test]
    fn appends_form_a_chain() {
        let tmp = TempDir::new();
        let p = project(&tmp.0);
        let path = tmp.0.join("ledger.jsonl");
        let lock = p.lock().unwrap();
        let a = append(&path, &lock, Kind::Note, data("a")).unwrap();
        let b = append(&path, &lock, Kind::Note, data("b")).unwrap();
        assert_eq!(a.parents, Vec::<String>::new());
        assert_eq!(b.parents, vec![a.hash.clone()]);
        let (events, diags) = read(&path).unwrap();
        assert!(diags.is_empty(), "{diags:?}");
        let texts: Vec<&Value> = events.iter().map(|e| &e.data["text"]).collect();
        assert_eq!(texts, ["a", "b"]);
        assert_eq!(events[1].hash, b.hash);
    }

    #[test]
    fn relaxed_stale_decide_event_is_written_as_decide() {
        let tmp = TempDir::new();
        let p = project(&tmp.0);
        let path = tmp.0.join("ledger.jsonl");
        let lock = p.lock().unwrap();
        let mut m = Map::new();
        m.insert("task".into(), "TSK-x7k2q4".into());
        m.insert("choice".into(), "keep".into());
        m.insert("inputs".into(), "abc".into());
        append(&path, &lock, Kind::Decide, m).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(r#""kind":"decide""#), "{text}");
        let (events, diags) = read(&path).unwrap();
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(events[0].kind, Kind::Decide);
        assert_eq!(events[0].data["task"], serde_json::json!("TSK-x7k2q4"));
        assert_eq!(events[0].data["choice"], serde_json::json!("keep"));
        assert_eq!(events[0].data["inputs"], serde_json::json!("abc"));
    }

    #[test]
    fn union_merge_order_and_edits() {
        let tmp = TempDir::new();
        let p = project(&tmp.0);
        let path = tmp.0.join("ledger.jsonl");
        let lock = p.lock().unwrap();
        append(&path, &lock, Kind::Note, data("root")).unwrap();
        append(&path, &lock, Kind::Note, data("main")).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();

        // A branch appended after "root" too; a union merge put its line first.
        let root_hash = short_hash(lines[0].as_bytes());
        let branch = format!(r#"{{"kind":"note","time":1,"parents":["{root_hash}"],"text":"branch"}}"#);
        std::fs::write(&path, format!("{branch}\n{}\n{}\n", lines[1], lines[0])).unwrap();
        let (events, diags) = read(&path).unwrap();
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(events[0].data["text"], "root");
        assert_eq!(events.len(), 3);
        assert_eq!(heads(&events).len(), 2);
        let next = append(&path, &lock, Kind::Note, data("join")).unwrap();
        assert_eq!(next.parents.len(), 2);

        // Removing "root" breaks both children.
        let text = std::fs::read_to_string(&path).unwrap();
        let kept: Vec<&str> = text.lines().filter(|l| !l.contains("\"root\"")).collect();
        std::fs::write(&path, kept.join("\n") + "\n").unwrap();
        let (_, diags) = read(&path).unwrap();
        assert_eq!(diags.iter().filter(|d| d.rule == Rule::Ledger).count(), 2);
    }

    #[test]
    fn torn_last_line_is_skipped_and_repaired() {
        let tmp = TempDir::new();
        let p = project(&tmp.0);
        let path = tmp.0.join("ledger.jsonl");
        let lock = p.lock().unwrap();
        append(&path, &lock, Kind::Note, data("ok")).unwrap();
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(br#"{"kind":"note","ti"#).unwrap();
        let (events, diags) = read(&path).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(diags[0].rule, Rule::LedgerTail);

        append(&path, &lock, Kind::Note, data("after")).unwrap();
        let (events, diags) = read(&path).unwrap();
        assert_eq!(events.len(), 2);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn parents_of_a_new_event_agree_with_the_parsed_heads() {
        let tmp = TempDir::new();
        let p = project(&tmp.0);
        let path = tmp.0.join("ledger.jsonl");
        let lock = p.lock().unwrap();
        let root = append(&path, &lock, Kind::Note, data("root")).unwrap();
        // More parents than the heads read keeps on the stack, then a line that is
        // not json, one without parents, a repeated line and a torn tail.
        let orphans: Vec<String> = (0..4).map(|i| format!("{i}").repeat(16)).collect();
        let many = format!(
            r#"{{"kind":"note","time":2,"parents":["{}",{}],"text":"many"}}"#,
            root.hash,
            orphans.iter().map(|o| format!("\"{o}\"")).collect::<Vec<_>>().join(",")
        );
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str(&format!("{many}\nnot json\n{{\"kind\":\"note\",\"time\":3}}\n{many}\n{{\"kind\":\"note\",\"time\":4,\"te"));
        std::fs::write(&path, &text).unwrap();
        let (events, diags) = read(&path).unwrap();
        assert_eq!(events.len(), 2, "a torn line, a malformed one and a repeat are out: {diags:?}");
        assert_eq!(heads_of(&text), heads(&events));

        let next = append(&path, &lock, Kind::Note, data("join")).unwrap();
        let (events, _) = read(&path).unwrap();
        assert_eq!(events.last().unwrap().parents, heads(&events[..events.len() - 1]), "the append wrote the parents of {next:?}");
    }

    #[test]
    fn heads_agree_when_a_parent_holds_a_json_escape() {
        let text = "{\"kind\":\"note\",\"time\":1,\"parents\":[],\"text\":\"a\"}\n\
                    {\"kind\":\"note\",\"time\":2,\"parents\":[\"ab\\u0063d\"],\"text\":\"b\"}\n";
        let (events, _) = parse(text);
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].parents, ["abcd"]);
        assert_eq!(heads_of(text), heads(&events));
    }
}
