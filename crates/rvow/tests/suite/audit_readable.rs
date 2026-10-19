//! The docs keep one sentence per line, and Markdown joins those lines into
//! one paragraph, so a paragraph may hold at most 4 of them. Callouts use the
//! GitHub alert form, which the site turns into Starlight callouts.

use std::path::Path;

const MAX_SENTENCES: usize = 4;
const ALERTS: &[&str] = &["NOTE", "TIP", "IMPORTANT", "WARNING", "CAUTION"];

fn read(rel: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)).unwrap()
}

/// The paragraphs of `text` with more than `MAX_SENTENCES` lines, by their first line.
/// Lists, tables, headings, quotes, code and indented lines are not paragraph lines.
fn long_paragraphs(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut run, mut first, mut code) = (0, String::new(), false);
    for line in text.lines().chain(std::iter::once("")) {
        if line.trim_start().starts_with("```") {
            code = !code;
        }
        let t = line.trim_start();
        let listed =
            t.starts_with("- ") || t.starts_with("* ") || t.split_once(". ").is_some_and(|(n, _)| n.chars().all(|c| c.is_ascii_digit()));
        let prose = !code && !t.is_empty() && !line.starts_with(' ') && !listed && !t.starts_with(['|', '#', '>', '<', '`']);
        if prose {
            if run == 0 {
                first = line.chars().take(70).collect();
            }
            run += 1;
        } else {
            if run > MAX_SENTENCES {
                out.push(format!("{run} sentences: {first}"));
            }
            run = 0;
        }
    }
    out
}

fn check(page: &str) {
    let text = read(page);
    let long = long_paragraphs(&text);
    assert!(long.is_empty(), "{page} has paragraphs of more than {MAX_SENTENCES} sentences:\n{}", long.join("\n"));
    for alert in text.lines().filter_map(|l| l.trim_start().strip_prefix("> [!")) {
        let kind = alert.split(']').next().unwrap_or("");
        assert!(ALERTS.contains(&kind), "{page}: `> [!{kind}]` is not a GitHub alert kind");
    }
}

#[test]
fn how_it_works_has_short_paragraphs() {
    check("docs/how-it-works.md");
}

#[test]
fn other_pages_have_short_paragraphs() {
    for page in ["docs/settings.md", "docs/authoring-context.md", "docs/ci.md", "docs/harnesses.md", "README.md"] {
        check(page);
    }
}
