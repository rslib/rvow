//! Every `rvow ...` command that the shipped docs show must exist in the CLI.

use crate::skill::problem;

const DOCS: [(&str, &str); 6] = [
    ("guides/proposal.md", include_str!("../../../guides/proposal.md")),
    ("guides/spec.md", include_str!("../../../guides/spec.md")),
    ("guides/tasks.md", include_str!("../../../guides/tasks.md")),
    ("guides/design.md", include_str!("../../../guides/design.md")),
    ("README.md", include_str!("../../../README.md")),
    ("AGENTS.md", include_str!("../../../AGENTS.md")),
];

/// The code spans and fenced blocks of a Markdown text: the odd pieces between backticks.
fn code(text: &str) -> impl Iterator<Item = &str> {
    text.split('`').skip(1).step_by(2)
}

#[test]
fn the_docs_show_only_real_commands() {
    let bad: Vec<String> = DOCS
        .iter()
        .flat_map(|(name, text)| code(text).flat_map(|c| c.lines()).filter_map(problem).map(move |p| format!("{name}: {p}")))
        .collect();
    assert!(bad.is_empty(), "docs show commands the CLI does not have:\n{}", bad.join("\n"));
}
