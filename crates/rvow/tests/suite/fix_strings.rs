//! Every `Error::new(code, message, fix)` in the sources must give a fix that starts an action.

use std::path::{Path, PathBuf};

const VERBS: &[&str] = &[
    "accept", "add", "amend", "ask", "check", "checkout", "choose", "commit", "confirm", "copy", "create", "delete", "edit", "export",
    "fix", "give", "ingest", "inspect", "install", "keep", "make", "move", "name", "pass", "pick", "provide", "put", "read", "record",
    "remove", "rename", "repair", "replace", "report", "resolve", "restore", "run", "say", "set", "shorten", "use", "wait", "write",
];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The text of the call's arguments, split at the commas of the top level.
fn arguments(rest: &str) -> Vec<String> {
    let (mut depth, mut in_str, mut escaped) = (1, false, false);
    let (mut args, mut cur) = (Vec::new(), String::new());
    for c in rest.chars() {
        if in_str {
            cur.push(c);
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                cur.push(c);
            }
            '(' | '[' | '{' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                cur.push(c);
            }
            ',' if depth == 1 => args.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    args.push(cur);
    args.into_iter().map(|a| a.trim().to_string()).collect()
}

/// The first string literal of `arg`, or `None` for a plain variable.
fn literal(arg: &str) -> Option<String> {
    let start = arg.find('"')? + 1;
    let mut out = String::new();
    let mut chars = arg[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(chars.next().unwrap_or(' ')),
            '"' => return Some(out),
            _ => out.push(c),
        }
    }
    None
}

fn starts_an_action(fix: &str) -> bool {
    fix.split([';', ',']).map(|clause| clause.trim().trim_start_matches("then ").trim()).any(|clause| {
        let word = clause.split_whitespace().next().unwrap_or("").to_lowercase();
        VERBS.contains(&word.as_str())
    })
}

#[test]
fn every_error_fix_names_an_action() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    sources(&root.join("rvow-core/src"), &mut files);
    sources(&root.join("rvow/src"), &mut files);
    let (mut seen, mut bad) = (0, Vec::new());
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        let code = text.split("#[cfg(test)]").next().unwrap_or("");
        for (i, _) in code.match_indices("Error::new(") {
            let args = arguments(&code[i + "Error::new(".len()..]);
            let Some(fix) = args.get(2).and_then(|a| literal(a)) else { continue };
            seen += 1;
            if !starts_an_action(&fix) {
                bad.push(format!("{}: {fix:?}", file.file_name().unwrap().to_string_lossy()));
            }
        }
    }
    assert!(seen > 80, "the scan found only {seen} fix strings");
    assert!(bad.is_empty(), "a fix must start with an action such as run, set or check:\n{}", bad.join("\n"));
}

#[test]
fn the_scan_tells_an_action_from_a_remark() {
    assert!(starts_an_action("run rvow status"));
    assert!(starts_an_action("an accepted change keeps its branch; amend it instead"));
    assert!(!starts_an_action("only a backlog change can be accepted"));
    assert_eq!(arguments("a, format!(\"x, y\", z), \"run it\")")[2], "\"run it\"");
    assert_eq!(literal("format!(\"set {}\", x)").as_deref(), Some("set {}"));
    assert_eq!(literal("fix"), None);
}
