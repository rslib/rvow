//! Parser for living specs (`specs/<capability>/spec.md`) and the spec deltas
//! of a change (`specs/<capability>.md`). Both share one block layout.

use crate::diag::{Diagnostic, Rule};
use crate::id::{Kind, Ref};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    Given,
    When,
    Then,
    And,
}

impl StepKind {
    pub fn word(self) -> &'static str {
        match self {
            StepKind::Given => "GIVEN",
            StepKind::When => "WHEN",
            StepKind::Then => "THEN",
            StepKind::And => "AND",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub kind: StepKind,
    pub text: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scenario {
    pub title: String,
    pub steps: Vec<Step>,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    pub id: Ref,
    pub title: String,
    pub deprecated: bool,
    pub text: String,
    pub text_line: usize,
    pub scenarios: Vec<Scenario>,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub title: String,
    pub requirements: Vec<Requirement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaOp {
    Add,
    Change,
    Deprecate,
    Move,
    Remove,
}

impl DeltaOp {
    pub fn word(self) -> &'static str {
        match self {
            DeltaOp::Add => "ADD",
            DeltaOp::Change => "CHANGE",
            DeltaOp::Deprecate => "DEPRECATE",
            DeltaOp::Move => "MOVE",
            DeltaOp::Remove => "REMOVE",
        }
    }

    fn parse(s: &str) -> Option<DeltaOp> {
        Some(match s {
            "ADD" => DeltaOp::Add,
            "CHANGE" => DeltaOp::Change,
            "DEPRECATE" => DeltaOp::Deprecate,
            "MOVE" => DeltaOp::Move,
            "REMOVE" => DeltaOp::Remove,
            _ => return None,
        })
    }

    /// Whether the delta carries the full requirement text and scenarios.
    pub fn has_body(self) -> bool {
        matches!(self, DeltaOp::Add | DeltaOp::Change)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delta {
    pub op: DeltaOp,
    pub req: Requirement,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaFile {
    pub deltas: Vec<Delta>,
}

pub fn parse_spec(src: &str) -> (Spec, Vec<Diagnostic>) {
    let (title, blocks, mut diags) = parse_blocks(src, false);
    let requirements = blocks.into_iter().map(|(_, r)| r).collect();
    if title.is_empty() {
        diags.push(Diagnostic::new(
            Rule::Structure,
            1,
            "spec needs a `# <capability>` title",
        ));
    }
    (
        Spec {
            title,
            requirements,
        },
        diags,
    )
}

pub fn parse_deltas(src: &str) -> (DeltaFile, Vec<Diagnostic>) {
    let (_, blocks, mut diags) = parse_blocks(src, true);
    let mut deltas = Vec::new();
    for (op, req) in blocks {
        let op = op.expect("delta mode yields an op");
        let has_text = !req.text.is_empty() || !req.scenarios.is_empty();
        match (op.has_body(), &req.id) {
            (_, Ref::Placeholder { .. }) if op != DeltaOp::Add => diags.push(Diagnostic::new(
                Rule::IdForm,
                req.line,
                "only ADD takes a placeholder id",
            )),
            _ => {}
        }
        if op.has_body() && req.title.is_empty() {
            diags.push(Diagnostic::new(
                Rule::Structure,
                req.line,
                "ADD and CHANGE need a title after the id",
            ));
        }
        if !op.has_body() && has_text {
            diags.push(Diagnostic::new(
                Rule::Structure,
                req.line,
                "DEPRECATE, MOVE and REMOVE take only the id, with no text or scenarios",
            ));
        }
        deltas.push(Delta { op, req });
    }
    (DeltaFile { deltas }, diags)
}

type Block = (Option<DeltaOp>, Requirement);

fn parse_blocks(src: &str, delta: bool) -> (String, Vec<Block>, Vec<Diagnostic>) {
    let mut title = String::new();
    let mut blocks: Vec<Block> = Vec::new();
    let mut diags = Vec::new();

    let mut fenced = false;
    for (i, raw) in src.lines().enumerate() {
        let n = i + 1;
        let line = raw.trim_end();
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced || line.trim().is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("# ") {
            if title.is_empty() && blocks.is_empty() {
                title = rest.trim().to_string();
            } else {
                diags.push(Diagnostic::new(
                    Rule::Structure,
                    n,
                    "only one `#` title, at the top",
                ));
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("## ") {
            if let Some(block) = parse_heading(rest, n, delta, &mut diags) {
                blocks.push(block);
            }
            continue;
        }
        let Some((_, req)) = blocks.last_mut() else {
            diags.push(Diagnostic::new(
                Rule::Structure,
                n,
                "text before the first `##` requirement",
            ));
            continue;
        };
        if let Some(rest) = line.strip_prefix("### ") {
            match rest.strip_prefix("Scenario:") {
                Some(t) => req.scenarios.push(Scenario {
                    title: t.trim().to_string(),
                    steps: Vec::new(),
                    line: n,
                }),
                None => diags.push(Diagnostic::new(
                    Rule::Structure,
                    n,
                    "a `###` heading must be `### Scenario: <title>`",
                )),
            }
            continue;
        }
        if let Some(scenario) = req.scenarios.last_mut() {
            match parse_step(line, n) {
                Some(step) => scenario.steps.push(step),
                None => diags.push(Diagnostic::new(
                    Rule::Structure,
                    n,
                    "a scenario line must be `- GIVEN|WHEN|THEN|AND <text>`",
                )),
            }
            continue;
        }
        if line == "status: deprecated" && req.text.is_empty() {
            req.deprecated = true;
            continue;
        }
        if req.text.is_empty() {
            req.text_line = n;
        } else {
            req.text.push(' ');
        }
        req.text.push_str(line.trim());
    }
    (title, blocks, diags)
}

fn parse_heading(rest: &str, n: usize, delta: bool, diags: &mut Vec<Diagnostic>) -> Option<Block> {
    let mut words = rest.trim();
    let op = if delta {
        let (word, tail) = words.split_once(' ').unwrap_or((words, ""));
        match DeltaOp::parse(word) {
            Some(op) => {
                words = tail.trim();
                Some(op)
            }
            None => {
                diags.push(Diagnostic::new(
                    Rule::Structure,
                    n,
                    format!(
                        "unknown delta {word:?}, expected ADD, CHANGE, DEPRECATE, MOVE or REMOVE"
                    ),
                ));
                return None;
            }
        }
    } else {
        None
    };
    let (id, title) = words.split_once(' ').unwrap_or((words, ""));
    let id = match id.parse::<Ref>() {
        Ok(id) => id,
        Err(e) => {
            diags.push(Diagnostic::new(Rule::IdForm, n, format!("{id:?}: {e}")));
            return None;
        }
    };
    if id.kind() != Kind::Req {
        diags.push(Diagnostic::new(
            Rule::IdForm,
            n,
            "a requirement id starts with REQ-",
        ));
        return None;
    }
    if !delta && title.trim().is_empty() {
        diags.push(Diagnostic::new(
            Rule::Structure,
            n,
            "a requirement heading needs a title after the id",
        ));
    }
    Some((
        op,
        Requirement {
            id,
            title: title.trim().to_string(),
            deprecated: false,
            text: String::new(),
            text_line: n,
            scenarios: Vec::new(),
            line: n,
        },
    ))
}

fn parse_step(line: &str, n: usize) -> Option<Step> {
    let rest = line.trim_start().strip_prefix("- ")?;
    let (word, text) = rest.split_once(' ').unwrap_or((rest, ""));
    let kind = match word {
        "GIVEN" => StepKind::Given,
        "WHEN" => StepKind::When,
        "THEN" => StepKind::Then,
        "AND" => StepKind::And,
        _ => return None,
    };
    Some(Step {
        kind,
        text: text.trim().to_string(),
        line: n,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = "\
# Ledger

## REQ-k3f9m2 Record a proof run
status: deprecated

WHEN the user runs `rvow prove` on a task,
rvow SHALL append one proof event.

### Scenario: A passing run is recorded
- GIVEN a task whose verify command exits 0
- WHEN `rvow prove` runs on that task
- THEN the ledger holds one proof event
- AND the event has exit status 0
";

    #[test]
    fn parses_a_living_spec() {
        let (spec, diags) = parse_spec(SPEC);
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(spec.title, "Ledger");
        let req = &spec.requirements[0];
        assert_eq!(req.id, "REQ-k3f9m2".parse().unwrap());
        assert_eq!(req.title, "Record a proof run");
        assert!(req.deprecated);
        assert_eq!(
            req.text,
            "WHEN the user runs `rvow prove` on a task, rvow SHALL append one proof event."
        );
        assert_eq!(req.text_line, 6);
        let steps: Vec<StepKind> = req.scenarios[0].steps.iter().map(|s| s.kind).collect();
        assert_eq!(
            steps,
            [
                StepKind::Given,
                StepKind::When,
                StepKind::Then,
                StepKind::And
            ]
        );
    }

    #[test]
    fn reports_structure_errors_with_lines() {
        let (_, diags) =
            parse_spec("stray\n# T\n## REQ-k3f9m2\n### Notes\n### Scenario: s\nplain\n");
        let lines: Vec<(Rule, usize)> = diags.iter().map(|d| (d.rule, d.line)).collect();
        assert_eq!(
            lines,
            [
                (Rule::Structure, 1),
                (Rule::Structure, 3),
                (Rule::Structure, 4),
                (Rule::Structure, 6)
            ]
        );
    }

    #[test]
    fn fenced_code_is_not_structure() {
        let src =
            "# T\n## REQ-k3f9m2 R\nrvow SHALL run.\n```\n# not a title\n## not a heading\n```\n";
        let (spec, diags) = parse_spec(src);
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(spec.requirements[0].text, "rvow SHALL run.");
    }

    #[test]
    fn parses_deltas() {
        let src = "\
## ADD REQ-new1 Record the output tail
rvow SHALL store the last 5 lines of output.
### Scenario: s
- WHEN a run ends
- THEN the tail is stored
## CHANGE REQ-k3f9m2 Record a proof run
rvow SHALL append one proof event.
## DEPRECATE REQ-q8w2c7
## MOVE REQ-h4t6v9
## REMOVE REQ-7xq2a5
";
        let (file, diags) = parse_deltas(src);
        assert!(diags.is_empty(), "{diags:?}");
        let ops: Vec<DeltaOp> = file.deltas.iter().map(|d| d.op).collect();
        assert_eq!(
            ops,
            [
                DeltaOp::Add,
                DeltaOp::Change,
                DeltaOp::Deprecate,
                DeltaOp::Move,
                DeltaOp::Remove
            ]
        );
    }

    #[test]
    fn delta_id_and_body_rules() {
        let src = "\
## ADD REQ-k3f9m2 Real id on ADD, as after accept
## CHANGE REQ-new2 Placeholder on CHANGE
## REMOVE REQ-7xq2a5
rvow SHALL not have text here.
## RENAME REQ-7xq2a5
";
        let (_, diags) = parse_deltas(src);
        let got: Vec<(Rule, usize)> = diags.iter().map(|d| (d.rule, d.line)).collect();
        assert_eq!(
            got,
            [
                (Rule::Structure, 5),
                (Rule::IdForm, 2),
                (Rule::Structure, 3)
            ]
        );
    }
}
