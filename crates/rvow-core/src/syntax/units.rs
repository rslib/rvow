//! Syntax units: the pieces a protected test file is compared by. A unit is
//! an item together with the attribute, decorator and comment siblings right
//! before it, so `#[test] fn a` is one unit and removing the attribute
//! changes it. A body container that is not a function splits into a header
//! unit plus one unit per body item: a Rust module, a Python class (decorated
//! or not), a C++ namespace, or a call a trusted query marks `@group`, such as
//! `describe(...)`. A function body never splits, so a statement added inside
//! a test changes that test's unit. A unit keeps its container: its key starts
//! with the header forms of the containers around it, so moving a test into
//! another describe, class or module changes its key. The compare counts:
//! each old key must appear in the new text at least as many times as before.
//! A new unit is free only when a trusted query marks it a test with a name the
//! old text does not use, every attribute above it is a test attribute, its
//! body calls no function that ends the process, and it holds no argument
//! that focuses the run on it. A doctest or Catch2 macro such as
//! `TEST_CASE("adds") { ... }` parses as a header with a missing `;` and a
//! block; the pair is one unit, and a new case is not free when its header
//! skips it. A helper, a hook or an import is drift. A project query is
//! trusted only when its file was protected at accept and is unchanged;
//! any other project query only names tests in drift items. A new
//! `mod NAME;` line of a Rust file is free too when `free_mod` accepts it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use tree_sitter::Node;

use crate::ledger;
use crate::syntax::canonical;
use crate::syntax::grammar::{self, Session};
use crate::syntax::queries;

/// The repository a unit compare reads. `top` resolves query files and the
/// module files of new `mod` lines. `protected` holds the files protected at
/// accept; without it no `mod` line is free.
#[derive(Clone, Copy)]
pub struct Repo<'a> {
    pub top: &'a Path,
    pub protected: Option<&'a BTreeMap<String, String>>,
}

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
    /// The block right after a macro header that does not end in `;`, such
    /// as the `{ ... }` of `TEST_CASE("adds")`. tree-sitter-cpp parses the
    /// pair as two siblings, so the block belongs to the unit of its header.
    tail: Option<Node<'a>>,
    /// The forms of the containers around the unit, outermost first.
    ctx: String,
    /// The unit is a statement of a function body that splits, such as a C
    /// `main` that holds `RUN_TEST`. Statements run in order, so the compare
    /// keeps their order.
    ordered: bool,
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
        if let Some(tail) = self.tail.filter(|_| self.omit.is_none()) {
            canonical::write_node(tail, bytes, &mut out);
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
fn push_units<'a>(parent: Node<'a>, named: Option<&queries::Queries>, bytes: &[u8], ctx: &str, ordered: bool, out: &mut Vec<Unit<'a>>) {
    let mut nodes: Vec<Node> = Vec::new();
    let count = parent.named_child_count();
    let mut i = 0;
    while i < count {
        let Some(child) = parent.named_child(i as u32) else { break };
        i += 1;
        nodes.push(child);
        if is_prefix(child.kind()) {
            continue;
        }
        let tail = macro_body(child);
        if tail.is_some() {
            i += 1;
        }
        push_item(std::mem::take(&mut nodes), tail, named, bytes, ctx, ordered, out);
    }
    if let Some(item) = nodes.last().copied() {
        out.push(Unit { item, nodes, omit: None, tail: None, ctx: ctx.to_string(), ordered });
    }
}

/// One item as a unit: a container that is not a function splits into a
/// header unit plus one unit per body item. The body items key on the form
/// of the header.
fn push_item<'a>(
    nodes: Vec<Node<'a>>,
    tail: Option<Node<'a>>,
    named: Option<&queries::Queries>,
    bytes: &[u8],
    ctx: &str,
    ordered: bool,
    out: &mut Vec<Unit<'a>>,
) {
    let Some(item) = nodes.last().copied() else { return };
    let body = match tail {
        Some(tail) => named.filter(|q| q.group(item, bytes)).map(|_| tail),
        None => container_body(item, named, bytes),
    };
    let omit = body.map(|body| (body.start_byte(), body.end_byte()));
    let unit = Unit { item, nodes, omit, tail, ctx: ctx.to_string(), ordered };
    let header = unit.form(bytes);
    out.push(unit);
    if let Some(body) = body {
        let statements = tail.is_none() && is_function_like(undecorated(item).kind());
        push_units(body, named, bytes, &header, statements, out);
    }
}

