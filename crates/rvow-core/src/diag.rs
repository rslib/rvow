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
    ClarifyForm,
    WideScope,
    SharedScope,
    WeakVerify,
    Runner,
    ConfigureInVerify,
    TestQuery,
    LongTask,
    BroadVerify,
    DependsChain,
    HotFile,
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
            | Rule::LedgerTail
            | Rule::ClarifyForm
            | Rule::WideScope
            | Rule::SharedScope
            | Rule::WeakVerify
            | Rule::ConfigureInVerify
            | Rule::TestQuery
            | Rule::LongTask
            | Rule::BroadVerify
            | Rule::DependsChain
            | Rule::HotFile => Severity::Warning,
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
            Rule::ClarifyForm => "clarify-form",
            Rule::WideScope => "wide-scope",
            Rule::SharedScope => "shared-scope",
            Rule::WeakVerify => "weak-verify",
            Rule::Runner => "runner",
            Rule::ConfigureInVerify => "configure-in-verify",
            Rule::TestQuery => "test-query",
            Rule::LongTask => "long-task",
            Rule::BroadVerify => "broad-verify",
            Rule::DependsChain => "depends-chain",
            Rule::HotFile => "hot-file",
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
            Rule::Ears => "write one sentence: [WHEN|WHILE|IF|WHERE <condition>,] <system> SHALL <response>",
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
            Rule::ClarifyForm => "write [NEEDS CLARIFICATION: question | recommended: answer]",
            Rule::WideScope => "list the files the task changes in files:, so a change elsewhere does not make its proof stale",
            Rule::SharedScope => "keep the files: of the two tasks apart, so a change to one does not make the proof of the other stale",
            Rule::WeakVerify => {
                "make the verify command run the check that fails today, and use a counted test runner when it sets baseline: pass"
            }
            Rule::Runner => {
                "fix the @runner verify: name a runner the config holds, pass each {param} a value it allows, and keep values to runner-safe chars"
            }
            Rule::ConfigureInVerify => {
                "build with `cmake --build --preset ...` and keep the configure step in a `setup` entry or a configured build dir"
            }
            Rule::TestQuery => "fix the query, or remove test_queries for this language",
            Rule::LongTask => "cut the title to one line and put the detail in the spec or in the test the task adds",
            Rule::BroadVerify => {
                "build and run one target, such as `cmake --build <dir> --target <t>` then `ctest -R '^<t>$'`, and put the full build in a final gate task"
            }
            Rule::DependsChain => "keep depends_on to a task whose files really break this one, so one edit does not make a chain stale",
            Rule::HotFile => "name each file in the files: of one task only, and let a final gate task cover what the tasks share",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub rule: Rule,
    pub line: usize,
    pub message: String,
    /// The finding sits in an imported requirement that no change has touched
    /// yet, so it counts as a warning whatever its rule says.
    pub legacy: bool,
}

impl Diagnostic {
    pub fn new(rule: Rule, line: usize, message: impl Into<String>) -> Diagnostic {
        Diagnostic { rule, line, message: message.into(), legacy: false }
    }

    pub fn severity(&self) -> Severity {
        if self.legacy {
            return Severity::Warning;
        }
        self.rule.severity()
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let level = match self.severity() {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(f, "{}: {}: line {}: {}", level, self.rule.code(), self.line, self.message)
    }
}
