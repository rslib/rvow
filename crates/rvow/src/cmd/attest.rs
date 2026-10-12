use std::path::Path;

use rvow_core::error::Result;
use rvow_core::store;

use super::{cwd, project};
use crate::output::Reply;

/// Exports the current proofs of a change as in-toto attestations.
pub fn attest(change: &str, out: &Path, sign: bool) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let done = rvow_core::attest::attest(&p, &home, &cwd()?, change, out, sign)?;
    let mut r = Reply::new();
    let signed = if done.signed { ", signed" } else { "" };
    r.line(format!("attestation: {} ({} statements{signed})", out.display(), done.lines));
    r.set("file", out.display().to_string());
    r.set("statements", done.lines);
    if done.signed {
        r.set("signature", format!("{}.sig", out.display()));
    }
    Ok(r)
}
