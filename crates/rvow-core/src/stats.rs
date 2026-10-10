//! Hook counts: `hook-stats.jsonl` in the project folder holds one line for each
//! denial and each stop outcome. A line holds no command text and no path.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::hook::Rule;
use crate::ledger::{append_line, now_ms};
use crate::store::Project;

pub const STATS_FILE: &str = "hook-stats.jsonl";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hook {
    PreToolUse,
    Stop,
}

impl Hook {
    fn name(self) -> &'static str {
        match self {
            Hook::PreToolUse => "pre-tool-use",
            Hook::Stop => "stop",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A tool call was denied.
    Deny,
    /// A rule in audit mode fired and the call went through.
    Audit,
    /// The backstop turned a denial into a question for the user.
    Ask,
    /// A stop was held back with a reason.
    Block,
    /// A stop went through.
    Pass,
}

impl Outcome {
    fn name(self) -> &'static str {
        match self {
            Outcome::Deny => "deny",
            Outcome::Audit => "audit",
            Outcome::Ask => "ask",
            Outcome::Block => "block",
            Outcome::Pass => "pass",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Line {
    pub time: u64,
    pub hook: String,
    pub rule: Option<String>,
    pub outcome: String,
    pub session: Option<String>,
}

/// Appends one line. A failure to count never fails a hook, so it is ignored.
pub fn record(project: &Project, hook: Hook, outcome: Outcome, rule: Option<Rule>, session: Option<&str>) {
    let line = Line {
        time: now_ms(),
        hook: hook.name().into(),
        rule: rule.map(|r| r.name().into()),
        outcome: outcome.name().into(),
        session: session.map(String::from),
    };
    if let Ok(text) = serde_json::to_string(&line) {
        let _ = append_line(&project.root.join(STATS_FILE), &text);
    }
}

const DAY_MS: u64 = 86_400_000;

/// What the stats file says about the last few days.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub events: u64,
    pub sessions: u64,
    /// Denials of tool calls, counted by rule name.
    pub denials: BTreeMap<String, u64>,
    /// Calls that an audit-mode rule let through, counted by rule name.
    pub audits: BTreeMap<String, u64>,
    /// Denials that the backstop turned into a question.
    pub asks: u64,
    pub stop_blocks: u64,
    pub stop_passes: u64,
}

impl Counts {
    /// Stop blocks per 100 stops, or `None` when no stop was counted.
    pub fn blocks_per_100(&self) -> Option<u64> {
        let stops = self.stop_blocks + self.stop_passes;
        (stops > 0).then(|| (self.stop_blocks * 100 + stops / 2) / stops)
    }
}

/// The counts of the last `days` days up to now.
pub fn recent(project: &Project, days: u64) -> Counts {
    counts(project, days, now_ms())
}

/// Counts the lines newer than `days` days before `now`. A line that does not parse is skipped.
pub fn counts(project: &Project, days: u64, now: u64) -> Counts {
    let text = std::fs::read_to_string(project.root.join(STATS_FILE)).unwrap_or_default();
    let since = now.saturating_sub(days.saturating_mul(DAY_MS));
    let mut out = Counts::default();
    let mut sessions = BTreeSet::new();
    for line in text.lines().filter_map(|l| serde_json::from_str::<Line>(l).ok()).filter(|l| l.time >= since) {
        out.events += 1;
        sessions.extend(line.session);
        match (line.outcome.as_str(), line.rule) {
            ("deny", Some(rule)) => *out.denials.entry(rule).or_default() += 1,
            ("audit", Some(rule)) => *out.audits.entry(rule).or_default() += 1,
            ("ask", _) => out.asks += 1,
            ("block", _) => out.stop_blocks += 1,
            ("pass", _) => out.stop_passes += 1,
            _ => {}
        }
    }
    out.sessions = sessions.len() as u64;
    out
}
