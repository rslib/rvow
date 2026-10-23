//! What the harness saw the user make, kept in the store so every session of the project reads it.

use serde_json::{Map, Value};

use crate::store::Project;

pub(super) use rvow_shell::witness::{WITNESS_FILE, reason};

/// The lists of the `user_made` record that the store keeps.
const LISTS: [&str; 3] = ["changes", "yolo", "runs"];

/// Entries kept per list; a session sends its own again with every call, so a trimmed one comes back.
const KEPT: usize = 64;

/// The `user_made` record `seen` joined with the record of the store, which is saved when `seen` adds to it.
pub(super) fn merged(project: &Project, seen: &Value) -> Value {
    let path = project.root.join(WITNESS_FILE);
    let stored: Map<String, Value> =
        std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
    let mut out = seen.clone();
    let mut next = stored.clone();
    for key in LISTS {
        let mut list: Vec<Value> = stored.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
        for entry in seen.get(key).and_then(Value::as_array).into_iter().flatten() {
            if !list.contains(entry) {
                list.push(entry.clone());
            }
        }
        list.drain(..list.len().saturating_sub(KEPT));
        if let Some(object) = out.as_object_mut() {
            object.insert(key.to_string(), Value::Array(list.clone()));
        }
        next.insert(key.to_string(), Value::Array(list));
    }
    if next != stored {
        let tmp = path.with_extension("json.tmp");
        // A write that fails leaves the session with its own record, which is what it had before.
        if std::fs::write(&tmp, Value::Object(next).to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
    out
}
