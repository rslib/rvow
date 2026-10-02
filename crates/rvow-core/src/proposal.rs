use crate::diag::{Diagnostic, Rule};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub branch: String,
    pub title: String,
    pub sections: Vec<String>,
    /// Changes that must be archived before this one, from `after: CHG-a, CHG-b`.
    pub after: Vec<String>,
}

const REQUIRED: [&str; 3] = ["Why", "What changes", "Out of scope"];
pub const MAX_WORDS: usize = 150;

pub fn parse_proposal(src: &str) -> (Proposal, Vec<Diagnostic>) {
    let mut diags = Vec::new();
    let mut branch = String::new();
    let mut title = String::new();
    let mut sections = Vec::new();
    let mut after = Vec::new();
    let mut words = 0;
    for (i, line) in src.lines().enumerate() {
        if !line.starts_with('#') && !line.starts_with("branch:") && !line.starts_with("after:") {
            words += line.split_whitespace().count();
        }
        if let Some(b) = line.strip_prefix("branch:") {
            if !sections.is_empty() || !branch.is_empty() {
                diags.push(Diagnostic::new(
                    Rule::Structure,
                    i + 1,
                    "branch: goes once, above the first section",
                ));
            }
            branch = b.trim().to_string();
        } else if let Some(a) = line.strip_prefix("after:") {
            after.extend(
                a.split(',')
                    .map(str::trim)
                    .filter(|x| !x.is_empty())
                    .map(String::from),
            );
        } else if let Some(t) = line.strip_prefix("# ") {
            if title.is_empty() {
                title = t.trim().to_string();
            }
        } else if let Some(s) = line.strip_prefix("## ") {
            sections.push(s.trim().to_string());
        }
    }
    if branch.is_empty() {
        diags.push(Diagnostic::new(
            Rule::Structure,
            1,
            "proposal needs a `branch: <name>` line",
        ));
    }
    if words > MAX_WORDS {
        diags.push(Diagnostic::new(
            Rule::Size,
            1,
            format!(
                "the proposal has {words} words, more than {MAX_WORDS}; say why and what, not how"
            ),
        ));
    }
    for name in REQUIRED {
        if !sections.iter().any(|s| s == name) {
            diags.push(Diagnostic::new(
                Rule::Structure,
                1,
                format!("proposal needs a `## {name}` section"),
            ));
        }
    }
    (
        Proposal {
            branch,
            title,
            sections,
            after,
        },
        diags,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_and_sections() {
        let (p, diags) = parse_proposal(
            "branch: feat/x\n\n## Why\nbecause\n## What changes\n## Out of scope\n## Assumptions\n",
        );
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(p.branch, "feat/x");
        assert!(p.after.is_empty());
        assert_eq!(
            p.sections,
            ["Why", "What changes", "Out of scope", "Assumptions"]
        );
    }

    #[test]
    fn after_lists_changes() {
        let (p, _) = parse_proposal("branch: b\nafter: CHG-7k2mq8, CHG-p3x8a1\n## Why\n");
        assert_eq!(p.after, ["CHG-7k2mq8", "CHG-p3x8a1"]);
    }

    #[test]
    fn title_is_the_first_h1() {
        let (p, _) = parse_proposal("branch: b\n\n# Output tail\n## Why\n# Later\n");
        assert_eq!(p.title, "Output tail");
        let (p, _) = parse_proposal("branch: b\n## Why\n");
        assert_eq!(p.title, "");
    }

    #[test]
    fn long_proposals_warn() {
        let body = "word ".repeat(MAX_WORDS + 1);
        let src = format!("branch: b\n## Why\n{body}\n## What changes\n## Out of scope\n");
        let (_, diags) = parse_proposal(&src);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, Rule::Size);
    }

    #[test]
    fn missing_parts_are_errors() {
        let (_, diags) = parse_proposal("## Why\n");
        let msgs: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            msgs,
            [
                "proposal needs a `branch: <name>` line",
                "proposal needs a `## What changes` section",
                "proposal needs a `## Out of scope` section"
            ]
        );
    }
}
