//! The protected set of a change: the files that only an amendment unlocks.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value;

use crate::change;
use crate::guard;
use crate::proof::{self, Active};
use crate::store::Project;

use rvow_base::paths::{key, real_path, rel_under};

/// The path of `file` relative to the repository top, in the form the
/// protected set uses, or `None` outside the repository.
pub fn rel_path(file: &Path, cwd: &Path, top: &Path) -> Option<String> {
    let target = key(&real_path(file, cwd));
    let top = key(&real_path(top, cwd));
    rel_under(&target, &top).map(String::from)
}

/// The protected paths of the active changes whose vow names the branch of `top`.
pub fn on_branch(project: &Project, top: &Path) -> crate::error::Result<BTreeSet<String>> {
    let branch = crate::git::branch(top)?.unwrap_or_default();
    let active = proof::active(project, &change::living(project)?)?;
    let vowed: Vec<&Active> = active.iter().filter(|c| c.branch() == Some(branch.as_str())).collect();
    Ok(of(project, &vowed))
}

/// The protected paths of `active`, taken from the baselines the vow stored.
pub fn of(project: &Project, active: &[&Active]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for c in active {
        out.extend(guard::guarded(c).into_keys().filter(|k| !k.ends_with(guard::RUST_TESTS)).map(|k| key(Path::new(&k))));
        let Some(hash) = c.baseline().and_then(|b| b.get("protected")).and_then(Value::as_str) else {
            continue;
        };
        let path = project.root.join("objects").join(hash);
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        if let Ok(map) = serde_json::from_str::<serde_json::Map<String, Value>>(&text) {
            out.extend(map.keys().map(|k| key(Path::new(k))));
        }
    }
    out
}
