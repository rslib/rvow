//! Syntax units: the pieces a protected test file is compared by. A unit is
//! an item together with the attribute, decorator and comment siblings right
//! before it, so `#[test] fn a` is one unit and removing the attribute
//! changes it. A body container that is not a function splits into a header
//! unit plus one unit per body item: a Rust module, a Python class (decorated
//! or not), a C++ namespace, or a call a shipped query marks `@group`, such as
//! `describe(...)`. A function body never splits, so a statement added inside
//! a test changes that test's unit. A unit keeps its container: its key starts
//! with the header forms of the containers around it, so moving a test into
//! another describe, class or module changes its key. The compare counts:
//! each old key must appear in the new text at least as many times as before.
//! A new unit is free only when a shipped query marks it a test with a name the
//! old text does not use, and every attribute above it is a test attribute.
//! A helper, a hook or an import is drift, and a project query only names
//! tests in drift items.

use std::collections::BTreeMap;
use std::path::Path;

use tree_sitter::Node;

use crate::ledger;
use crate::syntax::canonical;
use crate::syntax::grammar::{self, Session};
use crate::syntax::queries;

/// What the unit compare decided about a protected test file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Units {
    /// Every old unit keeps its form as many times as before, and every
    /// new unit is free.
    Preserved,
    /// At least one old unit is gone or changed, or a new unit is not
    /// free. The names are the tests a query marks among the changed old
    /// units; the caller prints them when there are any and keeps today's
    /// item text when there are none.
    Changed(Vec<String>),
}

/// Whether a node is function-like: its body is one test or helper, not a
/// list of siblings to split.
fn is_function_like(kind: &str) -> bool {
    kind.contains("function") || kind == "method_definition" || kind == "lambda_expression"
}

/// Whether a sibling is part of the unit of the item that follows it: a
/// Rust attribute, a decorator, or a comment.
fn is_prefix(kind: &str) -> bool {
    kind == "attribute_item" || kind == "decorator" || kind.ends_with("comment")
}

/// One unit: the prefix siblings right before an item, plus the item.
/// `omit` is the body span a split container header leaves out of its
/// form.
struct Unit<'a> {
    /// The item the queries mark and name.
    item: Node<'a>,
    /// The prefix siblings plus the item, in source order.
    nodes: Vec<Node<'a>>,
    omit: Option<(usize, usize)>,
    /// The forms of the containers around the unit, outermost first.
    ctx: String,
}

impl Unit<'_> {
    /// The canonical form of the unit: the container forms, then every node
    /// of the run, the item minus its body when the unit is a container
    /// header.
    fn form(&self, bytes: &[u8]) -> String {
        let mut out = self.ctx.clone();
        for node in &self.nodes {
            if node.id() == self.item.id() {
                match self.omit {
                    Some((start, end)) => canonical::write_node_omitting(*node, start..end, bytes, &mut out),
                    None => canonical::write_node(*node, bytes, &mut out),
                }
            } else {
                canonical::write_node(*node, bytes, &mut out);
            }
        }
        out
    }

    /// Where the unit starts in its file, its first prefix sibling included.
    fn start(&self) -> usize {
        self.nodes.first().map_or(0, |node| node.start_byte())
    }
}

/// The units of one body: each item with the attribute, decorator and
/// comment siblings right before it. A prefix run that no item follows is
/// its own unit, so a removed or changed trailing attribute is still drift.
/// `ctx` is the form of the container the body belongs to.
fn push_units<'a>(parent: Node<'a>, named: Option<&queries::Queries>, bytes: &[u8], ctx: &str, out: &mut Vec<Unit<'a>>) {
    let mut nodes: Vec<Node> = Vec::new();
    for i in 0..parent.named_child_count() {
        let Some(child) = parent.named_child(i as u32) else { continue };
        nodes.push(child);
        if is_prefix(child.kind()) {
            continue;
        }
        push_item(std::mem::take(&mut nodes), named, bytes, ctx, out);
    }
    if let Some(item) = nodes.last().copied() {
        out.push(Unit { item, nodes, omit: None, ctx: ctx.to_string() });
    }
}