/// The block of a macro header: tree-sitter-cpp parses `TEST_CASE("adds") { ... }`
/// as an expression statement whose `;` is missing, then a compound
/// statement. `None` for any other item.
fn macro_body(item: Node) -> Option<Node> {
    if item.kind() != "expression_statement" {
        return None;
    }
    let last = item.child(item.child_count().checked_sub(1)?)?;
    let body = item.next_named_sibling()?;
    (last.is_missing() && last.kind() == ";" && body.kind() == "compound_statement").then_some(body)
}

/// Whether every named node of `root` that holds bytes lies in the span of
/// some unit. Then every byte of the file belongs to the form of some unit,
/// an ERROR region included, so a change anywhere in a file with parse
/// errors is a change in some unit form and is drift.
fn covers_every_node(root: Node, units: &[Unit]) -> bool {
    let mut spans: Vec<(usize, usize)> = units
        .iter()
        .flat_map(|unit| {
            unit.nodes
                .iter()
                .map(|node| (node.start_byte(), node.end_byte()))
                .chain(unit.tail.map(|tail| (tail.start_byte(), tail.end_byte())))
        })
        .collect();
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(spans.len());
    for (start, end) in spans {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    let covered = |start: usize, end: usize| {
        let mut lo = 0;
        let mut hi = merged.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            if merged[mid].0 <= start { lo = mid + 1 } else { hi = mid }
        }
        lo > 0 && merged[lo - 1].1 >= end
    };
    let mut stack: Vec<Node> = (0..root.named_child_count()).filter_map(|i| root.named_child(i as u32)).collect();
    while let Some(node) = stack.pop() {
        if node.end_byte() > node.start_byte() && !covered(node.start_byte(), node.end_byte()) {
            return false;
        }
        stack.extend((0..node.named_child_count()).filter_map(|i| node.named_child(i as u32)));
    }
    true
}

/// The units of a parsed file, in source order. A file whose root holds no
/// named children is its own one unit.
fn unit_nodes<'a>(root: Node<'a>, named: Option<&queries::Queries>, bytes: &[u8]) -> Vec<Unit<'a>> {
    let mut out = Vec::new();
    push_units(root, named, bytes, "", false, &mut out);
    if out.is_empty() {
        out.push(Unit { item: root, nodes: vec![root], omit: None, tail: None, ctx: String::new(), ordered: false });
    }
    out
}

/// The body a container splits by, or `None` when the item stays one unit.
/// A function-like item never splits: a statement added inside a test
/// changes that test's unit, unless a trusted query marks the whole function
/// a group, as it does for a C `main` that holds `RUN_TEST`. A decorated class splits like its class, and
/// its header keeps the decorators. A group call splits by the body of its
/// callback, the first function-like descendant.
fn container_body<'a>(item: Node<'a>, named: Option<&queries::Queries>, bytes: &[u8]) -> Option<Node<'a>> {
    let def = undecorated(item);
    if is_function_like(def.kind()) {
        return named.filter(|q| q.group_spanning(item, bytes)).and_then(|_| def.child_by_field_name("body"));
    }
    match def.kind() {
        "mod_item" | "class_definition" | "namespace_definition" | "class_declaration" | "namespace_declaration" => {
            def.child_by_field_name("body").or_else(|| named_child_of_kind(def, "class_body"))
        }
        _ => named.filter(|q| q.group(item, bytes)).and_then(|_| group_body(item)),
    }
}

fn named_child_of_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    (0..node.named_child_count()).filter_map(|i| node.named_child(i as u32)).find(|child| child.kind() == kind)
}

fn group_body<'a>(item: Node<'a>) -> Option<Node<'a>> {
    first_function_like(item)?.child_by_field_name("body")
}

