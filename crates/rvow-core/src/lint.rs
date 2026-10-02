//! The checks behind `rvow validate`. Word lists follow the INCOSE Guide to
//! Writing Requirements (R7, R8, R9, R17, R24, R34), the sentence form follows
//! EARS and the keywords follow RFC 2119 and RFC 8174.

use std::collections::{HashMap, HashSet};

use crate::diag::{Diagnostic, Rule};
use crate::id::{Id, Ref};
use crate::spec::{DeltaFile, DeltaOp, Requirement, Spec, StepKind};
use crate::tasks::Task;

pub const MAX_WORDS: usize = 60;
pub const MAX_SCENARIOS: usize = 5;
pub const MAX_REQUIREMENTS: usize = 30;
pub const MAX_CLARIFICATIONS: usize = 3;
pub const CLARIFICATION: &str = "[NEEDS CLARIFICATION";

/// A diagnostic with the path of its file, relative to the change or the root.
pub type Finding = (String, Diagnostic);

const VAGUE: &[&str] = &[
    "fast",
    "quick",
    "quickly",
    "efficient",
    "efficiently",
    "appropriate",
    "appropriately",
    "user friendly",
    "easy",
    "easily",
    "some",
    "several",
    "sufficient",
    "reasonable",
    "optimal",
    "robust",
    "seamless",
    "flexible",
];

const ESCAPE: &[&str] = &[
    "if necessary",
    "if possible",
    "where possible",
    "as appropriate",
    "as required",
    "as needed",
    "etc",
    "including but not limited to",
    "and so on",
];

const PRONOUNS: &[&str] = &["it", "this", "they", "these", "those", "that"];

const TRIGGERS: &[&str] = &["when", "while", "if", "where"];

/// The text with every backtick span replaced by a space.
fn prose(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, part) in text.split('`').enumerate() {
        out.push_str(if i % 2 == 0 { part } else { " " });
    }
    out
}

fn code_spans(text: &str) -> impl Iterator<Item = &str> {
    text.split('`').skip(1).step_by(2)
}

fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
}

/// Lowercase words joined by single spaces and padded, so `" if necessary "`
/// matches whole words only.
fn phrase_text(text: &str) -> String {
    let joined: Vec<String> = words(text).map(str::to_lowercase).collect();
    format!(" {} ", joined.join(" "))
}

fn has_phrase(haystack: &str, phrase: &str) -> bool {
    haystack.contains(&format!(" {phrase} "))
}

pub fn lint_requirement(req: &Requirement, out: &mut Vec<Diagnostic>) {
    let line = req.text_line;
    let text = prose(&req.text);
    let text = text.trim();
    if text.is_empty() {
        out.push(Diagnostic::new(
            Rule::Structure,
            req.line,
            format!("{} has no requirement text", req.id),
        ));
    } else {
        lint_keywords(text, line, out);
        lint_ears(text, line, out);
        lint_words(&req.text, text, line, out);
    }
    lint_scenarios(req, out);
}

fn lint_keywords(text: &str, line: usize, out: &mut Vec<Diagnostic>) {
    let mut strong = 0;
    let mut weak = Vec::new();
    for w in words(text) {
        match w {
            "SHALL" | "MUST" => strong += 1,
            "SHOULD" | "MAY" => weak.push(w),
            _ if w.eq_ignore_ascii_case("shall") || w.eq_ignore_ascii_case("must") => {
                out.push(Diagnostic::new(
                    Rule::Keyword,
                    line,
                    format!("{w:?} carries no meaning unless it is written in capitals"),
                ));
            }
            _ => {}
        }
    }
    let total = strong + weak.len();
    if total != 1 {
        out.push(Diagnostic::new(
            Rule::Keyword,
            line,
            format!("found {total} of SHALL, MUST, SHOULD or MAY, expected exactly one"),
        ));
    } else if let Some(w) = weak.first() {
        out.push(Diagnostic::new(
            Rule::WeakKeyword,
            line,
            format!("{w} makes the requirement optional"),
        ));
    }
}

