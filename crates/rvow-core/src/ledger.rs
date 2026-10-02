//! The append-only JSON Lines ledger of a change. Each event names the events
//! it follows by hash, so the file forms a DAG that survives git's union merge,
//! which keeps lines in arbitrary order.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::diag::{Diagnostic, Rule};
use crate::error::{Error, Result};
use crate::store::Lock;

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

/// The first 16 hex characters of the SHA-256 of `bytes`.
pub fn short_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(HASH_LEN);
    for b in digest.iter().take(HASH_LEN / 2) {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Reads a ledger in event order. A missing file is an empty ledger.
///
/// The order is topological over `parents`, with time and then hash breaking
/// ties. A last line with no newline is a crashed write and is skipped with a
/// warning. Malformed lines and parents that name no line are errors.
pub fn read(path: &Path) -> Result<(Vec<Event>, Vec<Diagnostic>)> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(Error::io(path, e)),
    };
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
        let hash = short_hash(line.as_bytes());
        if !seen.insert(hash.clone()) {
            continue;
        }
        match serde_json::from_str::<Event>(line) {
            Ok(mut ev) => {
                ev.hash = hash;
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
    Ok((order(events), diags))
}

fn order(events: Vec<Event>) -> Vec<Event> {
    let index: HashMap<String, usize> = events.iter().enumerate().map(|(i, e)| (e.hash.clone(), i)).collect();
    let mut waiting = vec![0usize; events.len()];
    let mut children: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, ev) in events.iter().enumerate() {
        for p in &ev.parents {
            if let Some(&pi) = index.get(p) {
                waiting[i] += 1;
                children.entry(pi).or_default().push(i);
            }
        }
    }
    let key = |i: usize| Reverse((events[i].time, events[i].hash.clone(), i));
    let mut ready: BinaryHeap<_> = (0..events.len()).filter(|&i| waiting[i] == 0).map(key).collect();
    let mut out = Vec::with_capacity(events.len());
    while let Some(Reverse((_, _, i))) = ready.pop() {
        out.push(i);
        for &c in children.get(&i).into_iter().flatten() {
            waiting[c] -= 1;
            if waiting[c] == 0 {
                ready.push(key(c));
            }
        }
    }
    let mut slots: Vec<Option<Event>> = events.into_iter().map(Some).collect();
    out.into_iter().filter_map(|i| slots[i].take()).collect()
}

/// Appends `line` and a newline in one write. A partial last line from a
/// crashed write is removed first; the caller's lock means no writer is still
/// working on it.
pub(crate) fn append_line(path: &Path, line: &str) -> Result<()> {
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).map_err(|e| Error::io(path, e))?;
    let old = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    if !old.is_empty() && !old.ends_with(b"\n") {
        let keep = old.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        file.set_len(keep as u64).map_err(|e| Error::io(path, e))?;
    }
    let mut buf = Vec::with_capacity(line.len() + 1);
    buf.extend_from_slice(line.as_bytes());
    buf.push(b'\n');
    file.write_all(&buf).map_err(|e| Error::io(path, e))
}

/// The events no other event names as a parent.
fn heads(events: &[Event]) -> Vec<String> {
    let named: HashSet<&str> = events.iter().flat_map(|e| e.parents.iter().map(String::as_str)).collect();
    let mut heads: Vec<String> = events.iter().filter(|e| !named.contains(e.hash.as_str())).map(|e| e.hash.clone()).collect();
    heads.sort();
    heads
}

/// Appends one event after the current heads and returns it with its hash.
/// The lock proves the caller holds the project's write lock.
pub fn append(path: &Path, _lock: &Lock, kind: Kind, data: Map<String, Value>) -> Result<Event> {
    let (events, _) = read(path)?;
    let mut ev = Event { kind, time: now_ms(), parents: heads(&events), data, hash: String::new(), line: 0 };
    let line = serde_json::to_string(&ev).map_err(|e| Error::internal(format!("ledger event does not serialize: {e}")))?;
    append_line(path, &line)?;
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
}
