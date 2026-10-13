//! The canonical syntax form: the syntax tree without whitespace, with
//! every comment kept as its own token.

use std::path::{Path, PathBuf};

use tree_sitter::Node;

use crate::error::{Error, Result};
use crate::syntax::grammar::{Entry, Session};
use crate::vow::sha256_hex;

/// The canonical form of `text` under the grammar of `entry`, or `None` when
/// the text does not parse cleanly: the caller falls back to the raw rules.
/// The form keeps every node kind and the text of every leaf token, and
/// leaves out whitespace. Every third-party node the grammar marks as extra
/// (comments) is kept as its own token: its kind plus its text with runs of
/// whitespace collapsed to one space and the ends trimmed. Leaf text can
/// never collide with a kind: a leaf is written as `kind[len]text`.
pub fn canonical(session: &mut Session, home: &Path, entry: &Entry, base: Option<&str>, text: &str) -> Result<Option<String>> {
    let tree = session.parse(home, entry, base, text)?;
    let root = tree.root_node();
    if root.has_error() {
        return Ok(None);
    }
    let mut extras = Vec::new();
    collect_extras(root, &mut extras);
    let mut out = String::new();
    walk(root, text.as_bytes(), &extras, &mut out);
    Ok(Some(out))
}

/// What a canonical comparison of two texts decided: equal forms, forms
/// that differ, a text that does not parse cleanly, or a grammar that
/// cannot load (the caller keeps its own rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compare {
    Same,
    Different,
    NoForm,
}

/// The saved-result file for one compare: under `<home>/grammars`, keyed by
/// the sha256 of each text and the grammar sha256.
pub fn result_file(home: &Path, entry: &Entry, before: &str, after: &str) -> PathBuf {
    home.join("grammars").join(format!("syntax-{}-{}-{}", sha256_hex(before.as_bytes()), sha256_hex(after.as_bytes()), entry.sha256))
}

/// Compares the canonical forms of `before` and `after`. The result is saved
/// under `<home>/grammars`, keyed by the hash of each text and the grammar
/// hash, and a saved result answers without parsing. A corrupt or unreadable
/// result file means compare again. A parse with error nodes has no form; a
/// grammar failure fails with code "grammar".
pub fn compare(
    session: &mut Session,
    home: &Path,
    entry: &Entry,
    base: Option<&str>,
    before: &str,
    after: &str,
) -> std::result::Result<Compare, Error> {
    if let Some(saved) = read_result(home, entry, before, after) {
        return Ok(saved);
    }
    let outcome = match (canonical(session, home, entry, base, before), canonical(session, home, entry, base, after)) {
        (Ok(Some(a)), Ok(Some(b))) => {
            if a == b {
                Compare::Same
            } else {
                Compare::Different
            }
        }
        (Err(e), _) | (_, Err(e)) => return Err(e),
        _ => Compare::NoForm,
    };
    write_result(home, entry, before, after, outcome);
    Ok(outcome)
}

fn result_text(outcome: Compare) -> &'static str {
    match outcome {
        Compare::Same => "same",
        Compare::Different => "different",
        Compare::NoForm => "noform",
    }
}

fn parse_result(text: &str) -> Option<Compare> {
    match text.trim() {
        "same" => Some(Compare::Same),
        "different" => Some(Compare::Different),
        "noform" => Some(Compare::NoForm),
        _ => None,
    }
}

/// The saved result for these two texts under this grammar, or `None` when
/// no result was saved or the saved file is corrupt or unreadable.
fn read_result(home: &Path, entry: &Entry, before: &str, after: &str) -> Option<Compare> {
    let text = std::fs::read_to_string(result_file(home, entry, before, after)).ok()?;
    parse_result(&text)
}