fn lint_ears(text: &str, line: usize, out: &mut Vec<Diagnostic>) {
    let mut ears = |msg: &str| out.push(Diagnostic::new(Rule::Ears, line, msg));
    let first = words(text).next().unwrap_or("");
    let is_trigger = TRIGGERS.iter().any(|t| first.eq_ignore_ascii_case(t));
    if is_trigger && first != first.to_uppercase() {
        ears("write the EARS keyword in capitals, as in WHEN");
    }
    if words(text).filter(|w| *w == "WHEN" || *w == "IF").count() > 1 {
        ears("more than one WHEN or IF trigger, split the requirement");
    }
    if is_trigger {
        match text.find(',') {
            None => ears("end the condition with a comma before the main clause"),
            Some(comma) => {
                if !words(&text[comma..]).any(|w| matches!(w, "SHALL" | "MUST" | "SHOULD" | "MAY"))
                {
                    ears("the keyword belongs in the main clause, after the condition");
                }
            }
        }
        if first == "IF" && !text.contains(", THEN ") {
            ears("an IF condition takes the form IF <condition>, THEN <system> SHALL");
        }
    }
    let plain = text.replace("e.g.", "eg").replace("i.e.", "ie");
    let breaks = plain
        .char_indices()
        .filter(|(i, c)| {
            matches!(c, '.' | '!' | '?') && plain[i + 1..].starts_with(char::is_whitespace)
        })
        .count();
    if breaks > 0 {
        ears("write one sentence per requirement");
    }
    if !plain.ends_with('.') {
        ears("end the sentence with a period");
    }
}

fn lint_words(raw: &str, text: &str, line: usize, out: &mut Vec<Diagnostic>) {
    let phrases = phrase_text(text);
    for w in VAGUE.iter().filter(|w| has_phrase(&phrases, w)) {
        out.push(Diagnostic::new(
            Rule::Vague,
            line,
            format!("{w:?} is not measurable"),
        ));
    }
    for w in ESCAPE.iter().filter(|w| has_phrase(&phrases, w)) {
        out.push(Diagnostic::new(
            Rule::EscapeClause,
            line,
            format!("{w:?} leaves the requirement open"),
        ));
    }
    if text.contains('/') {
        out.push(Diagnostic::new(
            Rule::AndOr,
            line,
            "\"/\" or \"and/or\" makes the requirement ambiguous",
        ));
    }
    let subject_after_comma = text
        .split_once(',')
        .and_then(|(_, rest)| words(rest).next());
    for w in [words(text).next(), subject_after_comma]
        .into_iter()
        .flatten()
    {
        if PRONOUNS.contains(&w.to_lowercase().as_str()) {
            out.push(Diagnostic::new(
                Rule::Pronoun,
                line,
                format!("{w:?} as a subject hides what it refers to"),
            ));
        }
    }
    for span in code_spans(raw) {
        if looks_like_design(span) {
            out.push(Diagnostic::new(
                Rule::DesignInSpec,
                line,
                format!("`{span}` looks like a path or a code name"),
            ));
        }
    }
    let count = words(raw).count();
    if count > MAX_WORDS {
        out.push(Diagnostic::new(
            Rule::Size,
            line,
            format!("{count} words, more than {MAX_WORDS}"),
        ));
    }
}

fn looks_like_design(span: &str) -> bool {
    let has_ext = span.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty()
            && (1..=4).contains(&ext.len())
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
    });
    span.contains('/') || span.contains("::") || span.contains("()") || has_ext
}

