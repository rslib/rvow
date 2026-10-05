//! The fixed list of reasons a hook denies a tool call, so a denial can be counted by cause.

/// One cause of a denial. The list stays short: a rule names a family of denials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rule {
    /// `rvow accept`, `confirm`, `ingest` and a faked dialog answer belong to the user.
    OwnerCommand,
    /// Skipping or replacing the git hooks that run the gate.
    GitHooks,
    /// The ledger, the id table and other files only rvow writes.
    Ledger,
    /// Files outside the repository that enforce rvow.
    EnforcementFile,
    /// The checks that stay hidden from the implementing agent.
    HiddenChecks,
    /// The gate's cache file.
    CacheFile,
    /// The lease file.
    LeaseFile,
    /// The hook stats file.
    StatsFile,
    /// A subagent's edit outside its leased task.
    LeaseScope,
    /// A protected test or test setup file.
    ProtectedTest,
    /// A file outside the files of every task.
    OutsideScope,
    /// A code edit before the plan is accepted.
    UnacceptedPlan,
    /// Archived changes and the living specs.
    History,
}

impl Rule {
    pub const ALL: [Rule; 13] = [
        Rule::OwnerCommand,
        Rule::GitHooks,
        Rule::Ledger,
        Rule::EnforcementFile,
        Rule::HiddenChecks,
        Rule::CacheFile,
        Rule::LeaseFile,
        Rule::StatsFile,
        Rule::LeaseScope,
        Rule::ProtectedTest,
        Rule::OutsideScope,
        Rule::UnacceptedPlan,
        Rule::History,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Rule::OwnerCommand => "owner-command",
            Rule::GitHooks => "git-hooks",
            Rule::Ledger => "ledger",
            Rule::EnforcementFile => "enforcement-file",
            Rule::HiddenChecks => "hidden-checks",
            Rule::CacheFile => "cache-file",
            Rule::LeaseFile => "lease-file",
            Rule::StatsFile => "stats-file",
            Rule::LeaseScope => "lease-scope",
            Rule::ProtectedTest => "protected-test",
            Rule::OutsideScope => "outside-scope",
            Rule::UnacceptedPlan => "unaccepted-plan",
            Rule::History => "history",
        }
    }
}

/// A denial: the rule that fired and the text the agent reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denial {
    pub rule: Rule,
    pub reason: String,
}

pub(super) fn denial(rule: Rule, reason: String) -> Option<Denial> {
    Some(Denial { rule, reason })
}
