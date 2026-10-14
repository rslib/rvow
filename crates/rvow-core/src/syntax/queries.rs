//! Framework queries: the tree-sitter queries that mark a unit as a test,
//! name it, or mark a call as a group container. A shipped query per pinned
//! language names the common frameworks; a project adds its own per language
//! in `config.toml`. A missing or bad query file never decides a check: it
//! only leaves the drift item unnamed.

use std::collections::BTreeMap;
use std::path::Path;

use tree_sitter::{Language, Node, Query, QueryCursor, StreamingIterator};

use crate::error::{Error, Result};

/// The shipped query source of each language, in `include_str!` order. A
/// language that is not here has no shipped query; a project can still add
/// one in its config.
const SHIPPED: &[(&str, &str)] = &[
    ("javascript", include_str!("queries/javascript.scm")),
    // The TypeScript grammars parse test/it/describe as call expressions too.
    ("typescript", include_str!("queries/javascript.scm")),
    ("tsx", include_str!("queries/javascript.scm")),
    ("python", include_str!("queries/python.scm")),
    ("rust", include_str!("queries/rust.scm")),
    ("cpp", include_str!("queries/cpp.scm")),
    ("go", include_str!("queries/go.scm")),
];

/// One compiled query. A shipped query is the one `SHIPPED` holds; a project
/// query only names tests and never decides a split or a free unit.
struct Compiled {
    query: Query,
    shipped: bool,
}

/// The queries of one language: the shipped source first, then the project's.
pub struct Queries {
    compiled: Vec<Compiled>,
}

/// Compiles the shipped query of `lang` and every project file `config`
/// names for it. `top` resolves the repo-relative project paths. A query
/// source that fails to compile is a problem, never a check failure: the
/// caller keeps the queries that did compile and warns.
pub fn load(lang: &str, language: &Language, top: &Path, config: &BTreeMap<String, String>) -> Queries {
    let mut compiled = Vec::new();
    if let Some(source) = SHIPPED.iter().find(|(name, _)| *name == lang).map(|(_, s)| *s)
        && let Ok(query) = Query::new(language, source)
    {
        compiled.push(Compiled { query, shipped: true });
    }
    let Some(rel) = config.get(lang) else { return Queries { compiled } };
    let file = top.join(rel);
    let Ok(source) = std::fs::read_to_string(&file) else {
        return Queries { compiled };
    };
    // A query that fails to compile is a problem rvow check reports; the
    // drift check keeps the queries that did compile.
    if let Ok(query) = Query::new(language, &source) {
        compiled.push(Compiled { query, shipped: false });
    }
    Queries { compiled }
}

impl Queries {
    /// The name of the test that `item` holds, from the first query that
    /// marks it: a match whose `@test` node spans the whole item, plus a
    /// statement's trailing semicolon, and whose predicates hold. The name
    /// is the match's `@name` captures joined with `.`, so one pattern can
    /// name a GoogleTest `TEST(Suite, Name)` case `Suite.Name`. An item that
    /// no query marks stays unnamed.
    pub fn name(&self, item: Node, text: &[u8]) -> Option<String> {
        let names = self.marked_names(item, text, false)?;
        (!names.is_empty()).then(|| names.join("."))
    }

    /// The name that a shipped query gives the test `item` holds, or `None`
    /// when no shipped query marks it or it has no name. Only shipped queries
    /// can make a new unit free.
    pub fn shipped_name(&self, item: Node, text: &[u8]) -> Option<String> {
        let names = self.marked_names(item, text, true)?;
        (!names.is_empty()).then(|| names.join("."))
    }

    /// Whether a shipped query marks a call inside `item` as a group: a
    /// container that splits into a header unit plus one unit per body item,
    /// such as a JavaScript `describe(...)`. A project query never splits.
    pub fn group(&self, item: Node, text: &[u8]) -> bool {
        let mut cursor = QueryCursor::new();
        for c in self.compiled.iter().filter(|c| c.shipped) {
            let names = c.query.capture_names();
            let mut matches = cursor.matches(&c.query, item, text);
            while let Some(m) = matches.next() {
                for cap in m.captures() {
                    let what = names.get(cap.index as usize).copied().unwrap_or("");
                    if what == "group" && cap.node.start_byte() >= item.start_byte() && cap.node.end_byte() <= item.end_byte() {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// The `@name` capture texts of the first match whose `@test` node spans
    /// `item`, or `None` when no query marks it. The queries run over the
    /// tree that holds the item, rooted at its parent, so a pattern can
    /// match the attribute siblings of a Rust item. A decorated Python test
    /// is marked by its definition, which the decorators wrap. With
    /// `shipped_only` set, project queries are skipped.
    fn marked_names(&self, item: Node, text: &[u8], shipped_only: bool) -> Option<Vec<String>> {
        // A statement wrapper (`expression_statement` in JavaScript) ends in
        // a `;` the call inside does not cover.
        let span_end = match text.get(item.end_byte().saturating_sub(1)..item.end_byte()) {
            Some(b";") => item.end_byte() - 1,
            _ => item.end_byte(),
        };
        let target = super::units::undecorated(item);
        let root = item.parent().unwrap_or(item);
        let mut cursor = QueryCursor::new();
        for c in self.compiled.iter().filter(|c| c.shipped || !shipped_only) {
            let names = c.query.capture_names();
            let mut matches = cursor.matches(&c.query, root, text);
            while let Some(m) = matches.next() {
                let mut marked = false;
                let mut found: Vec<String> = Vec::new();
                for cap in m.captures() {
                    let what = names.get(cap.index as usize).copied().unwrap_or("");
                    match what {
                        "test" => marked = cap.node.start_byte() == target.start_byte() && cap.node.end_byte() == span_end,
                        "name" => found.push(cap.node.utf8_text(text).ok()?.to_string()),
                        _ => {}
                    }
                }
                if marked {
                    return Some(found);
                }
            }
        }
        None
    }
}

/// Validates one project query file against `language`: a readable file that
/// compiles, or an error that names the path. `rvow check` reports a failure
/// as a warning; the drift check keeps the file out of its naming instead.
pub fn validate(language: &Language, path: &Path) -> Result<()> {
    let source = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    match Query::new(language, &source) {
        Ok(_) => Ok(()),
        Err(e) => Err(Error::new("query", format!("{}: {e}", path.display()), "fix the query, or remove test_queries for this language")),
    }
}

/// The config key of the project query files.
pub const CONFIG_KEY: &str = "test_queries";