/// One item as a unit: a container that is not a function splits into a
/// header unit plus one unit per body item. The body items key on the form
/// of the header.
fn push_item<'a>(nodes: Vec<Node<'a>>, named: Option<&queries::Queries>, bytes: &[u8], ctx: &str, out: &mut Vec<Unit<'a>>) {
    let Some(item) = nodes.last().copied() else { return };
    let body = container_body(item, named, bytes);
    let omit = body.map(|body| (body.start_byte(), body.end_byte()));
    let unit = Unit { item, nodes, omit, ctx: ctx.to_string() };
    let header = unit.form(bytes);
    out.push(unit);
    if let Some(body) = body {
        push_units(body, named, bytes, &header, out);
    }
}

/// The units of a parsed file, in source order. A file whose root holds no
/// named children is its own one unit.
fn unit_nodes<'a>(root: Node<'a>, named: Option<&queries::Queries>, bytes: &[u8]) -> Vec<Unit<'a>> {
    let mut out = Vec::new();
    push_units(root, named, bytes, "", &mut out);
    if out.is_empty() {
        out.push(Unit { item: root, nodes: vec![root], omit: None, ctx: String::new() });
    }
    out
}

/// The body a container splits by, or `None` when the item stays one unit.
/// A function-like item never splits: a statement added inside a test
/// changes that test's unit. A decorated class splits like its class, and
/// its header keeps the decorators. A group call splits by the body of its
/// callback, the first function-like descendant.
fn container_body<'a>(item: Node<'a>, named: Option<&queries::Queries>, bytes: &[u8]) -> Option<Node<'a>> {
    let def = undecorated(item);
    if is_function_like(def.kind()) {
        return None;
    }
    match def.kind() {
        "mod_item" | "class_definition" | "namespace_definition" => def.child_by_field_name("body"),
        _ => named.filter(|q| q.group(item, bytes)).and_then(|_| group_body(item)),
    }
}

fn group_body<'a>(item: Node<'a>) -> Option<Node<'a>> {
    first_function_like(item)?.child_by_field_name("body")
}

fn first_function_like<'a>(node: Node<'a>) -> Option<Node<'a>> {
    if is_function_like(node.kind()) {
        return Some(node);
    }
    for i in 0..node.named_child_count() {
        let Some(child) = node.named_child(i as u32) else { continue };
        if let Some(found) = first_function_like(child) {
            return Some(found);
        }
    }
    None
}

/// The definition an item holds: a Python decorated_definition wraps the
/// function or class its decorators mark. Any other item is its own
/// definition.
pub fn undecorated(item: Node) -> Node {
    if item.kind() != "decorated_definition" {
        return item;
    }
    for i in 0..item.named_child_count() {
        let Some(child) = item.named_child(i as u32) else { continue };
        if child.kind() != "decorator" {
            return child;
        }
    }
    item
}

/// Whether `word` occurs in `text` as a whole word: no letter, digit or
/// underscore on either side. Identifiers, words and string content all
/// count.
fn has_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
    })
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether a node above a test is allowed: a Rust test attribute, a pytest
/// mark, or a comment. A comment is allowed only after the first old unit of
/// the file.
fn test_prefix(node: Node, bytes: &[u8], before_first_old: bool) -> bool {
    let Ok(text) = node.utf8_text(bytes) else { return false };
    match node.kind() {
        "attribute_item" => text.starts_with("#[test]") || text.starts_with("#[tokio::test") || text.starts_with("#[should_panic"),
        "decorator" => text.starts_with("@pytest.mark."),
        kind => kind.ends_with("comment") && !before_first_old,
    }
}

/// Whether a new unit is free: a shipped query marks it a test, its name is
/// one the old text does not use, and every node above it is a test prefix.
/// `TestMain` and `init` never free, whatever a query says.
fn free(unit: &Unit, named: Option<&queries::Queries>, old: &str, bytes: &[u8], before_first_old: bool) -> bool {
    let Some(name) = named.and_then(|q| q.shipped_name(unit.item, bytes)) else { return false };
    let short = name.rsplit('.').next().unwrap_or("");
    if short.is_empty() || matches!(short, "TestMain" | "init") || has_word(old, short) {
        return false;
    }
    let Some((_, prefix)) = unit.nodes.split_last() else { return false };
    prefix.iter().all(|node| test_prefix(*node, bytes, before_first_old))
        && decorators(unit.item).all(|node| test_prefix(node, bytes, before_first_old))
}

