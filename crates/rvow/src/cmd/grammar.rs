//! Fill and show the grammar cache behind `rvow grammar`.

use rvow_core::error::{Error, Result};
use rvow_core::store;
use rvow_core::syntax::grammar;

use crate::output::Reply;
use crate::toon::Obj;

/// States of a pinned grammar in the cache.
enum CacheState {
    Cached,
    Stale,
    Missing,
}

fn cache_state(home: &std::path::Path, entry: &grammar::Entry) -> CacheState {
    match grammar::cached(home, entry) {
        Some(_) => CacheState::Cached,
        None if grammar::cache_file(home, entry).is_file() => CacheState::Stale,
        None => CacheState::Missing,
    }
}

/// The download base override for this process.
fn base() -> Option<String> {
    std::env::var(grammar::URL_VAR).ok().filter(|v| !v.is_empty())
}

fn state_name(state: &CacheState) -> &'static str {
    match state {
        CacheState::Cached => "cached",
        CacheState::Stale => "stale",
        CacheState::Missing => "missing",
    }
}

fn row(home: &std::path::Path, entry: &grammar::Entry) -> Obj {
    let file = grammar::cache_file(home, entry);
    let size = std::fs::metadata(&file).ok().map(|m| m.len()).unwrap_or(0);
    crate::obj! {
        "lang" => entry.name.as_str(),
        "ext" => entry.ext.join(",").as_str(),
        "state" => state_name(&cache_state(home, entry)),
        "sha256" => entry.sha256.as_str(),
        "size" => size,
    }
}

/// `rvow grammar fetch [<lang>...]`: fills the cache from the pinned list;
/// without names, every pinned language. Fails on the first grammar the
/// cache or the download cannot satisfy, and a wrong download is never
/// written to the cache.
pub fn fetch(langs: &[String]) -> Result<Reply> {
    let home = store::home_dir()?;
    let targets: Vec<&'static grammar::Entry> = if langs.is_empty() {
        grammar::pinned().iter().filter(|e| !matches!(cache_state(&home, e), CacheState::Cached)).collect()
    } else {
        let mut targets = Vec::with_capacity(langs.len());
        for lang in langs {
            let entry = grammar::by_name(lang).ok_or_else(|| {
                Error::new("grammar", format!("no pinned grammar named {lang}"), "run rvow grammar list for the pinned names")
            })?;
            if !targets.iter().any(|e: &&grammar::Entry| e.name == entry.name) {
                targets.push(entry);
            }
        }
        targets
    };
    if targets.is_empty() {
        let mut r = Reply::new();
        r.line(format!("the {} pinned grammars are already in the cache", grammar::pinned().len()));
        r.help("run `rvow grammar list` to see each pinned language and its cache state");
        return Ok(r);
    }
    let base = base();
    let mut table = Vec::new();
    let mut fetched = 0usize;
    let mut cached = 0usize;
    for entry in &targets {
        let was_cached = matches!(cache_state(&home, entry), CacheState::Cached);
        let bytes = grammar::acquire(&home, entry, base.as_deref())?;
        // An explicit fetch always tries, and a success clears the one-hour
        // failure record of prove and gate.
        grammar::clear_fetch_failure(&home, entry);
        if was_cached {
            cached += 1;
        } else {
            fetched += 1;
        }
        table.push(crate::obj! {
            "lang" => entry.name.as_str(),
            "state" => if was_cached { "cached" } else { "fetched" },
            "size" => bytes.len(),
        });
    }
    let mut r = Reply::new();
    r.line(format!("fetched {fetched}, already cached {cached}"));
    r.table("grammars", table, "no grammars");
    r.help("a grammar download is checked by sha256 against the pinned list before it enters the cache");
    Ok(r)
}

/// `rvow grammar list`: each pinned language and whether the cache holds it.
pub fn list() -> Result<Reply> {
    let home = store::home_dir()?;
    let entries = grammar::pinned().iter().collect::<Vec<_>>();
    let cached = entries.iter().filter(|e| matches!(cache_state(&home, e), CacheState::Cached)).count();
    let mut rows: Vec<Obj> = entries.iter().map(|e| row(&home, e)).collect();
    rows.sort_by_key(|row| match row.iter().find(|(k, _)| k == "lang") {
        Some((_, crate::toon::Node::Str(s))) => s.clone(),
        _ => String::new(),
    });
    let mut r = Reply::new();
    let stale = entries.iter().filter(|e| matches!(cache_state(&home, e), CacheState::Stale)).count();
    r.line(format!("{} pinned languages, {cached} cached, {stale} stale", entries.len()));
    r.table("grammars", rows, "no pinned grammars");
    r.help("run `rvow grammar fetch json` to fill the cache for one language, or `rvow grammar fetch` for all");
    Ok(r)
}
