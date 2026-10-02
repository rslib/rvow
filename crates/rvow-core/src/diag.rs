use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
}

/// Every check that `validate` runs. The severity of a rule is fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rule {
    Structure,
    IdForm,
    DuplicateId,
    PlaceholderInSpec,
    UnknownId,
    Keyword,
    WeakKeyword,
    Ears,
    Scenario,
    Uncovered,
    Clarification,
    TaskField,
    Vague,
    EscapeClause,
    AndOr,
    Pronoun,
    ScenarioTitle,
    DesignInSpec,
    Size,
    Ledger,
    LedgerTail,
}

impl Rule {
    pub fn severity(self) -> Severity {
        match self {
            Rule::WeakKeyword
            | Rule::Vague
            | Rule::EscapeClause
            | Rule::AndOr
            | Rule::Pronoun
            | Rule::ScenarioTitle
            | Rule::DesignInSpec
            | Rule::Size
            | Rule::LedgerTail => Severity::Warning,
            _ => Severity::Error,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Rule::Structure => "structure",
            Rule::IdForm => "id-form",
            Rule::DuplicateId => "duplicate-id",
            Rule::PlaceholderInSpec => "placeholder-in-spec",
            Rule::UnknownId => "unknown-id",
            Rule::Keyword => "keyword",
            Rule::WeakKeyword => "weak-keyword",
            Rule::Ears => "ears",
            Rule::Scenario => "scenario",
            Rule::Uncovered => "uncovered",
            Rule::Clarification => "clarification",
            Rule::TaskField => "task-field",
            Rule::Vague => "vague",
            Rule::EscapeClause => "escape-clause",
            Rule::AndOr => "and-or",
            Rule::Pronoun => "pronoun",
            Rule::ScenarioTitle => "scenario-title",
            Rule::DesignInSpec => "design-in-spec",
            Rule::Size => "size",
            Rule::Ledger => "ledger",
            Rule::LedgerTail => "ledger-tail",
        }
    }

    pub fn fix(self) -> &'static str {
        match self {
            Rule::Structure => "use the layout shown in the template",
            Rule::IdForm => "write ids as KIND-xxxxxx or KIND-new<n>",
            Rule::DuplicateId => "give each item its own id",
            Rule::PlaceholderInSpec => "add requirements through a change with ADD",
            Rule::UnknownId => "name an id that exists, or ADD it in this change",
            Rule::Keyword => "write exactly one uppercase SHALL or MUST",
            Rule::WeakKeyword => "use SHALL or MUST unless the behavior is truly optional",
            Rule::Ears => {
                "write one sentence: [WHEN|WHILE|IF|WHERE <condition>,] <system> SHALL <response>"
            }
            Rule::Scenario => "write an optional GIVEN, one WHEN and at least one THEN, in order",
            Rule::Uncovered => "add a task with covers: for this requirement",
            Rule::Clarification => "answer the question and move the answer into the text",
            Rule::TaskField => "give the task verify: and files:, and only known fields",
            Rule::Vague => "replace the word with a measurable value",
            Rule::EscapeClause => "state the exact condition instead",
            Rule::AndOr => "split into separate requirements or pick one",
            Rule::Pronoun => "name the subject",
            Rule::ScenarioTitle => "split the scenario into one behavior each",
            Rule::DesignInSpec => "move paths and code names to design.md",
            Rule::Size => "split the requirement or the capability",
            Rule::Ledger => "restore the ledger from git or a backup",
            Rule::LedgerTail => "none needed, the next write removes the partial line",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub rule: Rule,
    pub line: usize,
    pub message: String,
}

impl Diagnostic {
    pub fn new(rule: Rule, line: usize, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            rule,
            line,
            message: message.into(),
        }
    }

    pub fn severity(&self) -> Severity {
        self.rule.severity()
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let level = match self.severity() {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(
            f,
            "{}: {}: line {}: {}",
            level,
            self.rule.code(),
            self.line,
            self.message
        )
    }
}
