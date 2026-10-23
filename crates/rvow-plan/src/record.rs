//! `spec-record.json`: the hash of every living requirement as `init`,
//! `import` or the last `archive` wrote it. The gate compares the living specs
//! against it, so a hand edit of a spec outside a change shows.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::store::{Lock, Project};

pub use rvow_base::layout::RECORD_FILE;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// `vow::requirement_hash` of the requirement as written.
    pub hash: String,
    /// Imported text that predates rvow's lint rules. Its lint errors show as
    /// warnings until a change touches the requirement.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub legacy: bool,
}

/// Requirement id to its recorded entry. A missing file is an empty record.
pub type Record = BTreeMap<String, Entry>;

pub fn load(project: &Project) -> Result<Record> {
    let path = project.root.join(RECORD_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| Error::new("record", format!("{}: {e}", path.display()), "restore spec-record.json from git or a backup")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Record::new()),
        Err(e) => Err(Error::io(&path, e)),
    }
}

/// Writes the whole record through a temporary file, so a crash leaves either
/// the old or the new record.
pub fn save(project: &Project, _lock: &Lock, record: &Record) -> Result<()> {
    let path = project.root.join(RECORD_FILE);
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(record).map_err(|e| Error::internal(format!("record does not serialize: {e}")))? + "\n";
    std::fs::write(&tmp, text).map_err(|e| Error::io(&tmp, e))?;
    std::fs::rename(&tmp, &path).map_err(|e| Error::io(&path, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;
    use crate::store::{Mode, init};

    #[test]
    fn round_trip_and_missing_file() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &tmp.0.join("home"), Mode::Home).unwrap();
        assert!(load(&p).unwrap().is_empty());
        let mut r = Record::new();
        r.insert("REQ-k3f9m2".into(), Entry { hash: "abc".into(), legacy: true });
        r.insert("REQ-q8w2c7".into(), Entry { hash: "def".into(), legacy: false });
        let lock = p.lock().unwrap();
        save(&p, &lock, &r).unwrap();
        assert_eq!(load(&p).unwrap(), r);
        let text = std::fs::read_to_string(p.root.join(RECORD_FILE)).unwrap();
        assert_eq!(text.matches("legacy").count(), 1, "false is not written");
    }
}