fn first_function_like<'a>(node: Node<'a>) -> Option<Node<'a>> {
    if is_function_like(node.kind()) || node.kind() == "do_block" {
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

/// Annotation and attribute names that mark a test or feed it data, without
/// switching it off: JUnit, xUnit, NUnit, MSTest and PHPUnit.
const TEST_ANNOTATIONS: &[&str] = &[
    "Test",
    "ParameterizedTest",
    "RepeatedTest",
    "TestFactory",
    "ValueSource",
    "CsvSource",
    "EnumSource",
    "NullSource",
    "EmptySource",
    "NullAndEmptySource",
    "Fact",
    "Theory",
    "TestCase",
    "TestMethod",
    "InlineData",
];

/// Whether every attribute inside an attribute list is a test attribute
/// that does not skip the test.
fn test_attribute_list(node: Node, bytes: &[u8]) -> bool {
    let mut attributes = Vec::new();
    collect_kind(node, "attribute", &mut attributes);
    !attributes.is_empty()
        && attributes.iter().all(|attribute| {
            let name = attribute.named_child(0).and_then(|name| name.utf8_text(bytes).ok()).unwrap_or("");
            let text = attribute.utf8_text(bytes).unwrap_or("");
            TEST_ANNOTATIONS.contains(&name) && !text.contains("Skip") && !text.contains("Ignore")
        })
}

fn collect_kind<'a>(node: Node<'a>, kind: &str, out: &mut Vec<Node<'a>>) {
    if node.kind() == kind {
        out.push(node);
        return;
    }
    for i in 0..node.named_child_count() {
        if let Some(child) = node.named_child(i as u32) {
            collect_kind(child, kind, out);
        }
    }
}

/// Whether a Java or Kotlin annotation is a test annotation that does not
/// skip the test: `@Test`, `@org.junit.Test`, `@ParameterizedTest(...)`.
fn test_annotation(text: &str) -> bool {
    let name: String = text.trim_start_matches('@').chars().take_while(|c| is_word_char(*c) || *c == '.').collect();
    TEST_ANNOTATIONS.contains(&name.rsplit('.').next().unwrap_or("")) && !text.contains("Skip") && !text.contains("Ignore")
}

