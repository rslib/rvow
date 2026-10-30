//! Hook counts: `hook-stats.jsonl` in the project folder holds one line for each
//! denial and each stop outcome, and one duration line for each hook call. A line
//! holds no command text and no path. When the file is larger than 1 MiB, a
//! duration append moves it whole to `ROTATED_FILE`, replacing the one before.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::ledger::{append_line, now_ms};
use crate::store::Project;

use rvow_shell::rule::Rule;

pub use rvow_base::layout::{ROTATED_FILE, STATS_FILE};

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

/// One timed step of a hook call. A fixed set, so a line holds a short name
/// that no call builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Finding the project store from the working folder and the home.
    Store,
    /// Binding the session of the call to the live lease of its worktree.
    Lease,
    /// The checks the hook itself runs: the deny rules of a call, the stop
    /// decision, the lease check of an `agent.spawn`.
    Rules,
    /// The provenance snapshot around a write call, and the catch-up at a stop.
    Snapshot,
    /// The brief a session start builds.
    Brief,
    /// The living specs a stop reads.
    Specs,
    /// The background re-prove check started after a stop.
    Reprove,
}

impl Step {
    const COUNT: usize = 7;
    const ALL: [Step; Step::COUNT] = [Step::Store, Step::Lease, Step::Rules, Step::Snapshot, Step::Brief, Step::Specs, Step::Reprove];

    pub fn name(self) -> &'static str {
        match self {
            Step::Store => "store",
            Step::Lease => "lease",
            Step::Rules => "rules",
            Step::Snapshot => "snapshot",
            Step::Brief => "brief",
            Step::Specs => "specs",
            Step::Reprove => "reprove",
        }
    }
}

/// The ms each step of one hook call took. A step runs at most once per call,
/// so a fixed array holds them and no map is built per call.
#[derive(Debug, Default, Clone, Copy)]
pub struct Timings([Option<u64>; Step::COUNT]);

impl Timings {
    /// Records the ms a step took, replacing an earlier time for the same step.
    pub fn add(&mut self, step: Step, ms: u64) {
        self.0[step as usize] = Some(ms);
    }

    /// The steps that ran, in the order of `Step`.
    pub fn iter(&self) -> impl Iterator<Item = (Step, u64)> + '_ {
        Step::ALL.into_iter().zip(self.0).filter_map(|(step, ms)| ms.map(|ms| (step, ms)))
    }

    fn is_empty(&self) -> bool {
        self.0.iter().all(Option::is_none)
    }

    /// The ms of every step together.
    pub fn total(&self) -> u64 {
        self.0.iter().flatten().sum()
    }

    /// Adds the times of `other` to the same steps of this one.
    pub fn merge(&mut self, other: &Timings) {
        for (step, ms) in other.iter() {
            self.add(step, self.0[step as usize].unwrap_or(0) + ms);
        }
    }
}

thread_local! {
    static INNER: std::cell::Cell<Timings> = const { std::cell::Cell::new(Timings([None; Step::COUNT])) };
}

/// Runs `f` and adds its ms to `step` of the calling thread, for a step that
/// runs deep inside a hook, such as its snapshot; `take_inner` collects them.
pub fn inner<T>(step: Step, f: impl FnOnce() -> T) -> T {
    let start = std::time::Instant::now();
    let out = f();
    let ms = start.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    INNER.with(|t| {
        let mut timings = t.get();
        timings.merge(&Timings::single(step, ms));
        t.set(timings);
    });
    out
}

/// The step times `inner` added on this thread since the last take.
pub fn take_inner() -> Timings {
    INNER.with(|t| t.replace(Timings::default()))
}

impl Timings {
    fn single(step: Step, ms: u64) -> Timings {
        let mut t = Timings::default();
        t.add(step, ms);
        t
    }
}

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
    /// Set only on a duration line that timed its steps; a line written before
    /// step times existed has none and still counts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<BTreeMap<String, u64>>,
}

/// The size of the stats file above which a duration append moves it aside.
const ROTATE_BYTES: u64 = 1024 * 1024;

