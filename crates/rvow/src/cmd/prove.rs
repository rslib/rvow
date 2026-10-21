use rvow_core::error::{Error, Result};
use rvow_core::gate::{self, Options};
use rvow_core::ledger::Run;
use rvow_core::proof;
use rvow_core::store;

use super::{branch_label, cwd, project};
use crate::output::Reply;
use crate::toon::Obj;

/// Adds one proof run's human line and returns its row and warnings.
pub fn proof_run(r: &mut Reply, p: &proof::Proved) -> (Obj, Vec<String>) {
    let o = &p.outcome;
    let tests = o.tests_run.map_or("unknown".to_string(), |n| n.to_string());
    let verdict = if p.proved {
        "proved".to_string()
    } else if p.baseline_recorded {
        "baseline recorded".to_string()
    } else if p.mutants.as_ref().is_some_and(|check| !check.passed()) {
        "failed: mutation check".to_string()
    } else if p.private_check.as_ref().is_some_and(|check| !check.passed) {
        "failed: private check".to_string()
    } else if o.timed_out {
        "failed: timed out".to_string()
    } else if o.passed() {
        match &p.junit {
            Some(rvow_core::junit::Check::Failed(why)) => format!("failed: junit report: {why}"),
            _ => "failed: the command passed but ran 0 tests".to_string(),
        }
    } else {
        format!("failed: exit {}", o.exit.map_or("signal".into(), |e| e.to_string()))
    };
    r.line(format!("{} {verdict} ({tests} tests, {} ms, event {})", p.task, o.duration_ms, p.event));
    let mut warnings: Vec<String> = p.lfs_pointers.iter().map(|f| format!("{f} is a Git LFS pointer, run git lfs pull")).collect();
    if let Some(note) = &p.note {
        r.line(format!("note: {note}"));
    }
    if let Some(hint) = &p.hint {
        r.line(format!("hint: {hint}"));
    }
    if p.baseline_recorded {
        r.line("baseline: recorded: the check fails before the work, as it must; do the work, then prove again");
    }
    let junit = match &p.junit {
        Some(rvow_core::junit::Check::Failed(why)) => {
            r.line(format!("junit: {why}"));
            Some(why.clone())
        }
        _ => None,
    };
    for w in &warnings {
        r.line(format!("warning: {w}"));
    }
    if p.looping {
        let hint = format!(
            "the same failure {} times in a row; if the task cannot pass as written, run rvow block {} --reason \"...\"",
            proof::LOOP_RUNS,
            p.task
        );
        r.line(format!("hint: {hint}"));
        warnings.push(hint);
    }
    let row = obj! {
        "task" => p.task.to_string(),
        "result" => verdict,
        "tests" => o.tests_run,
        "ms" => o.duration_ms,
        "event" => p.event,
        "junit" => junit,
        "note" => p.note.clone(),
    };
    (row, warnings)
}

pub fn prove(task: &str) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let proved = proof::prove(&p, &home, &cwd()?, task, &mut std::io::stderr())?;
    let mut r = Reply::new();
    let (row, warnings) = proof_run(&mut r, &proved);
    r.data = row;
    if !warnings.is_empty() {
        r.set("warnings", warnings);
    }
    match &proved.next {
        Some(next) => {
            r.text.push_str(&format!("next: {next}"));
            r.set("next", next.lines().collect::<Vec<_>>());
        }
        None => {
            let done = "every task on this branch is proved or blocked";
            r.line(format!("next: {done}"));
            r.set("next", done);
            r.help("Run `rvow gate` to check the branch before push");
        }
    }
    if proved.baseline_recorded {
        r.set("baseline", "recorded");
    }
    r.ok = proved.proved || proved.baseline_recorded;
    Ok(r)
}

/// The background re-prove: proves the stale tasks of every change on the
/// branch whose other tasks are proved, unless another run holds the slot.
pub fn background() -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let cwd = cwd()?;
    let mut r = Reply::new();
    let Some(_slot) = rvow_core::reprove::claim(&p)? else {
        r.line("a background re-prove already runs");
        return Ok(r);
    };
    for change in rvow_core::reprove::ready(&p, &home, &cwd)? {
        match rvow_core::archive::reprove(&p, &home, &cwd, &change) {
            Ok(proved) => r.line(format!("{change}: reproved {}", proved.join(", "))),
            Err(e) => r.line(format!("{change}: {}: {}", e.code, e.message)),
        }
    }
    Ok(r)
}

