use rvow_core::error::{Error, Result};
use rvow_core::store;
use rvow_core::tasks::parse_duration;
use rvow_core::yolo::{self, DEFAULT_FOR, Tier};

use super::project;
use crate::output::Reply;

/// `tier` is `None` to show what is on, `Some(None)` to switch off and `Some(Some(t))` to switch `t` on.
pub fn yolo(tier: Option<Option<Tier>>, length: Option<&str>) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let mut r = Reply::new();
    let on = match tier {
        None => yolo::active(&p),
        Some(None) => {
            yolo::off(&p)?;
            None
        }
        Some(Some(tier)) => {
            let length = match length {
                Some(text) => parse_duration(text).ok_or_else(|| {
                    Error::new("yolo-length", format!("{text:?} is not a length"), "pass --for with a value such as 30m or 2h")
                })?,
                None => DEFAULT_FOR,
            };
            Some(yolo::set(&p, tier, length)?)
        }
    };
    match on {
        Some(on) => {
            r.line(format!("yolo {} is on for {} more minutes; the gate still checks at push", on.tier.name(), on.minutes_left()));
            r.set("tier", on.tier.name());
            r.set("minutes", on.minutes_left());
            r.help("Run `rvow yolo off` to end it sooner");
        }
        None => {
            r.line("yolo is off");
            r.set("tier", "off");
        }
    }
    Ok(r)
}
