//! Compiled glob sets, cached for the thread that asks, because one hook call
//! asks for the same few sets many times.

use std::cell::RefCell;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::error::{Error, Result};

/// The compiled glob sets each thread keeps, oldest dropped first.
const GLOBSETS_KEPT: usize = 32;

/// The set `patterns` compile to. A compiled set is a pure function of its
/// patterns, and building one compiles every regex.
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
    }
    b.build().map_err(|e| Error::new("glob", e.to_string(), "fix the globs"))
}
