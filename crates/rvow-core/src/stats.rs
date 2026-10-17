//! Hook counts: `hook-stats.jsonl` in the project folder holds one line for each
//! denial and each stop outcome, and one duration line for each hook call. A line
//! holds no command text and no path. When the file is larger than 1 MiB, a
//! duration append moves it whole to `ROTATED_FILE`, replacing the one before.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::hook::Rule;
use crate::ledger::{append_line, now_ms};
use crate::store::Project;

pub const STATS_FILE: &str = "hook-stats.jsonl";
/// Where a full stats file moves, under the local-only `hook-state/` folder.
pub const ROTATED_FILE: &str = "hook-state/hook-stats.old.jsonl";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hook {
    PreToolUse,
    PostToolUse,
    Stop,
    SessionStart,
    LeaseBind,
    LeaseEnd,
}

impl Hook {
    fn name(self) -> &'static str {
        match self {
            Hook::PreToolUse => "pre-tool-use",
            Hook::PostToolUse => "post-tool-use",
            Hook::Stop => "stop",
            Hook::SessionStart => "session-start",
            Hook::LeaseBind => "lease-bind",
            Hook::LeaseEnd => "lease-end",
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
    #[serde(default)]
    pub rule: Option<String>,
    /// Empty on a duration line.
    #[serde(default)]
    pub outcome: String,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Set only on a duration line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// The size of the stats file above which a duration append moves it aside.
const ROTATE_BYTES: u64 = 1024 * 1024;

/// Appends one duration line for a hook call. A failure to count never fails a hook, so it is ignored.
pub fn record_duration(project: &Project, hook: Hook, session: Option<&str>, duration_ms: u64) {
    record_duration_bounded(project, hook, session, duration_ms, ROTATE_BYTES);
}

/// Like `record_duration`, and first moves the file aside when it is larger than `max_bytes`.
/// A rename never rewrites lines, so an append by another hook at the same time lands in one
/// of the two files and still counts; the file and its rotated copy stay near 2 * `max_bytes`.
pub fn record_duration_bounded(project: &Project, hook: Hook, session: Option<&str>, duration_ms: u64, max_bytes: u64) {
    use std::io::Write;
    let path = project.root.join(STATS_FILE);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > max_bytes) {
        let rotated = project.root.join(ROTATED_FILE);
        if let Some(dir) = rotated.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::rename(&path, &rotated);
    }
    let line = serde_json::json!({"time": now_ms(), "hook": hook.name(), "session": session, "duration_ms": duration_ms});
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(format!("{line}\n").as_bytes());
    }
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
        duration_ms: None,
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
    /// The duration in ms of each hook call, sorted, by hook name.
    pub durations: BTreeMap<String, Vec<u64>>,
}

/// The nearest-rank percentile `pct` of sorted `values`, or `None` when there are none.
pub fn percentile(values: &[u64], pct: u64) -> Option<u64> {
    let rank = (pct * values.len() as u64).div_ceil(100).max(1) as usize;
    values.get(rank - 1).or(values.last()).copied()
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
    let read = |rel: &str| std::fs::read_to_string(project.root.join(rel)).unwrap_or_default();
    let text = read(ROTATED_FILE) + &read(STATS_FILE);
    let since = now.saturating_sub(days.saturating_mul(DAY_MS));
    let mut out = Counts::default();
    let mut sessions = BTreeSet::new();
    for line in text.lines().filter_map(|l| serde_json::from_str::<Line>(l).ok()).filter(|l| l.time >= since) {
        if let Some(ms) = line.duration_ms {
            out.durations.entry(line.hook).or_default().push(ms);
            continue;
        }
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
    out.durations.values_mut().for_each(|v| v.sort_unstable());
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