fn lint_scenarios(req: &Requirement, out: &mut Vec<Diagnostic>) {
    if req.scenarios.is_empty() {
        out.push(Diagnostic::new(
            Rule::Scenario,
            req.line,
            format!("{} needs at least one scenario", req.id),
        ));
    }
    if req.scenarios.len() > MAX_SCENARIOS {
        out.push(Diagnostic::new(
            Rule::Size,
            req.line,
            format!(
                "{} scenarios, more than {MAX_SCENARIOS}",
                req.scenarios.len()
            ),
        ));
    }
    for sc in &req.scenarios {
        let title = phrase_text(&sc.title);
        if has_phrase(&title, "and") || has_phrase(&title, "or") {
            out.push(Diagnostic::new(
                Rule::ScenarioTitle,
                sc.line,
                "\"and\" or \"or\" in a scenario title hints at two behaviors",
            ));
        }
        #[derive(PartialEq, PartialOrd)]
        enum Phase {
            Given,
            When,
            Then,
        }
        let mut phase = Phase::Given;
        let mut whens = 0;
        let mut prev: Option<StepKind> = None;
        let mut bad = |line: usize, msg: &str| {
            out.push(Diagnostic::new(Rule::Scenario, line, msg));
        };
        for step in &sc.steps {
            if step.text.is_empty() {
                bad(step.line, "a step needs text after its keyword");
            }
            let kind = match (step.kind, prev) {
                (StepKind::And, None) => {
                    bad(step.line, "a scenario cannot start with AND");
                    continue;
                }
                (StepKind::And, Some(p)) => p,
                (k, _) => k,
            };
            prev = Some(kind);
            match kind {
                StepKind::Given if phase != Phase::Given => {
                    bad(step.line, "GIVEN comes before WHEN");
                }
                StepKind::When => {
                    whens += 1;
                    if phase == Phase::Then {
                        bad(step.line, "WHEN comes before THEN");
                    }
                    phase = Phase::When;
                }
                StepKind::Then => {
                    if phase == Phase::Given {
                        bad(step.line, "THEN needs a WHEN before it");
                    }
                    phase = Phase::Then;
                }
                _ => {}
            }
        }
        if whens != 1 {
            bad(
                sc.line,
                &format!("found {whens} WHEN steps, expected exactly one"),
            );
        }
        if phase != Phase::Then {
            bad(sc.line, "a scenario needs at least one THEN");
        }
    }
}

/// Checks one living spec on its own.
pub fn lint_spec(spec: &Spec) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for req in &spec.requirements {
        if matches!(req.id, Ref::Placeholder { .. }) {
            out.push(Diagnostic::new(
                Rule::PlaceholderInSpec,
                req.line,
                format!("{} is a placeholder", req.id),
            ));
        }
        if !seen.insert(&req.id) {
            out.push(Diagnostic::new(
                Rule::DuplicateId,
                req.line,
                format!("{} appears twice", req.id),
            ));
        }
        lint_requirement(req, &mut out);
    }
    if spec.requirements.len() > MAX_REQUIREMENTS {
        out.push(Diagnostic::new(
            Rule::Size,
            1,
            format!(
                "{} requirements, more than {MAX_REQUIREMENTS}",
                spec.requirements.len()
            ),
        ));
    }
    out
}

/// The living specs of a project, keyed by capability.
pub struct Living<'a> {
    pub specs: &'a [(String, Spec)],
}

impl Living<'_> {
    fn spec_path(cap: &str) -> String {
        format!("specs/{cap}/spec.md")
    }

    /// The capability that holds each requirement id.
    pub fn index(&self) -> HashMap<&Id, &str> {
        let mut map = HashMap::new();
        for (cap, spec) in self.specs {
            for req in &spec.requirements {
                if let Ref::Id(id) = &req.id {
                    map.insert(id, cap.as_str());
                }
            }
        }
        map
    }

    /// Checks every living spec, and that no id appears in two of them.
    pub fn lint(&self) -> Vec<Finding> {
        let mut out = Vec::new();
        let mut owner: HashMap<&Ref, &str> = HashMap::new();
        for (cap, spec) in self.specs {
            let path = Self::spec_path(cap);
            out.extend(lint_spec(spec).into_iter().map(|d| (path.clone(), d)));
            for req in &spec.requirements {
                if let Some(other) = owner.insert(&req.id, cap)
                    && other != cap
                {
                    out.push((
                        path.clone(),
                        Diagnostic::new(
                            Rule::DuplicateId,
                            req.line,
                            format!("{} is also in {}", req.id, Self::spec_path(other)),
                        ),
                    ));
                }
            }
        }
        out
    }
}

