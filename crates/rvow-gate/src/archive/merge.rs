use std::collections::BTreeMap;
use std::ops::Range;

use crate::change::{self, CapSpec, ChangeDir, DELTAS};
use crate::error::{Error, Result};
use crate::lint::Living;
use crate::spec::{DeltaOp, parse_spec};

fn lines_of<'a>(caps: &'a mut BTreeMap<String, Vec<String>>, texts: &[(String, String)], cap: &str) -> &'a mut Vec<String> {
    caps.entry(cap.to_string()).or_insert_with(|| match texts.iter().find(|(c, _)| c == cap) {
        Some((_, t)) => split(t),
        None => vec![format!("# {cap}")],
    })
}

fn is_heading(line: &str) -> bool {
    line.starts_with("## ")
}

/// The end of the block whose `## ` heading is at `start`: the next `## `
/// heading outside a code fence, or the end of the text.
fn block_end(lines: &[String], start: usize) -> usize {
    let mut fenced = false;
    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if !fenced && is_heading(line) {
            return i;
        }
    }
    lines.len()
}

/// The lines of the requirement `id` in a living spec.
fn find_block(lines: &[String], id: &str) -> Option<Range<usize>> {
    let mut fenced = false;
    for (i, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if !fenced
            && let Some(rest) = line.strip_prefix("## ")
            && rest.split_whitespace().next() == Some(id)
        {
            return Some(i..block_end(lines, i));
        }
    }
    None
}

fn trim_blank_tail(lines: &mut Vec<String>) {
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
}

fn append_block(lines: &mut Vec<String>, block: Vec<String>) {
    trim_blank_tail(lines);
    lines.push(String::new());
    lines.extend(block);
}

fn split(text: &str) -> Vec<String> {
    text.lines().map(String::from).collect()
}

fn join(lines: &[String]) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// The new text of every capability the merge touches, `None` for one with no
/// requirement left, and the merged ids by op.
type Merge = (BTreeMap<String, Option<String>>, Vec<(DeltaOp, String)>);

pub(super) fn merge(texts: &[(String, String)], living: &[CapSpec], change: &ChangeDir, loaded: &change::Loaded) -> Result<Merge> {
    let index: BTreeMap<String, String> =
        Living { specs: living }.index().into_iter().map(|(id, cap)| (id.to_string(), cap.to_string())).collect();
    let mut caps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut merged = Vec::new();
    for (cap, file) in &loaded.deltas {
        let path = change.path.join(DELTAS).join(format!("{cap}.md"));
        let delta_lines = split(&std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?);
        for d in &file.deltas {
            let id = d.req.id.to_string();
            let body = || {
                let start = d.req.line - 1;
                let mut block = vec![format!("## {id} {}", d.req.title)];
                block.extend_from_slice(&delta_lines[start + 1..block_end(&delta_lines, start)]);
                trim_blank_tail(&mut block);
                block
            };
            let home = index.get(&id).map(String::as_str).unwrap_or(cap);
            let missing = || Error::new("merge", format!("{id} is not in specs/{home}/spec.md"), "run rvow validate and fix the delta");
            let lines = lines_of(&mut caps, texts, home);
            match d.op {
                DeltaOp::Add => append_block(lines, body()),
                DeltaOp::Change => {
                    let range = find_block(lines, &id).ok_or_else(missing)?;
                    let mut block = body();
                    if range.end < lines.len() {
                        block.push(String::new());
                    }
                    lines.splice(range, block);
                }
                DeltaOp::Deprecate => {
                    let range = find_block(lines, &id).ok_or_else(missing)?;
                    let (spec, _) = parse_spec(&join(&lines[range.clone()]));
                    if !spec.requirements.first().is_some_and(|r| r.deprecated) {
                        lines.insert(range.start + 1, "status: deprecated".into());
                    }
                }
                DeltaOp::Move => {
                    let range = find_block(lines, &id).ok_or_else(missing)?;
                    let mut block: Vec<String> = lines.drain(range).collect();
                    trim_blank_tail(&mut block);
                    append_block(lines_of(&mut caps, texts, cap), block);
                }
                DeltaOp::Remove => {
                    let range = find_block(lines, &id).ok_or_else(missing)?;
                    lines.drain(range);
                }
            }
            merged.push((d.op, id));
        }
    }
    let out = caps
        .into_iter()
        .map(|(cap, mut lines)| {
            trim_blank_tail(&mut lines);
            let text = join(&lines);
            let empty = parse_spec(&text).0.requirements.is_empty();
            (cap, (!empty).then_some(text))
        })
        .collect();
    Ok((out, merged))
}
