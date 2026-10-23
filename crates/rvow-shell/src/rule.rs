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
    /// The yolo switch file.
    YoloFile,
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
    /// The plan files of the rvow store, read and written only through rvow.
    Store,
    /// Work on a change whose accept or confirm in this session the harness did not see the user make.
    UnseenAccept,
    /// The record of what the user made, which only the harness's own hook call writes.
    WitnessFile,
}

/// What a rule does when it fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The call is denied.
    Enforce,
    /// The call goes through and the hit is counted. The gate still catches it as drift or scope.
    Audit,
}

impl Rule {
    /// Tampering with rvow itself is enforced, and so is a lease that the user chose. Scope and protected tests
    /// are audited: the gate checks them.
    pub fn mode(self) -> Mode {
        match self {
            Rule::OutsideScope | Rule::ProtectedTest => Mode::Audit,
            _ => Mode::Enforce,
        }
    }

    /// A tampering rule guards rvow itself. Its denial stays a denial after any streak,
    /// since the backstop must not turn it into a question that a tired user may allow.
    pub fn tampering(self) -> bool {
        !matches!(self, Rule::OwnerCommand | Rule::LeaseScope | Rule::UnacceptedPlan | Rule::OutsideScope | Rule::ProtectedTest)
    }

    pub const ALL: [Rule; 17] = [
        Rule::OwnerCommand,
        Rule::GitHooks,
        Rule::Ledger,
        Rule::EnforcementFile,
        Rule::HiddenChecks,
        Rule::CacheFile,
        Rule::LeaseFile,
        Rule::StatsFile,
        Rule::YoloFile,
        Rule::LeaseScope,
        Rule::ProtectedTest,
        Rule::OutsideScope,
        Rule::UnacceptedPlan,
        Rule::History,
        Rule::Store,
        Rule::UnseenAccept,
        Rule::WitnessFile,
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
            Rule::YoloFile => "yolo-file",
            Rule::LeaseScope => "lease-scope",
            Rule::ProtectedTest => "protected-test",
            Rule::OutsideScope => "outside-scope",
            Rule::UnacceptedPlan => "unaccepted-plan",
            Rule::History => "history",
            Rule::Store => "store",
            Rule::UnseenAccept => "unseen-accept",
            Rule::WitnessFile => "witness-file",
        }
    }
}

/// A denial: the rule that fired and the text the agent reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denial {
    pub rule: Rule,
    pub reason: String,
}

pub fn denial(rule: Rule, reason: String) -> Option<Denial> {
    Some(Denial { rule, reason })
}