/// The parsed files of one change.
pub struct Change<'a> {
    pub tasks: &'a [Task],
    /// Delta files keyed by capability.
    pub deltas: &'a [(String, DeltaFile)],
    /// Raw text of every Markdown file in the change, keyed by path, for the
    /// `[NEEDS CLARIFICATION]` count.
    pub texts: &'a [(String, String)],
}

/// One open question, written as
/// `[NEEDS CLARIFICATION: question | recommended: answer]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub path: String,
    pub line: usize,
    pub question: String,
    pub recommended: Option<String>,
    /// The other choices, without the recommended one.
    pub options: Vec<String>,
    /// Why the decision matters.
    pub why: Option<String>,
    /// The marker exactly as written, so `rvow answer` can remove it.
    pub marker: String,
}

/// Every `[NEEDS CLARIFICATION` marker outside code. `accept` refuses a
/// change with any.
pub fn questions(texts: &[(String, String)]) -> Vec<Question> {
    let mut out = Vec::new();
    for (path, text) in texts {
        let mut fenced = false;
        for (i, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
            }
            if fenced {
                continue;
            }
            let plain = prose(line);
            for (start, _) in plain.match_indices(CLARIFICATION) {
                let rest = &plain[start + CLARIFICATION.len()..];
                let body = rest.split(']').next().unwrap_or("");
                let marker = format!("{CLARIFICATION}{body}]");
                let body = body.strip_prefix(':').unwrap_or(body);
                let mut parts = body.split('|').map(str::trim);
                let question = parts.next().unwrap_or("");
                let (mut recommended, mut options, mut why) = (None, Vec::new(), None);
                for part in parts {
                    let (key, value) = part.split_once(':').unwrap_or((part, ""));
                    let value = value.trim();
                    match key.trim() {
                        "recommended" if !value.is_empty() => recommended = Some(value.to_string()),
                        "options" => {
                            options = value
                                .split(';')
                                .map(str::trim)
                                .filter(|o| !o.is_empty())
                                .map(String::from)
                                .collect()
                        }
                        "why" if !value.is_empty() => why = Some(value.to_string()),
                        _ => {}
                    }
                }
                out.push(Question {
                    path: path.clone(),
                    line: i + 1,
                    question: question.to_string(),
                    recommended,
                    options,
                    why,
                    marker,
                });
            }
        }
    }
    out
}

/// Every `[NEEDS CLARIFICATION` marker, as (path, line).
pub fn clarifications(texts: &[(String, String)]) -> Vec<(String, usize)> {
    questions(texts)
        .into_iter()
        .map(|q| (q.path, q.line))
        .collect()
}

