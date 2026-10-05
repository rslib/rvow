//! Every `rvow ...` command that the shipped docs show must exist in the CLI.

use clap::CommandFactory;

use crate::skill::problem;

const DOCS: [(&str, &str); 11] = [
    ("guides/proposal.md", include_str!("../../../guides/proposal.md")),
    ("guides/spec.md", include_str!("../../../guides/spec.md")),
    ("guides/tasks.md", include_str!("../../../guides/tasks.md")),
    ("guides/design.md", include_str!("../../../guides/design.md")),
    ("CHANGELOG.md", include_str!("../../../CHANGELOG.md")),
    ("README.md", include_str!("../../../README.md")),
    ("AGENTS.md", include_str!("../../../AGENTS.md")),
    ("docs/authoring-context.md", include_str!("../../../docs/authoring-context.md")),
    ("docs/ci.md", include_str!("../../../docs/ci.md")),
    ("docs/how-it-works.md", include_str!("../../../docs/how-it-works.md")),
    ("docs/settings.md", include_str!("../../../docs/settings.md")),
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

#[test]
fn the_changelog_names_every_command() {
    let mut cmd = crate::Cli::command();
    cmd.build();
    let names: std::collections::HashSet<String> =
        cmd.get_subcommands().map(|c| c.get_name().to_string()).filter(|n| n != "help").collect();
    let changelog = include_str!("../../../CHANGELOG.md");
    // A spanned `rvow <name> ...` names the command whatever its arguments.
    let named: std::collections::HashSet<String> = code(changelog)
        .filter_map(|span| {
            let mut words = span.split_whitespace();
            (words.next() == Some("rvow")).then(|| words.next().map(String::from)).flatten()
        })
        .collect();
    let missing: Vec<String> = names.iter().filter(|n| !named.contains(*n)).cloned().collect();
    assert!(missing.is_empty(), "CHANGELOG.md does not name these shipped commands:\n{}", missing.join("\n"));
}
