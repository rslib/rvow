use crate::diag::{Diagnostic, Rule};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub branch: String,
    pub sections: Vec<String>,
}

const REQUIRED: [&str; 2] = ["Why", "What changes"];

pub fn parse_proposal(src: &str) -> (Proposal, Vec<Diagnostic>) {
    let mut diags = Vec::new();
    let mut branch = String::new();
    let mut sections = Vec::new();
    for (i, line) in src.lines().enumerate() {
        if let Some(b) = line.strip_prefix("branch:") {
            if !sections.is_empty() || !branch.is_empty() {
                diags.push(Diagnostic::new(
                    Rule::Structure,
                    i + 1,
                    "branch: goes once, above the first section",
                ));
            }
            branch = b.trim().to_string();
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
    for name in REQUIRED {
        if !sections.iter().any(|s| s == name) {
            diags.push(Diagnostic::new(
                Rule::Structure,
                1,
                format!("proposal needs a `## {name}` section"),
            ));
        }
    }
    (Proposal { branch, sections }, diags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_and_sections() {
        let (p, diags) =
            parse_proposal("branch: feat/x\n\n## Why\nbecause\n## What changes\n## Assumptions\n");
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(p.branch, "feat/x");
        assert_eq!(p.sections, ["Why", "What changes", "Assumptions"]);
    }

    #[test]
    fn missing_parts_are_errors() {
        let (_, diags) = parse_proposal("## Why\n");
        let msgs: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            msgs,
            [
                "proposal needs a `branch: <name>` line",
                "proposal needs a `## What changes` section"
            ]
        );
    }
}