/// Appends one duration line for a hook call, with the ms each of its steps took.
/// A failure to count never fails a hook, so it is ignored.
pub fn record_duration(project: &Project, hook: Hook, session: Option<&str>, duration_ms: u64, steps: &Timings) {
    append_duration(project, hook, session, duration_ms, Some(steps), ROTATE_BYTES);
}

/// Like `record_duration`, and first moves the file aside when it is larger than `max_bytes`.
/// A rename never rewrites lines, so an append by another hook at the same time lands in one
/// of the two files and still counts; the file and its rotated copy stay near 2 * `max_bytes`.
pub fn record_duration_bounded(project: &Project, hook: Hook, session: Option<&str>, duration_ms: u64, max_bytes: u64) {
    append_duration(project, hook, session, duration_ms, None, max_bytes);
}

/// Appends one duration line, with the time of each step when the caller timed them.
fn append_duration(project: &Project, hook: Hook, session: Option<&str>, duration_ms: u64, steps: Option<&Timings>, max_bytes: u64) {
    use std::io::Write;
    let path = project.root.join(STATS_FILE);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > max_bytes) {
        let rotated = project.root.join(ROTATED_FILE);
        if let Some(dir) = rotated.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::rename(&path, &rotated);
    }
    let mut line = serde_json::json!({"time": now_ms(), "hook": hook.name(), "session": session, "duration_ms": duration_ms});
    if let Some(steps) = steps.filter(|s| !s.is_empty()) {
        line["steps"] = serde_json::Value::Object(steps.iter().map(|(step, ms)| (step.name().to_owned(), ms.into())).collect());
    }
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
        steps: None,
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
    /// The duration in ms of each step, sorted, by hook name and then step name.
    pub steps: BTreeMap<String, BTreeMap<String, Vec<u64>>>,
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
            if let Some(steps) = line.steps {
                let by_step = out.steps.entry(line.hook.clone()).or_default();
                for (step, ms) in steps {
                    by_step.entry(step).or_default().push(ms);
                }
            }
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
    out.steps.values_mut().for_each(|by_step| by_step.values_mut().for_each(|v| v.sort_unstable()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// The step times of one hook as they read back, by step name.
    fn step_times(p: &Project, hook: &str) -> BTreeMap<String, Vec<u64>> {
        recent(p, 1).steps.get(hook).cloned().unwrap_or_default()
    }

    #[test]
    fn stats_step_times_recorded() {
        let (_tmp, p) = project();
        let mut steps = Timings::default();
        steps.add(Step::Store, 3);
        steps.add(Step::Lease, 1);
        steps.add(Step::Rules, 40);
        record_duration(&p, Hook::PreToolUse, Some("s1"), 44, &steps);
        let c = recent(&p, 1);
        assert_eq!(c.durations["pre-tool-use"], [44]);
        let want: BTreeMap<String, Vec<u64>> =
            [("store".to_string(), vec![3]), ("lease".to_string(), vec![1]), ("rules".to_string(), vec![40])].into();
        assert_eq!(step_times(&p, "pre-tool-use"), want);
        let text = std::fs::read_to_string(p.root.join(STATS_FILE)).unwrap();
        assert!(text.contains("\"steps\":{\"store\":3,\"lease\":1,\"rules\":40}"), "{text}");
    }

    #[test]
    fn stats_step_inner_times_add_up() {
        let _ = take_inner();
        inner(Step::Snapshot, || std::thread::sleep(std::time::Duration::from_millis(3)));
        inner(Step::Snapshot, || ());
        let t = take_inner();
        assert!(t.iter().any(|(s, ms)| s == Step::Snapshot && ms >= 3), "{t:?}");
        assert!(take_inner().is_empty());
    }

    #[test]
    fn stats_step_old_line_reads() {
        let (_tmp, p) = project();
        let old = json!({"time": now_ms(), "hook": "pre-tool-use", "session": "s1", "duration_ms": 7});
        append_line(&p.root.join(STATS_FILE), &old.to_string()).unwrap();
        let c = recent(&p, 1);
        assert_eq!(c.durations["pre-tool-use"], [7]);
        assert!(c.steps.is_empty(), "a line without steps adds no step: {:?}", c.steps);
    }
}
