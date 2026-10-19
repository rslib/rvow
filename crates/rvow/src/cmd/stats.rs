use rvow_core::error::Result;
use rvow_core::stats;
use rvow_core::store;

use super::project;
use crate::output::Reply;
use crate::toon::Obj;

pub fn stats(days: u64) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let c = stats::recent(&p, days);
    let mut r = Reply::new();
    r.set("days", days);
    r.set("events", c.events);
    if c.events == 0 && c.durations.is_empty() {
        r.line(format!("0 events in the last {days} days"));
        r.line("the hooks count each denial and each stop as you work; run rvow install if they are not set up");
        r.help("Run `rvow install` to add the hooks, then work for a while and run `rvow stats` again");
        return Ok(r);
    }
    r.line(format!("last {days} days: {} events in {} sessions", c.events, c.sessions));
    let hooks: Vec<Obj> = c
        .durations
        .iter()
        .map(|(hook, ms)| {
            let (median, p95) = (stats::percentile(ms, 50).unwrap_or(0), stats::percentile(ms, 95).unwrap_or(0));
            crate::obj! { "hook" => hook.as_str(), "calls" => ms.len() as u64, "median_ms" => median, "p95_ms" => p95 }
        })
        .collect();
    for (hook, ms) in &c.durations {
        let (median, p95) = (stats::percentile(ms, 50).unwrap_or(0), stats::percentile(ms, 95).unwrap_or(0));
        r.line(format!("{hook}: median {median} ms, 95th percentile {p95} ms over {} calls", ms.len()));
    }
    let mut rows: Vec<(&String, &u64)> = c.denials.iter().collect();
    rows.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (rule, n) in &rows {
        r.line(format!("denied {rule}: {n}"));
    }
    for (rule, n) in &c.audits {
        r.line(format!("audited {rule}: {n}"));
    }
    if c.asks > 0 {
        r.line(format!("asked the user after repeated denials: {}", c.asks));
    }
    r.line(format!("stops: {} blocked, {} passed", c.stop_blocks, c.stop_passes));
    for (reason, n) in &c.stop_reasons {
        r.line(format!("stop blocked for {reason}: {n}"));
    }
    if let Some(per_100) = c.blocks_per_100() {
        r.line(format!("{per_100} stop blocks per 100 stops"));
        r.set("stop_blocks_per_100", per_100);
    }
    r.set("sessions", c.sessions);
    r.set("stop_blocks", c.stop_blocks);
    r.set("stop_passes", c.stop_passes);
    let table: Vec<Obj> = rows.iter().map(|(rule, n)| crate::obj! { "rule" => rule.as_str(), "count" => **n }).collect();
    r.table("denials", table, "no denials");
    let audits: Vec<Obj> = c.audits.iter().map(|(rule, n)| crate::obj! { "rule" => rule.as_str(), "count" => *n }).collect();
    r.table("audits", audits, "no audits");
    r.set("asks", c.asks);
    r.table("hooks", hooks, "no hook durations");
    let reasons: Vec<Obj> = c.stop_reasons.iter().map(|(reason, n)| crate::obj! { "reason" => reason.as_str(), "count" => *n }).collect();
    r.table("stop_reasons", reasons, "no stop blocks");
    Ok(r)
}
