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

/// What held a stop back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The plan changed without an amendment.
    Drift,
    /// A task of the session has no proof.
    NoProof,
}

impl StopReason {
    fn name(self) -> &'static str {
        match self {
            StopReason::Drift => "drift",
            StopReason::NoProof => "no-proof",
        }
    }
}

/// The `reason` of a block line written before stop reasons were recorded.
const UNKNOWN_REASON: &str = "unknown";

#[derive(Debug, Serialize, Deserialize)]
pub struct Line {
    pub time: u64,
    pub hook: String,
    pub rule: Option<String>,
    pub outcome: String,
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Appends one line. A failure to count never fails a hook, so it is ignored.
pub fn record(project: &Project, hook: Hook, outcome: Outcome, rule: Option<Rule>, session: Option<&str>) {
    write(project, hook, outcome, rule.map(Rule::name), None, session);
}

/// Appends one line for a stop, with what held it back when it blocked.
pub fn record_stop(project: &Project, outcome: Outcome, reason: Option<StopReason>, session: Option<&str>) {
    write(project, Hook::Stop, outcome, None, reason.map(StopReason::name), session);
}

fn write(project: &Project, hook: Hook, outcome: Outcome, rule: Option<&str>, reason: Option<&str>, session: Option<&str>) {
    let line = Line {
        time: now_ms(),
        hook: hook.name().into(),
        rule: rule.map(String::from),
        outcome: outcome.name().into(),
        session: session.map(String::from),
        reason: reason.map(String::from),
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
    /// Stop blocks counted by what held them; a line without a reason counts as `unknown`.
    pub stop_reasons: BTreeMap<String, u64>,
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
        if line.outcome == "block" {
            *out.stop_reasons.entry(line.reason.clone().unwrap_or_else(|| UNKNOWN_REASON.into())).or_default() += 1;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook;
    use crate::proof::tests::accepted;
    use serde_json::json;

    fn project() -> (crate::git::testrepo::TempDir, Project) {
        let tmp = crate::git::testrepo::TempDir::new();
        let repo = crate::git::testrepo::repo_with_commit(&tmp.0, "r");
        let p = crate::store::init(&repo, &tmp.0.join("home"), crate::store::Mode::Home).unwrap();
        (tmp, p)
    }

    fn reasons(p: &Project) -> BTreeMap<String, u64> {
        recent(p, 1).stop_reasons
    }

    #[test]
    fn stop_reason_counts_each_kind_of_block() {
        let (_tmp, p) = project();
        for reason in [StopReason::Drift, StopReason::NoProof, StopReason::Drift] {
            record_stop(&p, Outcome::Block, Some(reason), Some("s1"));
        }
        record_stop(&p, Outcome::Pass, None, Some("s1"));
        let want: BTreeMap<String, u64> = [("drift".to_string(), 2), ("no-proof".to_string(), 1)].into();
        assert_eq!(reasons(&p), want);
        assert_eq!(recent(&p, 1).stop_blocks, 3);
    }

    #[test]
    fn stop_reason_of_an_old_block_line_is_unknown() {
        let (_tmp, p) = project();
        let old = json!({"time": now_ms(), "hook": "stop", "rule": null, "outcome": "block", "session": "s1"});
        append_line(&p.root.join(STATS_FILE), &old.to_string()).unwrap();
        let want: BTreeMap<String, u64> = [("unknown".to_string(), 1)].into();
        assert_eq!(reasons(&p), want);
    }

    #[test]
    fn stop_reason_is_recorded_when_the_stop_hook_blocks() {
        let (tmp, p, repo, _task) = accepted();
        let home = tmp.0.join("home");
        let write = json!({"file_path": repo.join("out.txt").to_str().unwrap(), "content": "no"});
        let call = json!({"session_id": "s1", "cwd": repo.to_str().unwrap(), "tool_name": "Write", "tool_input": write});
        assert_eq!(hook::claude_pre_tool_use(&p, &call).unwrap(), None);
        let stop = json!({"session_id": "s1", "cwd": repo.to_str().unwrap()});
        assert!(hook::claude_stop(&p, &home, &stop).unwrap().is_some());
        let want: BTreeMap<String, u64> = [("no-proof".to_string(), 1)].into();
        assert_eq!(reasons(&p), want);
    }
}
