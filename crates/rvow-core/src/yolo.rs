//! The relaxed hook tiers that the user can switch on for a while with `rvow yolo`.
//! The switch is a file in the project folder with an end time, so it ends by itself. Agents cannot
//! write it or run the command. The gate is not relaxed: it still checks everything at push.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::ledger::now_ms;
use crate::store::Project;

pub const YOLO_FILE: &str = "yolo.json";
pub const DEFAULT_FOR: Duration = Duration::from_secs(2 * 3600);
pub const MAX_FOR: Duration = Duration::from_secs(24 * 3600);

/// How much the hooks relax, in order. Each tier includes the one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    /// Every rule except tampering only audits.
    Audit,
    /// Also lets the agent run `rvow accept` and `rvow confirm`.
    Auto,
    /// Every hook only audits and a stop is never held back.
    Bypass,
}

impl Tier {
    pub fn name(self) -> &'static str {
        match self {
            Tier::Audit => "audit",
            Tier::Auto => "auto",
            Tier::Bypass => "bypass",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Body {
    tier: Tier,
    until: u64,
}

/// The yolo file: the state plus the seal that ties it to the rerun cache key.
#[derive(Debug, Serialize, Deserialize)]
struct File {
    #[serde(flatten)]
    body: Body,
    #[serde(default)]
    seal: Option<String>,
}

/// A tier that is on, and when it ends in milliseconds since the epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct On {
    pub tier: Tier,
    pub until: u64,
}

fn path(project: &Project) -> std::path::PathBuf {
    project.root.join(YOLO_FILE)
}

/// Switches `tier` on for `length`, which is at most `MAX_FOR`.
pub fn set(project: &Project, tier: Tier, length: Duration) -> Result<On> {
    if length > MAX_FOR {
        return Err(Error::new(
            "yolo-length",
            format!("{}s is longer than the 24h limit", length.as_secs()),
            "pass --for with 24h or less, such as --for 2h",
        ));
    }
    let until = now_ms() + length.as_millis() as u64;
    let body = Body { tier, until };
    let text = serde_json::to_string(&body).map_err(|e| Error::internal(format!("cannot write the yolo state: {e}")))?;
    let key = crate::seal::key(&crate::hardening::project_home(project)?)?;
    let seal = crate::seal::mac(&key, &text)?;
    let file = serde_json::to_string(&File { body, seal: Some(seal) })
        .map_err(|e| Error::internal(format!("cannot write the yolo state: {e}")))?;
    std::fs::write(path(project), file).map_err(|e| Error::io(&path(project), e))?;
    Ok(On { tier, until })
}

/// Switches the tiers off.
pub fn off(project: &Project) -> Result<()> {
    match std::fs::remove_file(path(project)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(Error::io(&path(project), e)),
        _ => Ok(()),
    }
}

/// The tier that is on at `now`, or `None` when none is, it ended, the file is
/// unreadable, or its seal does not check under the rerun cache key.
pub fn active_at(project: &Project, now: u64) -> Option<On> {
    let text = std::fs::read_to_string(path(project)).ok()?;
    let file: File = serde_json::from_str(&text).ok()?;
    let key = crate::seal::read_key(&crate::hardening::project_home(project).ok()?)?;
    let body = serde_json::to_string(&file.body).ok()?;
    if !crate::seal::verify(&key, &body, file.seal.as_deref().unwrap_or("")) {
        return None;
    }
    (file.body.until > now).then_some(On { tier: file.body.tier, until: file.body.until })
}

impl On {
    pub fn minutes_left(self) -> u64 {
        self.until.saturating_sub(now_ms()) / 60_000
    }
}

pub fn active(project: &Project) -> Option<On> {
    active_at(project, now_ms())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::{TempDir, repo_with_commit};

    fn project() -> (TempDir, Project) {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = crate::store::init(&repo, &tmp.0.join("home"), crate::store::Mode::Home).unwrap();
        (tmp, p)
    }

    #[test]
    fn a_tier_is_on_until_it_ends_or_is_switched_off() {
        let (_tmp, p) = project();
        assert_eq!(active(&p), None);
        let on = set(&p, Tier::Auto, Duration::from_secs(60)).unwrap();
        assert_eq!(active(&p), Some(on));
        assert_eq!(active_at(&p, on.until), None, "it ends at its end time");
        off(&p).unwrap();
        assert_eq!(active(&p), None);
        off(&p).unwrap();
    }

    #[test]
    fn a_tier_cannot_last_more_than_a_day() {
        let (_tmp, p) = project();
        let err = set(&p, Tier::Bypass, MAX_FOR + Duration::from_secs(1)).unwrap_err();
        assert_eq!(err.code, "yolo-length");
        assert_eq!(active(&p), None);
    }

    #[test]
    fn the_tiers_are_ordered_and_a_broken_file_means_none() {
        assert!(Tier::Audit < Tier::Auto && Tier::Auto < Tier::Bypass);
        let (_tmp, p) = project();
        std::fs::write(p.root.join(YOLO_FILE), "{").unwrap();
        assert_eq!(active(&p), None);
    }
}