/// The decorators a Python decorated_definition holds as children. They
/// are not prefix siblings, so `free` checks them here: a
/// `@pytest.fixture(autouse=True)` named `test_*` is not a free test.
fn decorators(item: Node) -> impl Iterator<Item = Node> {
    let held = if item.kind() == "decorated_definition" { item.named_child_count() } else { 0 };
    (0..held).filter_map(move |i| item.named_child(i as u32)).filter(|child| child.kind() == "decorator")
}

/// Which new units keep an old form: the first new copies of each form, up
/// to the number of old copies of it.
fn kept_units(forms: &[String], old: &BTreeMap<&String, usize>) -> Vec<bool> {
    let mut used: BTreeMap<&String, usize> = BTreeMap::new();
    forms
        .iter()
        .map(|form| {
            let seen = used.entry(form).or_default();
            *seen += 1;
            *seen <= old.get(form).copied().unwrap_or(0)
        })
        .collect()
}

/// The unit compare of a protected test file: `Some` when the compare
/// decided, `None` when the caller keeps its own rule. `None` covers a file
/// with no pinned grammar, a grammar that is not in the cache, a fetch that
/// failed inside the retry window, and a text that does not parse cleanly.
/// `top` resolves the project query files of `test_queries`, keyed by
/// language; the queries decide which containers split and which new units
/// are free, so they load before the compare. `session` is the grammar
/// session of the whole drift call, shared by every file. The compare stays
/// offline: it reads the cache only and never downloads, so it honors
/// `RVOW_GRAMMAR_OFFLINE` by construction.
pub fn preserved(
    rel: &str,
    before: &str,
    now: &str,
    home: Option<&Path>,
    top: Option<&Path>,
    test_queries: &BTreeMap<String, String>,
    session: &mut Option<Session>,
) -> Option<Units> {
    let home = home?;
    let entry = grammar::for_file(rel)?;
    if grammar::cached(home, entry).is_none() || grammar::fetch_recently_failed(home, entry, ledger::now_ms(), grammar::FETCH_RETRY_WINDOW)
    {
        return None;
    }
    if session.is_none() {
        *session = Some(Session::new().ok()?);
    }
    let session = session.as_mut()?;
    let base = std::env::var(grammar::URL_VAR).ok().filter(|v| !v.is_empty());
    let mut parse = |text: &str| -> Option<_> {
        let tree = session.parse(home, entry, base.as_deref(), text).ok()?;
        (!tree.root_node().has_error()).then_some(tree)
    };
    let before_tree = parse(before)?;
    let now_tree = parse(now)?;
    let language = session.language(entry)?;
    let named = top.map(|top| queries::load(entry.name.as_str(), &language, top, test_queries));
    let old = unit_nodes(before_tree.root_node(), named.as_ref(), before.as_bytes());
    let new = unit_nodes(now_tree.root_node(), named.as_ref(), now.as_bytes());
    let old_forms: Vec<String> = old.iter().map(|unit| unit.form(before.as_bytes())).collect();
    let new_forms: Vec<String> = new.iter().map(|unit| unit.form(now.as_bytes())).collect();
    fn count(forms: &[String]) -> BTreeMap<&String, usize> {
        let mut out = BTreeMap::new();
        for form in forms {
            *out.entry(form).or_default() += 1;
        }
        out
    }
    let old_counts = count(&old_forms);
    let new_counts = count(&new_forms);
    let mut names: Vec<String> = Vec::new();
    let mut missing = false;
    for (unit, form) in old.iter().zip(&old_forms) {
        let kept = new_counts.get(form).copied().unwrap_or(0);
        if kept >= old_counts.get(form).copied().unwrap_or(0) {
            continue;
        }
        missing = true;
        if let Some(q) = &named
            && let Some(name) = q.name(unit.item, before.as_bytes())
            && !names.contains(&name)
        {
            names.push(name);
        }
    }
    let kept = kept_units(&new_forms, &old_counts);
    let first_old = new.iter().zip(&kept).find(|(_, kept)| **kept).map_or(usize::MAX, |(unit, _)| unit.start());
    let added =
        new.iter().zip(&kept).any(|(unit, kept)| !kept && !free(unit, named.as_ref(), before, now.as_bytes(), unit.start() < first_old));
    if !missing && !added {
        return Some(Units::Preserved);
    }
    Some(Units::Changed(names))
}