/// The annotations an item holds itself: the `modifiers` of a Java or
/// Kotlin declaration, or the attribute lists of a C# or PHP method. They
/// are not prefix siblings, so `free` checks them here.
fn held_annotations(item: Node) -> Vec<Node> {
    let mut out = Vec::new();
    for i in 0..item.named_child_count() {
        let Some(child) = item.named_child(i as u32) else { continue };
        match child.kind() {
            "attribute_list" => out.push(child),
            "modifiers" => {
                for j in 0..child.named_child_count() {
                    if let Some(held) = child.named_child(j as u32)
                        && matches!(held.kind(), "marker_annotation" | "annotation")
                    {
                        out.push(held);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Whether a node above a test is allowed: a Rust test attribute, a pytest
/// mark, a JUnit, xUnit, NUnit, MSTest or PHPUnit test annotation, or a
/// comment. A comment is allowed only after the first old unit of the file.
fn test_prefix(node: Node, bytes: &[u8], before_first_old: bool) -> bool {
    let Ok(text) = node.utf8_text(bytes) else { return false };
    match node.kind() {
        "marker_annotation" | "annotation" => test_annotation(text),
        "attribute_list" => test_attribute_list(node, bytes),
        "attribute_item" => text.starts_with("#[test]") || text.starts_with("#[tokio::test") || text.starts_with("#[should_panic"),
        "decorator" => text.starts_with("@pytest.mark."),
        kind => kind.ends_with("comment") && !before_first_old,
    }
}

/// Names that end the process whatever they are called on: `_exit`,
/// `exitProcess`, `Environment.FailFast`, `Runtime.halt` and the like.
const EXIT_ALWAYS: &[&str] = &["_exit", "_Exit", "quick_exit", "exitProcess", "reallyExit", "FailFast", "halt", "die", "Goexit"];

/// Names that end the process when called bare or on a process-like
/// receiver (`EXIT_RECEIVERS`), so that `controller.abort()` stays free.
const EXIT_ON_RECEIVER: &[&str] = &["exit", "exit!", "Exit", "abort", "quit", "terminate"];

/// Receivers whose `exit`, `abort` or `terminate` ends the process,
/// compared without case: `std::process`, `sys`, `System`, `Kernel`.
const EXIT_RECEIVERS: &[&str] = &["process", "sys", "os", "system", "environment", "kernel", "std", "libc", "pytest", "runtime", "syscall"];

/// Calls that fetch a function by name and hand it back, so an argument that
/// names an exit ends the process: `getattr(sys, "exit")`, `send(:exit)`.
const NAME_LOOKUPS: &[&str] = &["getattr", "send", "public_send", "__send__", "method", "getMethod", "getDeclaredMethod"];

/// Receivers whose `get` fetches a member by name, so `Reflect.get(obj,
/// "exit")` ends the process. Compared without case.
const NAME_LOOKUP_GETS: &[&str] = &["reflect", "object", "module", "class"];

/// Node kinds that index a container by a key: a Python subscript, a JS
/// subscript expression, and the Ruby and PHP element access.
fn is_subscript(kind: &str) -> bool {
    matches!(kind, "subscript" | "subscript_expression" | "element" | "element_access" | "offset_expression")
}

/// Node kinds that wrap string or symbol text without adding meaning.
fn is_text_node(kind: &str) -> bool {
    matches!(
        kind,
        "string"
            | "simple_symbol"
            | "symbol"
            | "string_content"
            | "string_fragment"
            | "string_array"
            | "encapsed_string"
            | "template_string"
    )
}

/// Whether the text of a string or symbol leaf names a function that ends the
/// process, without its quotes or a Ruby `:`.
fn names_exit(text: &str) -> bool {
    let text = text.trim_matches(['"', '\'', '`']);
    let text = text.strip_prefix(':').unwrap_or(text);
    EXIT_ALWAYS.contains(&text) || EXIT_ON_RECEIVER.contains(&text)
}

/// Whether a call fetches a function by name: `getattr(...)`, `send(...)` or
/// `get` on a receiver that looks members up.
fn looks_up_by_name(call: Node, bytes: &[u8]) -> bool {
    let function = ["function", "method", "name", "type"]
        .into_iter()
        .find_map(|field| call.child_by_field_name(field))
        .or_else(|| call.named_child(0));
    let Some(function) = function else { return false };
    let Ok(text) = function.utf8_text(bytes) else { return false };
    let text = text.trim();
    let (owner, name) = match text.rsplit_once('.') {
        Some((owner, name)) => (owner.to_string(), name),
        None => (receiver(bytes, function.start_byte()).unwrap_or_default(), text),
    };
    let first =
        call.child_by_field_name("arguments").and_then(|args| args.named_child(0)).and_then(|arg| arg.utf8_text(bytes).ok()).unwrap_or("");
    if name == "get" {
        let lower = owner.to_ascii_lowercase();
        return NAME_LOOKUP_GETS.iter().any(|known| lower.ends_with(known)) && process_like(first);
    }
    if !NAME_LOOKUPS.contains(&name) {
        return false;
    }
    // `getattr(obj, "exit")` looks up on its first argument; `send(:exit)` and
    // `getMethod("exit")` on their receiver, and a bare `send` on self, which
    // reaches Kernel in Ruby.
    if name == "getattr" { process_like(first) } else { owner.is_empty() || process_like(&owner) }
}

/// Whether an object's text looks like the process or a holder of its exit
/// functions: `sys`, `os`, `process`, `Kernel`, `System.class`, `Runtime`.
fn process_like(text: &str) -> bool {
    let text = text.trim().to_ascii_lowercase();
    let last = text.trim_end_matches(".class").rsplit(['.', ':']).next().unwrap_or("");
    // A module loaded by name, such as `__import__("os")`, can be any module.
    EXIT_RECEIVERS.contains(&last) || text.contains("process") || text.contains("runtime") || text.contains("import")
}

/// Whether a string or symbol leaf reaches an exit function where it stands:
/// the key of a subscript or element access, as in `process["exit"]` or
/// `hash[:exit]`, or an argument of a call that looks a function up by name,
/// as in `getattr(sys, "exit")` or `Kernel.send(:exit)`.
fn exit_by_name(leaf: Node, bytes: &[u8]) -> bool {
    if !leaf.utf8_text(bytes).is_ok_and(names_exit) {
        return false;
    }
    let mut node = leaf;
    while let Some(parent) = node.parent() {
        if !is_text_node(parent.kind()) {
            break;
        }
        node = parent;
    }
    let Some(holder) = node.parent() else { return false };
    if is_subscript(holder.kind()) {
        let object = ["value", "object", "receiver"]
            .into_iter()
            .find_map(|field| holder.child_by_field_name(field))
            .or_else(|| holder.named_child(0))
            .filter(|object| object.id() != node.id());
        return object.and_then(|object| object.utf8_text(bytes).ok()).is_some_and(process_like);
    }
    matches!(holder.kind(), "argument_list" | "arguments") && holder.parent().is_some_and(|call| looks_up_by_name(call, bytes))
}

/// Whether a leaf is string or comment text, which never calls anything.
fn is_text_leaf(kind: &str) -> bool {
    ["string", "content", "fragment", "comment", "char", "escape", "heredoc", "regex"].iter().any(|part| kind.contains(part))
}

/// The receiver segment just before byte `at`, when the name there follows
/// `.`, `::`, `?.`, `->` or `\\`: `process` for `std::process::exit`,
/// `getRuntime()` for `Runtime.getRuntime().halt`. `None` for a bare name.
fn receiver(bytes: &[u8], at: usize) -> Option<String> {
    let before = String::from_utf8_lossy(bytes.get(..at).unwrap_or_default());
    let before = before.trim_end();
    let rest = ["::", "?.", "->", ".", "\\"].iter().find_map(|sep| before.strip_suffix(sep))?;
    let start = rest
        .rfind(|c: char| c.is_whitespace() || matches!(c, ';' | '{' | ',' | '=' | '.' | ':' | '>' | '!' | '&' | '|'))
        .map_or(0, |i| i + 1);
    Some(rest[start..].to_string())
}

/// Whether the name ending at byte `end` is called: an open parenthesis
/// follows, maybe after `)` or `?.`, or a `.call`, `.apply` or `.bind`.
fn called_at(bytes: &[u8], end: usize) -> bool {
    let after = String::from_utf8_lossy(bytes.get(end..).unwrap_or_default());
    let after = after.trim_start_matches(|c: char| c.is_whitespace() || c == ')');
    let after = after.strip_prefix("?.").unwrap_or(after);
    after.starts_with('(') || [".call", ".apply", ".bind"].iter().any(|tail| after.starts_with(tail))
}

/// Whether a Ruby or PHP bare name is a call: it is not assigned to and it
/// is no PHP variable. Both languages call a method without parentheses.
fn bare_call(leaf: Node, bytes: &[u8]) -> bool {
    let after = String::from_utf8_lossy(bytes.get(leaf.end_byte()..).unwrap_or_default());
    let after = after.trim_start();
    let assigned = after.starts_with('=') && !after.starts_with("==") && !after.starts_with("=>");
    !assigned && !leaf.parent().is_some_and(|parent| matches!(parent.kind(), "variable_name" | "parameters" | "block_parameters"))
}

/// Whether a leaf names a function that ends the process and is called
/// there. A string, a comment or a variable named exit is no call.
fn exit_leaf(leaf: Node, bytes: &[u8], lang: &str) -> bool {
    if is_text_leaf(leaf.kind()) || leaf.kind().contains("symbol") {
        return exit_by_name(leaf, bytes);
    }
    let Ok(text) = leaf.utf8_text(bytes) else { return false };
    let text = text.strip_prefix('\\').unwrap_or(text);
    // A leading `::` names the global function, like a bare name.
    let receiver = receiver(bytes, leaf.start_byte()).filter(|r| !r.is_empty());
    let c_like = matches!(lang, "c" | "cpp");
    let fatal = text.starts_with("Fatal") && receiver.as_deref() == Some("log");
    let known = EXIT_ALWAYS.contains(&text)
        || fatal
        || (EXIT_ON_RECEIVER.contains(&text)
            && receiver.as_deref().is_none_or(|r| {
                let r = r.to_ascii_lowercase();
                EXIT_RECEIVERS.iter().any(|known| r.ends_with(known)) || r.contains("process") || r.contains("runtime")
            }))
        || (c_like && receiver.is_none() && matches!(text, "raise" | "kill"));
    // In C and C++ a function name can be taken and called later, as in
    // `auto f = &exit; f(0);`, so naming it counts.
    let named_c = c_like && matches!(text, "exit" | "_exit" | "_Exit" | "quick_exit" | "abort");
    known && (called_at(bytes, leaf.end_byte()) || named_c || (matches!(lang, "ruby" | "php") && bare_call(leaf, bytes)))
}

/// Whether a node ends the process: a PHP `exit` statement, or a leaf that
/// `exit_leaf` accepts. Rust macro arguments are token leaves too, so
/// `dbg!(std::process::exit(0))` counts.
fn calls_exit(node: Node, bytes: &[u8], lang: &str) -> bool {
    if node.kind() == "exit_statement" {
        return true;
    }
    if node.child_count() == 0 {
        return exit_leaf(node, bytes, lang);
    }
    (0..node.child_count()).filter_map(|i| node.child(i)).any(|child| calls_exit(child, bytes, lang))
}

/// Whether a test call holds an argument that makes the runner skip every
/// other test: RSpec `focus: true` or `:focus`, or the node test
/// `{ only: true }` option. A test that is no call has none.
fn focuses_run(item: Node, bytes: &[u8]) -> bool {
    let call = if matches!(item.kind(), "call" | "call_expression") { Some(item) } else { item.named_child(0) };
    call.filter(|call| matches!(call.kind(), "call" | "call_expression"))
        .and_then(|call| call.child_by_field_name("arguments"))
        .is_some_and(|arguments| focus_argument(arguments, bytes))
}

fn focus_argument(node: Node, bytes: &[u8]) -> bool {
    if is_function_like(node.kind()) || matches!(node.kind(), "do_block" | "block" | "statement_block" | "body_statement") {
        return false;
    }
    if node.child_count() == 0 {
        return !is_text_leaf(node.kind())
            && node.utf8_text(bytes).is_ok_and(|text| matches!(text.trim_start_matches(':'), "focus" | "only" | "exclusive"));
    }
    (0..node.child_count()).filter_map(|i| node.child(i)).any(|child| focus_argument(child, bytes))
}

/// Doctest decorators that switch a case off or make a failure pass.
const DOCTEST_SKIPS: &[&str] = &["skip", "may_fail", "should_fail", "expected_failures"];

/// Catch2 tags that hide a case or let it fail: `[.]`, `[.slow]`, `[!hide]`,
/// `[!mayfail]` and `[!shouldfail]`.
const CATCH2_SKIP_TAGS: &[&str] = &["[.", "[hide]", "[!hide]", "[!mayfail]", "[!shouldfail]"];

/// Whether the header of a doctest or Catch2 case holds a `doctest::skip()`
/// style decorator or a Catch2 tag that hides it.
fn skips_case(header: Node, bytes: &[u8]) -> bool {
    if header.child_count() == 0 {
        let Ok(text) = header.utf8_text(bytes) else { return false };
        if is_text_leaf(header.kind()) {
            return CATCH2_SKIP_TAGS.iter().any(|tag| text.contains(tag));
        }
        return DOCTEST_SKIPS.contains(&text)
            && receiver(bytes, header.start_byte()).is_none_or(|r| r.is_empty() || r.ends_with("doctest"));
    }
    (0..header.child_count()).filter_map(|i| header.child(i)).any(|child| skips_case(child, bytes))
}

/// Whether the text of a C or C++ unit holds a line that starts a directive.
/// A directive inside a parse error is no `preproc_*` node, so the text is
/// read as well.
fn holds_directive_text(unit: &Unit, bytes: &[u8]) -> bool {
    let start = unit.nodes.first().map_or(unit.item.start_byte(), |node| node.start_byte());
    let end = unit.tail.map_or(unit.item.end_byte(), |tail| tail.end_byte());
    let text = String::from_utf8_lossy(bytes.get(start..end).unwrap_or_default());
    text.lines().any(|line| line.trim_start().starts_with('#'))
}

/// Whether a node holds a preprocessor directive: a `#define` or `#undef`
/// inside a new test can switch off the asserts of every later test.
fn has_preprocessor(node: Node) -> bool {
    node.kind().starts_with("preproc_") || (0..node.named_child_count()).filter_map(|i| node.named_child(i as u32)).any(has_preprocessor)
}

/// Whether a C or C++ test body calls Unity's `TEST_IGNORE` or
/// `TEST_IGNORE_MESSAGE`, which skips the test.
fn ignores_test(node: Node, bytes: &[u8]) -> bool {
    if node.child_count() == 0 {
        return !is_text_leaf(node.kind()) && node.utf8_text(bytes).is_ok_and(|text| text.starts_with("TEST_IGNORE"));
    }
    (0..node.child_count()).filter_map(|i| node.child(i)).any(|child| ignores_test(child, bytes))
}

/// Whether a node holds an ERROR node anywhere in it. The text of an ERROR
/// node is not a test, so a unit that holds one is never free.
fn has_error_node(node: Node) -> bool {
    node.kind() == "ERROR" || (0..node.named_child_count()).filter_map(|i| node.named_child(i as u32)).any(has_error_node)
}

/// Whether a unit holds an ERROR node anywhere in it. The text of an ERROR
/// node is not a test, so a unit that holds one is never free. The header of
/// a doctest or Catch2 macro is left out: `macro_body` already decides that
/// pair, and a template argument list parses as an ERROR node there.
fn holds_error_node(unit: &Unit) -> bool {
    match unit.tail {
        Some(tail) => has_error_node(tail),
        None => unit.nodes.iter().any(|node| has_error_node(*node)),
    }
}

/// Whether a new unit is free: a trusted query marks it a test, its name is
/// one the old text does not use, every node above it is a test prefix, and
/// its body calls no function that ends the process.
/// `TestMain` and `init` never free, whatever a query says.
fn free(unit: &Unit, named: Option<&queries::Queries>, lang: &str, old: &str, bytes: &[u8], before_first_old: bool) -> bool {
    let Some(name) = named.and_then(|q| q.trusted_test_name(unit.item, bytes)) else { return false };
    if name.is_empty()
        || matches!(name.as_str(), "TestMain" | "init")
        || has_word(old, &name)
        || holds_error_node(unit)
        || calls_exit(unit.item, bytes, lang)
        || unit.tail.is_some_and(|tail| calls_exit(tail, bytes, lang) || skips_case(unit.item, bytes))
        || focuses_run(unit.item, bytes)
        || has_preprocessor(unit.item)
        || unit.tail.is_some_and(has_preprocessor)
        || (matches!(lang, "c" | "cpp") && holds_directive_text(unit, bytes))
        || (matches!(lang, "c" | "cpp") && (ignores_test(unit.item, bytes) || unit.tail.is_some_and(|tail| ignores_test(tail, bytes))))
    {
        return false;
    }
    let Some((_, prefix)) = unit.nodes.split_last() else { return false };
    prefix.iter().all(|node| test_prefix(*node, bytes, before_first_old))
        && decorators(unit.item).all(|node| test_prefix(node, bytes, before_first_old))
        && held_annotations(unit.item).into_iter().all(|node| test_prefix(node, bytes, before_first_old))
}

/// The path of a file that a Rust `mod NAME;` line names, relative to the
/// repository top, in the order rustc tries: `#[path]` against the folder of
/// the declaring file, else `NAME.rs` then `NAME/mod.rs` under the folder
/// that holds the module's files. `None` when a path leaves the top.
fn module_files(rel: &str, name: &str, path: Option<&str>) -> Option<Vec<String>> {
    let declaring = Path::new(rel);
    let dir = declaring.parent().unwrap_or(Path::new(""));
    let stem = declaring.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let joined: Vec<PathBuf> = match path {
        Some(path) => vec![dir.join(path)],
        None => {
            let base = if matches!(stem, "main" | "lib" | "mod") { dir.to_path_buf() } else { dir.join(stem) };
            vec![base.join(format!("{name}.rs")), base.join(name).join("mod.rs")]
        }
    };
    joined.iter().map(|p| normalized(p)).collect()
}

fn normalized(path: &Path) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(parts.join("/"))
}

/// The value of a `#[path = "..."]` attribute, or `None` for any other text.
fn path_attribute(text: &str) -> Option<&str> {
    let inner = text.strip_prefix("#[")?.strip_suffix(']')?.trim();
    let value = inner.strip_prefix("path")?.trim_start().strip_prefix('=')?.trim();
    let value = value.strip_prefix('"')?.strip_suffix('"')?;
    (!value.contains(['"', '\\'])).then_some(value)
}

/// Whether a new body-less Rust `mod NAME;` line is free: its only
/// attributes are `#[path = "..."]` and `#[cfg(test)]`, the module file is
/// new since accept and exists now, and it calls no function that ends the
/// process and holds no focusing argument. The module file is parsed whole.
fn free_mod(
    unit: &Unit,
    rel: &str,
    bytes: &[u8],
    before_first_old: bool,
    repo: Repo,
    parse: &mut impl FnMut(&str) -> Option<tree_sitter::Tree>,
) -> bool {
    let Some(protected) = repo.protected else { return false };
    let item = unit.item;
    if item.kind() != "mod_item" || item.child_by_field_name("body").is_some() || !unit.ctx.is_empty() {
        return false;
    }
    let Some(name) = item.child_by_field_name("name").and_then(|n| n.utf8_text(bytes).ok()) else { return false };
    let mut path = None;
    let Some((_, prefix)) = unit.nodes.split_last() else { return false };
    for node in prefix {
        let Ok(text) = node.utf8_text(bytes) else { return false };
        let compact: String = text.split_whitespace().collect();
        match node.kind() {
            "attribute_item" if compact == "#[cfg(test)]" => {}
            "attribute_item" if path.is_none() && path_attribute(text).is_some() => path = path_attribute(text),
            kind if kind.ends_with("comment") && !before_first_old => {}
            _ => return false,
        }
    }
    let Some(files) = module_files(rel, name, path) else { return false };
    let present: Vec<&String> = files.iter().filter(|file| repo.top.join(file).is_file()).collect();
    if present.is_empty() || present.iter().any(|file| protected.contains_key(file.as_str())) {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(repo.top.join(present[0])) else { return false };
    let Some(tree) = parse(&text) else { return false };
    let root = tree.root_node();
    let source = text.as_bytes();
    !calls_exit(root, source, "rust")
        && !(0..root.named_child_count()).filter_map(|i| root.named_child(i as u32)).any(|child| focuses_run(child, source))
        && !reaches_other_code(root, source)
}

/// Attributes that run code before any test or out of its sight: `#[ctor]`
/// start-up functions and statics placed in a link section.
const STARTUP_ATTRIBUTES: &[&str] = &["ctor", "dtor", "link_section", "used", "no_mangle", "export_name"];

/// Whether a new module file brings in code that the compare never reads: a
/// `mod` line of its own, an `include!`, or a start-up attribute.
fn reaches_other_code(node: Node, bytes: &[u8]) -> bool {
    let hit = match node.kind() {
        "mod_item" => node.child_by_field_name("body").is_none(),
        "macro_invocation" => node
            .child_by_field_name("macro")
            .and_then(|m| m.utf8_text(bytes).ok())
            .is_some_and(|m| m.trim_start_matches("::").trim_start_matches("std::").trim_start_matches("core::") == "include"),
        "attribute_item" | "inner_attribute_item" => {
            node.utf8_text(bytes).is_ok_and(|text| STARTUP_ATTRIBUTES.iter().any(|name| has_word(text, name)))
        }
        _ => false,
    };
    hit || (0..node.named_child_count()).filter_map(|i| node.named_child(i as u32)).any(|child| reaches_other_code(child, bytes))
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
/// A C or C++ text parses into units even when it holds other errors, as long
/// as every node of both trees belongs to a unit.
/// `repo` resolves the project query files of `test_queries`, keyed by
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
    repo: Option<Repo>,
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
    let mut parse = |text: &str| session.parse(home, entry, base.as_deref(), text).ok();
    let before_tree = parse(before)?;
    let now_tree = parse(now)?;
    let language = session.language(entry)?;
    let mut parse = |text: &str| session.parse(home, entry, base.as_deref(), text).ok();
    let named = repo.map(|repo| queries::load(entry.name.as_str(), &language, repo.top, test_queries, repo.protected));
    let old = unit_nodes(before_tree.root_node(), named.as_ref(), before.as_bytes());
    let new = unit_nodes(now_tree.root_node(), named.as_ref(), now.as_bytes());
    if before_tree.root_node().has_error() || now_tree.root_node().has_error() {
        // A C or C++ file with other errors, such as a default argument of
        // `= {}`, still compares by unit: an ERROR region is part of the
        // unit around it and is never free. Any other language, and any
        // tree a unit does not cover, keeps the caller's own rule.
        let covers = |tree: &tree_sitter::Tree, units: &[Unit]| covers_every_node(tree.root_node(), units);
        if !matches!(entry.name.as_str(), "c" | "cpp") || !covers(&before_tree, &old) || !covers(&now_tree, &new) {
            return None;
        }
    }
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
    // The old statements of a split function must keep their order, or a
    // `return` moved above a `RUN_TEST` would skip that test.
    let ordered: BTreeSet<&str> = old.iter().filter(|unit| unit.ordered).map(|unit| unit.ctx.as_str()).collect();
    for ctx in ordered {
        let in_ctx = |units: &[Unit], forms: &[String]| -> Vec<String> {
            units.iter().zip(forms).filter(|(unit, _)| unit.ordered && unit.ctx == ctx).map(|(_, form)| form.clone()).collect()
        };
        let old_seq = in_ctx(&old, &old_forms);
        let new_seq = in_ctx(&new, &new_forms);
        let mut rest = new_seq.iter();
        if !old_seq.iter().all(|form| rest.any(|candidate| candidate == form)) {
            missing = true;
        }
    }
    let kept = kept_units(&new_forms, &old_counts);
    let first_old = new.iter().zip(&kept).find(|(_, kept)| **kept).map_or(usize::MAX, |(unit, _)| unit.start());
    let added = new.iter().zip(&kept).any(|(unit, kept)| {
        let early = unit.start() < first_old;
        !kept
            && !free(unit, named.as_ref(), entry.name.as_str(), before, now.as_bytes(), early)
            && !repo.is_some_and(|repo| free_mod(unit, rel, now.as_bytes(), early, repo, &mut parse))
    });
    if !missing && !added {
        return Some(Units::Preserved);
    }
    Some(Units::Changed(names))
}
