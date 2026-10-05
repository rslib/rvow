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
    if c.events == 0 {
        r.line(format!("0 events in the last {days} days"));
        r.line("the hooks count each denial and each stop as you work; run rvow install if they are not set up");
        r.help("Run `rvow install` to add the hooks, then work for a while and run `rvow stats` again");
        return Ok(r);
    }
    r.line(format!("last {days} days: {} events in {} sessions", c.events, c.sessions));
    let mut rows: Vec<(&String, &u64)> = c.denials.iter().collect();
    rows.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (rule, n) in &rows {
        r.line(format!("denied {rule}: {n}"));
    }
    r.line(format!("stops: {} blocked, {} passed", c.stop_blocks, c.stop_passes));
    if let Some(per_100) = c.blocks_per_100() {
        r.line(format!("{per_100} stop blocks per 100 stops"));
        r.set("stop_blocks_per_100", per_100);
    }
    r.set("sessions", c.sessions);
    r.set("stop_blocks", c.stop_blocks);
    r.set("stop_passes", c.stop_passes);
    let table: Vec<Obj> = rows.iter().map(|(rule, n)| crate::obj! { "rule" => rule.as_str(), "count" => **n }).collect();
    r.table("denials", table, "no denials");
    Ok(r)
}
