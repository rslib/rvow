//! Compiled glob sets, cached for the thread that asks, because one hook call
//! asks for the same few sets many times.

use std::cell::RefCell;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::error::{Error, Result};

/// The compiled glob sets each thread keeps, oldest dropped first.
const GLOBSETS_KEPT: usize = 32;

/// The set `patterns` compile to. A compiled set is a pure function of its
/// patterns, and building one compiles every regex. An entry with no glob
/// characters names a file or a folder, so it also matches every path under it.
pub fn globset(patterns: &[String]) -> Result<GlobSet> {
    thread_local! {
        static SETS: RefCell<Vec<(Vec<String>, GlobSet)>> = const { RefCell::new(Vec::new()) };
    }
    if let Some(set) = SETS.with(|sets| sets.borrow().iter().find(|(p, _)| p == patterns).map(|(_, set)| set.clone())) {
        return Ok(set);
    }
    let set = build_globset(patterns)?;
    SETS.with(|sets| {
        let mut sets = sets.borrow_mut();
        if sets.len() >= GLOBSETS_KEPT {
            sets.remove(0);
        }
        sets.push((patterns.to_vec(), set.clone()));
    });
    Ok(set)
}

fn build_globset(patterns: &[String]) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        let glob = GlobBuilder::new(p)
            .literal_separator(true)
            .build()
            .map_err(|e| Error::new("glob", format!("bad glob {p:?}: {e}"), "fix the glob"))?;
        b.add(glob);
        if !p.contains(['*', '?', '[', '{']) {
            let under = format!("{}/**", p.trim_end_matches('/'));
            b.add(
                GlobBuilder::new(&under)
                    .literal_separator(true)
                    .build()
                    .map_err(|e| Error::new("glob", format!("bad glob {under:?}: {e}"), "fix the glob"))?,
            );
        }
    }
    b.build().map_err(|e| Error::new("glob", e.to_string(), "fix the globs"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(p: &str) -> GlobSet {
        globset(&[p.to_string()]).unwrap()
    }

    #[test]
    fn folder_entry_covers_every_path_under_it() {
        let s = set("crates/a");
        assert!(s.is_match("crates/a"), "the entry itself");
        assert!(s.is_match("crates/a/Cargo.toml"));
        assert!(s.is_match("crates/a/src/deep/x.rs"));
        assert!(!s.is_match("crates/ab/x.rs"), "a sibling with the same prefix");
        assert!(!s.is_match("crates"));
    }

    #[test]
    fn folder_entry_with_a_trailing_slash_covers_the_folder() {
        assert!(set("crates/a/").is_match("crates/a/src/x.rs"));
    }

    #[test]
    fn folder_entry_rule_leaves_glob_entries_alone() {
        let s = set("src/*.rs");
        assert!(s.is_match("src/a.rs"));
        assert!(!s.is_match("src/a.rs/b"), "a glob entry gets no folder match");
        assert!(!s.is_match("src/x/a.rs"), "* stays inside one folder");
    }
}
