use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use rvow_core::error::{Error, Result};
use rvow_core::ledger::{self, Run};
use rvow_core::store;

use crate::output::Reply;
use crate::toon::Obj;

/// Adds the runs that a `gate --out` file carried to the ledger of the project
/// each run names. Home mode only: the file is read from a CI checkout where
/// the store is not reachable, and this runs where the store lives.
pub fn ingest(file: &Path) -> Result<Reply> {
    let home = store::home_dir()?;
    let file_handle = File::open(file).map_err(|e| Error::io(file, e))?;
    let reader = BufReader::new(file_handle);
    let mut rows: Vec<Obj> = Vec::new();
    let mut count = 0usize;
    let mut skipped = 0usize;
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let run: Run = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        if !rvow_core::bundle::safe_ingest_project(&run.project) {
            skipped += 1;
            continue;
        }
        let project_root = home.join(&run.project);
        if !project_root.join(store::PROJECT_FILE).is_file() {
            skipped += 1;
            continue;
        }
        let event = match ledger::ingest_run(&project_root, &run) {
            Ok(ev) => ev,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        count += 1;
        let proved = run.data.get("proved").and_then(|v| v.as_bool()).unwrap_or(false);
        let task = run.data.get("task").and_then(|v| v.as_str()).unwrap_or("").to_string();
        rows.push(obj! {
            "project" => run.project,
            "change" => run.change,
            "task" => task,
            "proved" => proved,
            "event" => event.hash,
        });
    }
    if count == 0 && skipped == 0 {
        return Err(Error::new(
            "ingest",
            format!("{} holds no runs", file.display()),
            "write the file with rvow gate --rerun --out <file> and ingest that",
        ));
    }
    let mut r = Reply::new();
    r.line(format!("ingested {count} runs, {skipped} skipped"));
    r.set("count", count);
    r.set("skipped", skipped);
    if !rows.is_empty() {
        r.table("runs", rows, "");
    }
    r.ok = count > 0 || skipped == 0;
    Ok(r)
}