pub fn prove_stale() -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let cwd = cwd()?;
    let stale = proof::stale_tasks(&p, &home, &cwd)?;
    let mut r = Reply::new();
    if stale.is_empty() {
        r.line("no stale tasks");
        r.set("proofs", format!("0 stale tasks on branch {}", branch_label()));
        return Ok(r);
    }
    let mut rows = Vec::new();
    let mut all_warnings = Vec::new();
    let mut revalidated = 0;
    for task in &stale {
        if proof::revalidate(&p, &home, &cwd, task, &mut std::io::stderr())?.is_some() {
            r.line(format!("revalidated {task}: its artifacts are unchanged, so its verify did not run"));
            revalidated += 1;
            continue;
        }
        let proved = proof::prove(&p, &home, &cwd, task, &mut std::io::stderr())?;
        let (row, warnings) = proof_run(&mut r, &proved);
        rows.push(row);
        all_warnings.extend(warnings);
        r.ok &= proved.proved;
    }
    let kept = if revalidated > 0 { format!(", {revalidated} revalidated") } else { String::new() };
    r.set("count", format!("{} stale tasks{kept}", rows.len() + revalidated));
    r.table("proofs", rows, "");
    if !all_warnings.is_empty() {
        r.set("warnings", all_warnings);
    }
    Ok(r)
}

pub fn gate(opts: Options, no_cache: bool) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let out = opts.out.clone();
    let run = if no_cache { gate::gate_uncached } else { gate::gate };
    let g = run(&p, &home, &cwd()?, opts, &mut std::io::stderr())?;
    if let Some(path) = out {
        write_runs(&path, &g.runs)?;
    }
    let mut r = Reply::new();
    let mut rows = Vec::new();
    for (task, ms) in &g.timings {
        r.line(format!("rerun {task}: {ms} ms"));
        rows.push(obj! { "task" => task, "ms" => ms });
    }
    for task in &g.proven {
        r.line(format!("proven {task}: its proof is current and its files are committed, so it is not run again"));
    }
    for task in &g.cached {
        r.line(format!("cached {task}: an earlier clean pass has the same key"));
    }
    for w in &g.warnings {
        r.line(format!("warning: {w}"));
    }
    for f in &g.failures {
        r.line(format!("fail: {f}"));
    }
    r.ok = g.passed();
    if r.ok {
        r.line(format!("gate passed ({} active changes on this branch)", g.changes));
    } else {
        r.line(format!("gate failed: {} problems", g.failures.len()));
        r.help("Run `rvow status` to see each task's state");
    }
    r.set("passed", r.ok);
    r.set("changes", g.changes);
    if !g.cached.is_empty() {
        r.set("cached", g.cached);
    }
    if !g.proven.is_empty() {
        r.set("proven", g.proven);
    }
    if !rows.is_empty() {
        r.table("reruns", rows, "");
    }
    if !g.warnings.is_empty() {
        r.set("warnings", g.warnings);
    }
    if !g.failures.is_empty() {
        r.set("failures", g.failures);
    }
    Ok(r)
}

pub fn bundle(path: &std::path::Path) -> Result<Reply> {
    let home = store::home_dir()?;
    let p = project(&home)?;
    let bundle = rvow_core::bundle::export(&p, &home, &cwd()?)?;
    let text = serde_json::to_string_pretty(&bundle).map_err(|e| Error::internal(format!("bundle does not serialize: {e}")))?;
    std::fs::write(path, text).map_err(|e| Error::io(path, e))?;
    let mut reply = Reply::new();
    reply.line(format!("verification bundle: {}", path.display()));
    reply.set("bundle", path.display().to_string());
    reply.help("Run `rvow gate --bundle <file> --out <runs>` in the matching CI checkout");
    Ok(reply)
}

pub fn gate_bundle(path: &std::path::Path, out: Option<&std::path::Path>) -> Result<Reply> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    let bundle = serde_json::from_str(&text)
        .map_err(|e| Error::new("bundle", format!("{}: {e}", path.display()), "export a new bundle from the accepted plan"))?;
    let verified = rvow_core::bundle::verify(&bundle, &cwd()?, &mut std::io::stderr())?;
    if let Some(out) = out {
        write_runs(out, &verified.runs)?;
    }
    let mut reply = Reply::new();
    reply.ok = verified.failures.is_empty();
    reply.line(if reply.ok { "portable gate passed" } else { "portable gate failed" });
    reply.set("passed", reply.ok);
    reply.set("runs", verified.runs.len());
    for failure in &verified.failures {
        reply.line(format!("fail: {failure}"));
    }
    if !verified.failures.is_empty() {
        reply.set("failures", verified.failures);
    }
    Ok(reply)
}

/// Writes each run as one JSON line, the shape `rvow ingest` reads. A fresh
/// file is created; an existing file is replaced.
fn write_runs(path: &std::path::Path, runs: &[Run]) -> Result<()> {
    let mut out = String::new();
    for run in runs {
        let line =
            serde_json::to_string(run).map_err(|e| Error::new("out", format!("run does not serialize: {e}"), "report this as a bug"))?;
        out.push_str(&line);
        out.push('\n');
    }
    std::fs::write(path, out).map_err(|e| Error::io(path, e))
}