/// Saves the compare result. A write failure is ignored: the caller still
/// returns the computed outcome, and the next compare tries again.
fn write_result(home: &Path, entry: &Entry, before: &str, after: &str, outcome: Compare) {
    let file = result_file(home, entry, before, after);
    let Some(dir) = file.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = dir.join(format!(".rvow-compare-{}.tmp", std::process::id()));
    if std::fs::write(&tmp, result_text(outcome)).is_err() {
        return;
    }
    // Rename over the old file: a reader never sees a partial result.
    if file.is_file() && std::fs::remove_file(&file).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    if std::fs::rename(&tmp, &file).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Byte ranges of every extra node (comments and other trivia a grammar
/// declares), pre-order, so gap text can drop them before it records them.
fn collect_extras(node: Node, extras: &mut Vec<(usize, usize)>) {
    if node.is_extra() {
        extras.push((node.start_byte(), node.end_byte()));
        return;
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            collect_extras(child, extras);
        }
    }
}

fn walk(node: Node, source: &[u8], extras: &[(usize, usize)], out: &mut String) {
    out.push('(');
    out.push_str(node.kind());
    if node.is_extra() {
        push_normalized(source, node.start_byte()..node.end_byte(), out);
    } else if node.child_count() == 0 {
        push_text(source, node.start_byte()..node.end_byte(), out);
    } else {
        let mut covered = node.start_byte();
        for i in 0..node.child_count() {
            let Some(child) = node.child(i) else { continue };
            if child.start_byte() > covered {
                push_gap(source, covered..child.start_byte(), extras, out);
            }
            covered = covered.max(child.end_byte());
            walk(child, source, extras, out);
        }
        if node.end_byte() > covered {
            push_gap(source, covered..node.end_byte(), extras, out);
        }
    }
    out.push(')');
}

/// Text that no child of a node covers. A grammar can hold content of a
/// string or of a heredoc only as such a gap (quoted toml string is one
/// example), so the gap is token text in the form unless it is whitespace
/// or the span of an extra: extras leave the gap out and walk writes each
/// one as its own token instead.
fn push_gap(source: &[u8], range: std::ops::Range<usize>, extras: &[(usize, usize)], out: &mut String) {
    let mut rest = Vec::new();
    sub_extras(range, extras, &mut rest);
    for piece in rest {
        push_if_visible(source, piece, out);
    }
}

/// The parts of `range` minus every extra span that overlaps it.
fn sub_extras(range: std::ops::Range<usize>, extras: &[(usize, usize)], out: &mut Vec<std::ops::Range<usize>>) {
    let mut rest = vec![range];
    for (extra_start, extra_end) in extras {
        let mut next = Vec::with_capacity(rest.len());
        for r in rest {
            if *extra_end <= r.start || r.end <= *extra_start {
                next.push(r.clone());
                continue;
            }
            if r.start < *extra_start {
                next.push(r.start..*extra_start);
            }
            if *extra_end < r.end {
                next.push(*extra_end..r.end);
            }
        }
        rest = next;
    }
    out.extend(rest);
}

/// A gap piece is form text only when it holds something other than
/// whitespace: trivia that no node and no extra covers is not code.
fn push_if_visible(source: &[u8], piece: std::ops::Range<usize>, out: &mut String) {
    let bytes = &source[piece.clone()];
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return;
    }
    push_text(source, piece, out);
}

/// An extra (comment) node as its own token: runs of whitespace collapse to
/// one space and the ends trim, so reindenting a comment keeps the form.
fn push_normalized(source: &[u8], piece: std::ops::Range<usize>, out: &mut String) {
    let text = String::from_utf8_lossy(&source[piece]);
    let normal: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    out.push('[');
    out.push_str(&normal.len().to_string());
    out.push(']');
    out.push_str(&normal);
}

fn push_text(source: &[u8], piece: std::ops::Range<usize>, out: &mut String) {
    let text = String::from_utf8_lossy(&source[piece]);
    out.push('[');
    out.push_str(&text.len().to_string());
    out.push(']');
    out.push_str(&text);
}