impl Change<'_> {
    pub fn lint(&self, living: &Living) -> Vec<Finding> {
        let mut out = Vec::new();
        let index = living.index();
        let mut named: HashMap<&Ref, String> = HashMap::new();
        let mut added: HashSet<&Ref> = HashSet::new();

        for (cap, file) in self.deltas {
            let path = format!("specs/{cap}.md");
            for delta in &file.deltas {
                let req = &delta.req;
                let mut push = |d: Diagnostic| out.push((path.clone(), d));
                if let Some(first) = named.insert(&req.id, path.clone()) {
                    push(Diagnostic::new(
                        Rule::DuplicateId,
                        req.line,
                        format!("{} is already named in {first}", req.id),
                    ));
                }
                match (&req.id, delta.op) {
                    (Ref::Placeholder { .. }, DeltaOp::Add) => {
                        added.insert(&req.id);
                    }
                    (Ref::Id(id), DeltaOp::Add) => {
                        added.insert(&req.id);
                        if let Some(home) = index.get(id) {
                            push(Diagnostic::new(
                                Rule::DuplicateId,
                                req.line,
                                format!("ADD {id} is already in specs/{home}/spec.md"),
                            ));
                        }
                    }
                    (Ref::Id(id), op) if op != DeltaOp::Add => match index.get(id) {
                        None => push(Diagnostic::new(
                            Rule::UnknownId,
                            req.line,
                            format!("{id} is in no living spec"),
                        )),
                        Some(home) if op == DeltaOp::Move && home == cap => push(Diagnostic::new(
                            Rule::Structure,
                            req.line,
                            format!("{id} is already in {cap}"),
                        )),
                        Some(_) => {}
                    },
                    _ => {}
                }
                if delta.op.has_body() {
                    let mut diags = Vec::new();
                    lint_requirement(req, &mut diags);
                    for d in diags {
                        push(d);
                    }
                }
            }
        }

        let mut task_ids = HashSet::new();
        let mut covered: HashSet<&Ref> = HashSet::new();
        for task in self.tasks {
            let mut push = |d: Diagnostic| out.push(("tasks.md".to_string(), d));
            if !task_ids.insert(&task.id) {
                push(Diagnostic::new(
                    Rule::DuplicateId,
                    task.line,
                    format!("{} appears twice", task.id),
                ));
            }
            for c in &task.covers {
                covered.insert(c);
                let known = match c {
                    Ref::Placeholder { .. } => added.contains(c),
                    Ref::Id(id) => index.contains_key(id) || added.contains(c),
                };
                if !known {
                    push(Diagnostic::new(
                        Rule::UnknownId,
                        task.line,
                        format!("{} covers {c}, which no spec or ADD holds", task.id),
                    ));
                }
            }
        }

        for (cap, file) in self.deltas {
            for delta in file.deltas.iter().filter(|d| d.op.has_body()) {
                if !covered.contains(&delta.req.id) {
                    out.push((
                        format!("specs/{cap}.md"),
                        Diagnostic::new(
                            Rule::Uncovered,
                            delta.req.line,
                            format!("no task covers {}", delta.req.id),
                        ),
                    ));
                }
            }
        }

        for q in questions(self.texts)
            .iter()
            .filter(|q| q.recommended.is_none())
        {
            out.push((
                q.path.clone(),
                Diagnostic::new(
                    Rule::ClarifyForm,
                    q.line,
                    "the open question has no recommended answer",
                ),
            ));
        }
        let marks = clarifications(self.texts);
        if let Some((path, line)) = marks.get(MAX_CLARIFICATIONS) {
            out.push((
                path.clone(),
                Diagnostic::new(
                    Rule::Clarification,
                    *line,
                    format!(
                        "{} open questions, at most {MAX_CLARIFICATIONS} per change",
                        marks.len()
                    ),
                ),
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{parse_deltas, parse_spec};
    use crate::tasks::parse_tasks;

    fn rules_for(text: &str) -> Vec<Rule> {
        let src =
            format!("# Cap\n## REQ-k3f9m2 Title\n{text}\n### Scenario: s\n- WHEN x\n- THEN y\n");
        let (spec, diags) = parse_spec(&src);
        assert!(diags.is_empty(), "{diags:?}");
        lint_spec(&spec).into_iter().map(|d| d.rule).collect()
    }

    #[test]
    fn clean_requirements_pass() {
        assert_eq!(rules_for("rvow SHALL record each run."), []);
        assert_eq!(
            rules_for("WHEN the user runs `rvow prove`, rvow SHALL append one event."),
            []
        );
        assert_eq!(
            rules_for("IF the command exits nonzero, THEN rvow SHALL mark the task failed."),
            []
        );
        assert_eq!(
            rules_for(
                "WHILE a change is active, WHEN a session starts, rvow SHALL print the brief."
            ),
            []
        );
    }

    #[test]
    fn keyword_rules() {
        assert_eq!(
            rules_for("rvow must record each run."),
            [Rule::Keyword, Rule::Keyword]
        );
        assert_eq!(
            rules_for("rvow SHALL record and SHALL print."),
            [Rule::Keyword]
        );
        assert_eq!(rules_for("rvow MAY print a hint."), [Rule::WeakKeyword]);
    }

    #[test]
    fn ears_rules() {
        assert_eq!(
            rules_for("When a run ends, rvow SHALL store it."),
            [Rule::Ears]
        );
        assert_eq!(
            rules_for("WHEN a run ends rvow SHALL store it."),
            [Rule::Ears]
        );
        assert_eq!(
            rules_for("IF a run fails, rvow SHALL store it."),
            [Rule::Ears]
        );
        assert_eq!(
            rules_for("WHEN a run ends, WHEN a run fails, rvow SHALL store it."),
            [Rule::Ears]
        );
        assert_eq!(rules_for("rvow SHALL store it. Then print."), [Rule::Ears]);
        assert_eq!(rules_for("rvow SHALL store each run"), [Rule::Ears]);
        assert_eq!(rules_for("rvow SHALL store runs, e.g. failed ones."), []);
    }

    #[test]
    fn word_rules() {
        assert_eq!(rules_for("rvow SHALL be fast."), [Rule::Vague]);
        assert_eq!(
            rules_for("rvow SHALL retry if necessary."),
            [Rule::EscapeClause]
        );
        assert_eq!(rules_for("rvow SHALL print and/or log."), [Rule::AndOr]);
        assert_eq!(rules_for("It SHALL print the brief."), [Rule::Pronoun]);
        assert_eq!(
            rules_for("rvow SHALL call `ledger::append()`."),
            [Rule::DesignInSpec]
        );
        assert_eq!(
            rules_for("rvow SHALL write `src/ledger.rs`."),
            [Rule::DesignInSpec]
        );
        assert_eq!(rules_for("rvow SHALL run `rvow prove`."), []);
        assert_eq!(rules_for("rvow SHALL hold some fastener."), [Rule::Vague]);
    }

    #[test]
    fn scenario_rules() {
        let lint = |steps: &str| -> Vec<Rule> {
            let src = format!("# C\n## REQ-k3f9m2 T\nrvow SHALL run.\n### Scenario: s\n{steps}");
            let (spec, diags) = parse_spec(&src);
            assert!(diags.is_empty(), "{diags:?}");
            lint_spec(&spec).into_iter().map(|d| d.rule).collect()
        };
        assert_eq!(
            lint("- GIVEN a\n- AND b\n- WHEN c\n- THEN d\n- AND e\n"),
            []
        );
        assert_eq!(lint("- AND a\n- WHEN c\n- THEN d\n"), [Rule::Scenario]);
        assert_eq!(lint("- WHEN c\n- WHEN d\n- THEN d\n"), [Rule::Scenario]);
        assert_eq!(lint("- WHEN c\n- THEN d\n- GIVEN e\n"), [Rule::Scenario]);
        assert_eq!(lint("- GIVEN a\n- WHEN c\n"), [Rule::Scenario]);
        assert_eq!(lint("- THEN d\n"), [Rule::Scenario, Rule::Scenario]);
    }

    #[test]
    fn scenario_title_and_missing_scenario() {
        let (spec, _) = parse_spec(
            "# C\n## REQ-k3f9m2 T\nrvow SHALL run.\n## REQ-h4t6v9 U\nrvow SHALL stop.\n### Scenario: runs and stops\n- WHEN a\n- THEN b\n",
        );
        let rules: Vec<Rule> = lint_spec(&spec).into_iter().map(|d| d.rule).collect();
        assert_eq!(rules, [Rule::Scenario, Rule::ScenarioTitle]);
    }

    fn living() -> Vec<(String, Spec)> {
        let src =
            "# Ledger\n## REQ-k3f9m2 Run\nrvow SHALL run.\n### Scenario: s\n- WHEN a\n- THEN b\n";
        vec![("ledger".into(), parse_spec(src).0)]
    }

    #[test]
    fn living_specs_reject_duplicates_across_files() {
        let mut specs = living();
        specs.push(("brief".into(), specs[0].1.clone()));
        let found = Living { specs: &specs }.lint();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "specs/brief/spec.md");
        assert_eq!(found[0].1.rule, Rule::DuplicateId);
    }

    #[test]
    fn change_cross_checks() {
        let specs = living();
        let living = Living { specs: &specs };
        let deltas = vec![(
            "ledger".to_string(),
            parse_deltas(
                "\
## ADD REQ-new1 Tail
rvow SHALL store the tail.
### Scenario: s
- WHEN a
- THEN b
## ADD REQ-new2 Head
rvow SHALL store the head.
### Scenario: s
- WHEN a
- THEN b
## REMOVE REQ-zzzzzz
## MOVE REQ-k3f9m2
## ADD REQ-k3f9m2 Again
rvow SHALL run.
### Scenario: s
- WHEN a
- THEN b
",
            )
            .0,
        )];
        let (tasks, diags) = parse_tasks(
            "\
- TSK-x7k2q4 Tail
  covers: REQ-new1, REQ-new9
  verify: true
  files: **
",
        );
        assert!(diags.is_empty(), "{diags:?}");
        let texts = vec![(
            "proposal.md".to_string(),
            "[NEEDS CLARIFICATION: a | recommended: x]\n[NEEDS CLARIFICATION: b | recommended: x]\n[NEEDS CLARIFICATION: c | recommended: x] [NEEDS CLARIFICATION: d | recommended: x]\n"
                .to_string(),
        )];
        let found = Change {
            tasks: &tasks,
            deltas: &deltas,
            texts: &texts,
        }
        .lint(&living);
        let got: Vec<(&str, Rule, usize)> = found
            .iter()
            .map(|(p, d)| (p.as_str(), d.rule, d.line))
            .collect();
        assert_eq!(
            got,
            [
                ("specs/ledger.md", Rule::UnknownId, 11),
                ("specs/ledger.md", Rule::Structure, 12),
                ("specs/ledger.md", Rule::DuplicateId, 13),
                ("specs/ledger.md", Rule::DuplicateId, 13),
                ("tasks.md", Rule::UnknownId, 1),
                ("specs/ledger.md", Rule::Uncovered, 6),
                ("specs/ledger.md", Rule::Uncovered, 13),
                ("proposal.md", Rule::Clarification, 3),
            ]
        );
        assert_eq!(clarifications(&texts).len(), 4);
        let q = questions(&[(
            "proposal.md".to_string(),
            "x [NEEDS CLARIFICATION: Which store? | recommended: home] y\n".to_string(),
        )]);
        assert_eq!(q[0].question, "Which store?");
        assert_eq!(q[0].recommended.as_deref(), Some("home"));
        assert_eq!(
            q[0].marker,
            "[NEEDS CLARIFICATION: Which store? | recommended: home]"
        );
        let q = questions(&[(
            "p.md".to_string(),
            "[NEEDS CLARIFICATION: Where? | recommended: home | options: repo; both | why: sets the layout]".to_string(),
        )]);
        assert_eq!(q[0].options, ["repo", "both"]);
        assert_eq!(q[0].why.as_deref(), Some("sets the layout"));
        let quoted = vec![(
            "design.md".to_string(),
            "`[NEEDS CLARIFICATION]` marks a question.\n```\n[NEEDS CLARIFICATION: x]\n```\n"
                .to_string(),
        )];
        assert_eq!(clarifications(&quoted), []);
    }
}
