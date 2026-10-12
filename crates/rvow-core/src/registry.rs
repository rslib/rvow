//! `ids.jsonl`: every id the project ever minted, so a removed id is never
//! drawn again.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::id::{Id, Kind};
use crate::ledger::{append_line, now_ms};
use crate::store::{IDS_FILE, Lock, Project};

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    id: String,
    change: Option<String>,
    time: u64,
}

/// Every id in the registry. A partial last line from a crashed write is
/// ignored, and an unreadable line is an error.
pub fn load(project: &Project) -> Result<HashSet<Id>> {
    let path = project.root.join(IDS_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(Error::io(&path, e)),
    };
    let complete = text.ends_with('\n');
    let lines: Vec<&str> = text.lines().collect();
    let mut ids = HashSet::new();
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() || (i + 1 == lines.len() && !complete) {
            continue;
        }
        let id = serde_json::from_str::<Entry>(line).ok().and_then(|e| e.id.parse::<Id>().ok()).ok_or_else(|| {
            Error::new(
                "ids-file",
                format!("{}: line {} is not an id entry", path.display(), i + 1),
                "restore ids.jsonl from git or a backup",
            )
        })?;
        ids.insert(id);
    }
    Ok(ids)
}

/// The ids that rvow minted for `change`.
pub fn minted_for(project: &Project, change: &Id) -> Result<HashSet<Id>> {
    let path = project.root.join(IDS_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(e) => return Err(Error::io(&path, e)),
    };
    let want = change.to_string();
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str::<Entry>(line).ok())
        .filter(|e| e.change.as_deref() == Some(want.as_str()))
        .filter_map(|e| e.id.parse::<Id>().ok())
        .collect())
}

/// Mints an id that the project never used and records it.
pub fn mint(project: &Project, _lock: &Lock, kind: Kind, change: Option<&Id>) -> Result<Id> {
    let taken = load(project)?;
    let id = Id::mint(kind, &taken)?;
    let entry = Entry { id: id.to_string(), change: change.map(Id::to_string), time: now_ms() };
    let line = serde_json::to_string(&entry).map_err(|e| Error::internal(format!("ids entry does not serialize: {e}")))?;
    append_line(&project.root.join(IDS_FILE), &line)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;
    use crate::store::{Mode, init};

    #[test]
    fn minted_ids_are_recorded_and_never_repeat() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &tmp.0.join("home"), Mode::Home).unwrap();
        let lock = p.lock().unwrap();
        let chg = mint(&p, &lock, Kind::Chg, None).unwrap();
        let mut seen = HashSet::from([chg.clone()]);
        for _ in 0..50 {
            assert!(seen.insert(mint(&p, &lock, Kind::Tsk, Some(&chg)).unwrap()));
        }
        assert_eq!(load(&p).unwrap(), seen);
    }

    #[test]
    fn a_bad_line_is_an_error() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &tmp.0.join("home"), Mode::Home).unwrap();
        std::fs::write(p.root.join(IDS_FILE), "nonsense\n").unwrap();
        assert_eq!(load(&p).unwrap_err().code, "ids-file");
    }
}
