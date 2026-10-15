//! Framework queries: the tree-sitter queries that mark a unit as a test,
//! name it, or mark a call as a group container. A shipped query per pinned
//! language names the common frameworks; a project adds its own per language
//! in `config.toml`. A shipped query is trusted. A project query is trusted
//! only when its file was protected at accept and is unchanged. A missing or
//! bad query file never decides a check: it only leaves the drift item
//! unnamed.

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
    ("c", include_str!("queries/c.scm")),
    ("go", include_str!("queries/go.scm")),
    ("java", include_str!("queries/java.scm")),
    ("kotlin", include_str!("queries/kotlin.scm")),
    ("c_sharp", include_str!("queries/c_sharp.scm")),
    ("ruby", include_str!("queries/ruby.scm")),
    ("php", include_str!("queries/php.scm")),
];

/// One compiled query. A trusted query can decide a split or a free unit:
/// the one `SHIPPED` holds, or a project query whose file was protected at
/// accept. Any other project query only names tests.
struct Compiled {
    query: Query,
    trusted: bool,
}

/// The queries of one language: the shipped source first, then the project's.
pub struct Queries {
    compiled: Vec<Compiled>,
}

/// Compiles the shipped query of `lang` and every project file `config`
/// names for it. `top` resolves the repo-relative project paths. `protected`
/// holds the files protected at accept, keyed by path, with the hash of each:
/// a project query file is trusted when it is there with its hash. A query
/// source that fails to compile is a problem, never a check failure: the
/// caller keeps the queries that did compile and warns.
pub fn load(
    lang: &str,
    language: &Language,
    top: &Path,
    config: &BTreeMap<String, String>,
    protected: Option<&BTreeMap<String, String>>,
) -> Queries {
    let mut compiled = Vec::new();
    if let Some(source) = SHIPPED.iter().find(|(name, _)| *name == lang).map(|(_, s)| *s)
        && let Ok(query) = Query::new(language, source)
    {
        compiled.push(Compiled { query, trusted: true });
    }
    let Some(rel) = config.get(lang) else { return Queries { compiled } };
    let file = top.join(rel);
    let Ok(source) = std::fs::read_to_string(&file) else {
        return Queries { compiled };
    };
    // A query that fails to compile is a problem rvow check reports; the
    // drift check keeps the queries that did compile.
    if let Ok(query) = Query::new(language, &source) {
        let trusted = protected
            .and_then(|files| files.get(crate::vow::query_key(rel)?.as_str()))
            .is_some_and(|hash| *hash == crate::vow::sha256_hex(source.as_bytes()));
        compiled.push(Compiled { query, trusted });
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

    /// The name of the test `item` holds from a trusted query, the last
    /// `@name` capture: `Name` for a GoogleTest `Suite.Name`. `None` when no
    /// trusted query marks it or it has no name. Only trusted queries can
    /// make a new unit free.
    pub fn trusted_test_name(&self, item: Node, text: &[u8]) -> Option<String> {
        self.marked_names(item, text, true)?.pop()
    }

    /// Whether a trusted query marks the whole of `item` as a group. A
    /// function-like item splits only this way: a C `main` that holds
    /// `RUN_TEST` lines.
    pub fn group_spanning(&self, item: Node, text: &[u8]) -> bool {
        self.group_where(item, text, |cap| cap.start_byte() == item.start_byte() && cap.end_byte() == item.end_byte())
    }

    /// Whether a trusted query marks a call inside `item` as a group: a
    /// container that splits into a header unit plus one unit per body item,
    /// such as a JavaScript `describe(...)`. An untrusted project query never splits.
    pub fn group(&self, item: Node, text: &[u8]) -> bool {
        self.group_where(item, text, |cap| cap.start_byte() >= item.start_byte() && cap.end_byte() <= item.end_byte())
    }

    fn group_where(&self, item: Node, text: &[u8], within: impl Fn(Node) -> bool) -> bool {
        let mut cursor = QueryCursor::new();
        for c in self.compiled.iter().filter(|c| c.trusted) {
            let names = c.query.capture_names();
            let mut matches = cursor.matches(&c.query, item, text);
            while let Some(m) = matches.next() {
                for cap in m.captures() {
                    let what = names.get(cap.index as usize).copied().unwrap_or("");
                    if what == "group" && within(cap.node) {
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
    /// `trusted_only` set, untrusted project queries are skipped.
    fn marked_names(&self, item: Node, text: &[u8], trusted_only: bool) -> Option<Vec<String>> {
        // A statement wrapper (`expression_statement` in JavaScript) ends in
        // a `;` the call inside does not cover.
        let span_end = match text.get(item.end_byte().saturating_sub(1)..item.end_byte()) {
            Some(b";") => item.end_byte() - 1,
            _ => item.end_byte(),
        };
        let target = super::units::undecorated(item);
        let root = item.parent().unwrap_or(item);
        let mut cursor = QueryCursor::new();
        for c in self.compiled.iter().filter(|c| c.trusted || !trusted_only) {
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
