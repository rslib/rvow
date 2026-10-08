//! `rvow gate`: the deterministic check behind the Stop hook, pre-push and
//! archive.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::change;
use crate::config;
use crate::error::Result;
use crate::fingerprint;
use crate::git;
use crate::ledger::Run;
use crate::proof::{self, Active, Now};
use crate::runner;
use crate::store::Project;

pub mod cache;
mod check;
mod rerun;
mod signature;

use check::check_scope;
use rerun::{Archived, Lazy, Scope, prove_data, rerun_archived};

#[cfg(test)]
use crate::change::Stage;
#[cfg(test)]
use crate::record;
#[cfg(test)]
use crate::vow::requirement_hash;

#[path = "gate_hook.rs"]
mod hook_integrity;

pub(crate) use check::{check_change, check_lifecycle, check_record};
pub use signature::{bound_confirm_payload, confirm_payload, sign};

#[derive(Debug, Default)]
pub struct Report {
    pub failures: Vec<String>,
    pub warnings: Vec<String>,
    /// Each rerun task and how long it took, so slow checks are visible.
    pub timings: Vec<(String, u64)>,
    /// Archived verification failures, reported separately from active tasks.
    pub archived: Vec<String>,
    /// The rerun results of the branch's tasks, when `--out` asked for them:
    /// each carries the data a `rvow prove` event would, plus the change and
    /// project that route it back on ingest.
    pub runs: Vec<crate::ledger::Run>,
    /// Tasks that the cache answered without a run.
    pub cached: Vec<String>,
    pub changes: usize,
}

impl Report {
    pub fn passed(&self) -> bool {
        self.failures.is_empty() && self.archived.is_empty()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub strict: bool,
    pub rerun: bool,
    /// Write the rerun results to this file, for `rvow ingest` to read.
    pub out: Option<PathBuf>,
}

/// Checks the active changes of the current branch.
pub fn gate(project: &Project, home: &Path, cwd: &Path, opts: Options, echo: &mut dyn Write) -> Result<Report> {
    gate_for(project, home, cwd, opts, None, true, echo)
}

/// Like `gate`, but a rerun runs every task and ignores the cache.
pub fn gate_uncached(project: &Project, home: &Path, cwd: &Path, opts: Options, echo: &mut dyn Write) -> Result<Report> {
    gate_for(project, home, cwd, opts, None, false, echo)
}

fn gate_for(
    project: &Project,
    home: &Path,
    cwd: &Path,
    opts: Options,
    target: Option<&crate::id::Id>,
    use_cache: bool,
    echo: &mut dyn Write,
) -> Result<Report> {
    let top = git::toplevel(cwd)?;
    let (living, _) = change::load_living(project)?;
    let changes = proof::active(project, &living)?;
    let mut r = Report::default();

    for c in &changes {
        if c.vow().is_none() {
            r.failures.push(format!("{} is in active/ without a vow event; it was moved by hand", c.dir.id));
        }
    }
    check_lifecycle(project, &mut r)?;
    check_record(project, &living, &mut r)?;

    if let Some(failure) = hook_integrity::failure(&top) {
        r.failures.push(failure);
    }
    let branch = git::branch(&top)?;
    if branch.is_none() && !changes.is_empty() {
        r.failures.push(format!("HEAD is detached while {} changes are active; check out a branch", changes.len()));
    }
    let queue = branch.as_deref().map_or_else(Vec::new, |branch| proof::on_branch(&changes, branch));
    if let Some(id) = target
        && !queue.iter().any(|change| &change.dir.id == id)
    {
        r.failures.push(format!("{id} is not active on the current branch; check out its accepted branch before archiving"));
    }
    r.changes = queue.iter().filter(|change| target.is_none_or(|id| &change.dir.id == id)).count();

    let config = config::load(project, home)?;
    let now = Now { top: &top, config: &config, env: fingerprint::env(&top, &config)?, living: &living };
    for c in queue.iter().filter(|change| target.is_none_or(|id| &change.dir.id == id)) {
        check_change(project, c, &now, &opts, &mut r)?;
    }
    if let Some(branch) = branch.as_deref()
        && !queue.is_empty()
    {
        check_scope(project, &top, &config.base, branch, &living, &queue, &mut r)?;
    }

    let env: Vec<(String, String)> = if opts.rerun { config.rerun_env.clone().into_iter().collect() } else { Vec::new() };
    let mut run_dir = top.clone();
    let scope;
    let _keep_tree;
    if opts.rerun {
        let policy = config::cache_policy(project, home)?;
        let links = config::rerun_links(project, home)?;
        let linked = !links.is_empty() && !policy.with_links;
        if use_cache && opts.out.is_none() && linked {
            r.warnings.push("the rerun cache is off because rerun_links links build output; set cache_with_links = true to keep it".into());
        }
        let mut cache =
            if !use_cache || opts.out.is_some() || policy.off || linked { None } else { Some(cache::Cache::open(project, home)?) };
        if let Some(cache) = &cache {
            r.warnings.extend(cache.warnings().iter().cloned());
        }
        let mut plan = Vec::new();
        for c in &queue {
            let mut own = std::collections::BTreeMap::new();
            if cache.is_some() {
                for t in &c.loaded.tasks {
                    let reqs = proof::covered(t, &c.loaded, &living);
                    let key = cache::cache_key(project, home, &top, &config, t, &reqs, &now.env).unwrap_or_else(|e| {
                        r.warnings.push(format!("{} is not cached: {}", t.id, e.message));
                        None
                    });
                    own.insert(t.id.to_string(), key);
                }
            }
            let keys = cache::dependent_keys(&c.loaded.tasks, &own);
            for t in &c.loaded.tasks {
                let key = keys.get(&t.id.to_string()).cloned().flatten();
                if cache.is_some() && key.is_none() && own.get(&t.id.to_string()).is_some_and(Option::is_some) {
                    r.warnings
                        .push(format!("{} is not cached: its depends_on names an unknown task, a cycle or an uncacheable task", t.id));
                }
                let max_age = t.max_age.unwrap_or(policy.max_age).as_millis() as u64;
                let hit = match (&cache, &key) {
                    (Some(cache), Some(key)) => cache.lookup(&t.id.to_string(), key, crate::ledger::now_ms(), max_age),
                    _ => None,
                };
                plan.push((c, t, key, hit));
            }
        }
        let misses: Vec<&crate::tasks::Task> = plan.iter().filter(|p| p.3.is_none()).map(|p| p.1).collect();
        scope = Scope::new(&top, &config.base, &misses)?;
        let mut setup = Vec::new();
        for step in &config.setup {
            if scope.wants(step)? {
                setup.push(step.run().to_string());
            }
        }
        let mut lazy = Lazy::new(&top, links, setup, config.timeout(), &env);
        if plan.is_empty() && lazy.dir(&mut r, echo)?.is_none() {
            return Ok(r);
        }
        let out_dir = match &opts.out {
            Some(_) => match lazy.dir(&mut r, echo)? {
                Some(dir) => Some(dir),
                None => return Ok(r),
            },
            None => None,
        };
        let run_projection = match &out_dir {
            Some(dir) => {
                let folder = project
                    .root
                    .strip_prefix(home)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| project.root.to_string_lossy().into_owned());
                let run_now = Now { top: dir, config: &config, env: fingerprint::env_with(dir, &config, &env)?, living: &living };
                Some((folder, run_now))
            }
            None => None,
        };
        for (c, t, key, hit) in &plan {
            let id = t.id.to_string();
            if let Some(hit) = hit {
                let _ = writeln!(echo, "cached {id}: passed {} ms for this key", hit.duration_ms);
                r.cached.push(id);
                continue;
            }
            let Some(dir) = lazy.dir(&mut r, echo)? else { return Ok(r) };
            let inputs = run_projection.as_ref().map(|(_, now)| now.inputs(t)).transpose()?;
            let cmd = t.verify.as_deref().unwrap_or("");
            let _ = writeln!(echo, "rerun {id}: {cmd}");
            let o = runner::run(cmd, &dir, &env, t.timeout.unwrap_or(config.timeout()), echo)?;
            r.timings.push((id.clone(), o.duration_ms));
            if let (Some(cache), Some(key)) = (&mut cache, key) {
                let lock = project.lock()?;
                cache.record_pass(&lock, &id, key, &o, &proof::machine(&config).to_string())?;
            }
            if let (Some((folder, run_now)), Some(inputs)) = (&run_projection, &inputs) {
                r.runs.push(Run { project: folder.clone(), change: c.dir.id.to_string(), data: prove_data(c, t, &o, run_now, inputs) });
            }
            if !(o.passed() && o.tests_run != Some(0)) {
                let why = if o.timed_out {
                    "timed out".to_string()
                } else if o.passed() {
                    "ran 0 tests".to_string()
                } else {
                    format!("exit {}", o.exit.map_or("signal".into(), |e| e.to_string()))
                };
                r.failures.push(format!("{id} failed on rerun ({why}): {cmd}"));
            }
        }
        let archived = Archived { project, home, now: &now, env: &env, max_age: policy.max_age };
        rerun_archived(&archived, &mut lazy, &mut cache, &mut r, echo)?;
        if lazy.failed() {
            return Ok(r);
        }
        run_dir = lazy.current();
        _keep_tree = Some(lazy);
    } else {
        scope = Scope::new(&top, &config.base, &[])?;
        _keep_tree = None;
    }
    for step in &config.checks {
        if !scope.wants(step)? {
            continue;
        }
        let cmd = step.run();
        let _ = writeln!(echo, "check: {cmd}");
        let o = runner::run(cmd, &run_dir, &env, config.timeout(), echo)?;
        if !o.passed() {
            r.failures.push(format!("check failed: {cmd}"));
        }
    }
    Ok(r)
}
/// Checks repository integrity and scope while requiring proofs only for the
/// selected change, so other unfinished changes can remain on the branch.
pub fn change_gate(project: &Project, home: &Path, cwd: &Path, c: &Active) -> Result<Report> {
    gate_for(project, home, cwd, Options { strict: true, rerun: false, out: None }, Some(&c.dir.id), true, &mut std::io::sink())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;
    use crate::proof::prove;
    use crate::proof::tests::accepted;

    fn run_gate(p: &Project, home: &Path, repo: &Path, opts: Options) -> Report {
        gate(p, home, repo, opts, &mut std::io::sink()).unwrap()
    }

    #[test]
    fn rerun_archived_reports_a_regression_the_branch_caused() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        git(&repo, &["add", "out.txt"]);
        git(&repo, &["commit", "-q", "-m", "out"]);
        prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        let id = proof::active(&p, &change::load_living(&p).unwrap().0).unwrap()[0].dir.id.to_string();
        crate::archive::archive(&p, &home, &repo, &id).unwrap();

        // A later change on a feature branch off main touches more.txt only.
        git(&repo, &["checkout", "-q", "-b", "feat/next"]);
        let c = change::new(&p, "Next", "feat/next", false).unwrap();
        std::fs::write(
            c.path.join("tasks.md"),
            "- TSK-new2 Write more\n  verify: grep -q ok more.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: more.txt, out.txt\n",
        )
        .unwrap();
        let prepared = crate::vow::prepare(&p, &home, c, &repo, &mut std::io::sink()).unwrap();
        crate::vow::commit(&p, prepared).unwrap();
        std::fs::write(repo.join("more.txt"), "ok").unwrap();
        git(&repo, &["add", "more.txt"]);
        git(&repo, &["commit", "-q", "-m", "more"]);

        // Nothing touched yet: the archived task is not rerun.
        let opts = Options { strict: true, rerun: true, out: None };
        let r = run_gate(&p, &home, &repo, opts.clone());
        assert!(r.passed() && r.archived.is_empty(), "{r:?}");

        // The feature branch edits out.txt in its scope but breaks the archived requirement.
        std::fs::write(repo.join("out.txt"), "bad").unwrap();
        git(&repo, &["add", "out.txt"]);
        git(&repo, &["commit", "-q", "-m", "break"]);
        let r = run_gate(&p, &home, &repo, opts);
        assert!(!r.passed(), "archived regressions must block: {r:?}");
        assert_eq!(r.archived.len(), 1, "{r:?}");
        assert!(r.archived[0].contains(&task) && r.archived[0].contains("failed on rerun"), "{r:?}");
    }

    #[test]
    fn an_archived_task_is_not_rerun_while_its_files_are_unchanged() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        git(&repo, &["add", "out.txt"]);
        git(&repo, &["commit", "-q", "-m", "out"]);
        prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        let id = proof::active(&p, &change::load_living(&p).unwrap().0).unwrap()[0].dir.id.to_string();
        crate::archive::archive(&p, &home, &repo, &id).unwrap();

        // A feature branch touches the archived task's file but keeps it valid.
        git(&repo, &["checkout", "-q", "-b", "feat/next"]);
        std::fs::write(repo.join("out.txt"), "ok ok").unwrap();
        git(&repo, &["add", "out.txt"]);
        git(&repo, &["commit", "-q", "-m", "touch"]);

        let opts = Options { strict: true, rerun: true, out: None };
        let first = run_gate(&p, &home, &repo, opts.clone());
        assert!(first.timings.iter().any(|(t, _)| t == &task), "the first gate runs it: {first:?}");
        let second = run_gate(&p, &home, &repo, opts);
        assert!(second.cached.contains(&task), "the second gate answers from the cache: {second:?}");
        assert!(second.timings.iter().all(|(t, _)| t != &task), "{second:?}");
    }

    #[test]
    fn a_failed_run_keeps_the_gate_failing() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        // The task fails: out.txt does not hold the required value.
        std::fs::write(repo.join("out.txt"), "bad").unwrap();
        crate::proof::prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(r.failures.iter().any(|f| f.contains("is failed")), "{r:?}");
        // A later, still-failing run does not let the gate pass.
        crate::proof::prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(r.failures.iter().any(|f| f.contains("is failed")), "{r:?}");
    }

    #[test]
    fn gate_records_a_timing_per_rerun_task() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        git(&repo, &["add", "out.txt"]);
        git(&repo, &["commit", "-q", "-m", "out"]);
        let r = run_gate(&p, &home, &repo, Options { strict: true, rerun: true, out: None });
        assert!(r.passed(), "{r:?}");
        assert!(r.timings.iter().any(|(id, _)| id.contains(&task)), "no timing for {task}: {:?}", r.timings);
    }

    #[test]
    fn gate_follows_task_states_scope_and_rerun() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(r.failures.iter().any(|f| f.ends_with("is missing")), "{r:?}");

        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(r.passed(), "{r:?}");

        std::fs::write(repo.join("stray.txt"), "x").unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(r.failures[0].contains("stray.txt"), "{r:?}");
        std::fs::remove_file(repo.join("stray.txt")).unwrap();

        // The rerun uses HEAD, where out.txt is not committed yet.
        let opts = Options { strict: true, rerun: true, out: None };
        let r = run_gate(&p, &home, &repo, opts.clone());
        assert!(r.failures.iter().any(|f| f.contains("failed on rerun")), "{r:?}");
        git(&repo, &["add", "out.txt"]);
        git(&repo, &["commit", "-q", "-m", "out"]);
        let r = run_gate(&p, &home, &repo, opts);
        assert!(r.passed(), "{r:?}");
        assert_eq!(r.timings.len(), 1);
        let list = git::run(&repo, &["worktree", "list"]).unwrap();
        assert_eq!(list.lines().count(), 1, "the rerun tree is removed");

        std::fs::write(repo.join("out.txt"), "ok\n").unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(r.passed() && !r.warnings.is_empty(), "{r:?}");
        let r = run_gate(&p, &home, &repo, Options { strict: true, rerun: false, out: None });
        assert!(!r.passed(), "strict fails on stale");
    }

    #[test]
    fn signed_confirmation_refused_when_unsigned_or_not_allowed() {
        let tmp = TempDir::new();
        let keys = tmp.0.join("keys");
        std::fs::create_dir_all(&keys).unwrap();
        run_keygen(&keys.join("trusted"));
        run_keygen(&keys.join("rogue"));
        let allowed_signers = keys.join("allowed_signers");
        std::fs::write(&allowed_signers, signers_line(&keys.join("trusted.pub"), "alice@example")).unwrap();

        // Each case gets a fresh project so earlier Confirm events cannot leak.
        // The TempDir stays in the tuple so the case's files live until the
        // caller is done with them.
        let case = || {
            let (tmp, p, repo, _) = accepted();
            let home = tmp.0.join("home");
            let dir = std::fs::read_dir(p.root.join("active")).unwrap().next().unwrap().unwrap().path();
            let tasks = dir.join("tasks.md");
            let text = std::fs::read_to_string(&tasks).unwrap();
            std::fs::write(&tasks, text.replace("Write ok", "Write one")).unwrap();
            assert!(crate::amend::amend(&p, &home, &repo, None, "edit", None).is_ok());
            std::fs::write(repo.join("out.txt"), "ok").unwrap();
            let (living, _) = crate::change::load_living(&p).unwrap();
            let changes = crate::proof::active(&p, &living).unwrap();
            crate::proof::prove(&p, &home, &repo, &changes[0].loaded.tasks.first().unwrap().id.to_string(), &mut std::io::sink()).unwrap();
            let amd = crate::amend::pending(&changes).first().unwrap().id.to_string();
            std::fs::write(
                p.root.join("config.toml"),
                format!("require_signature = true\nallowed_signers = \"{}\"\n", allowed_signers.display()),
            )
            .unwrap();
            (tmp, p, home, repo, dir, amd)
        };

        // Append one Confirm event and report whether the gate calls it signed.
        let gate_signed_ok = |data: serde_json::Map<String, serde_json::Value>| {
            let (tmp, p, home, repo, dir, _) = case();
            let lock = p.lock().unwrap();
            crate::ledger::append(&dir.join(crate::change::LEDGER), &lock, crate::ledger::Kind::Confirm, data).unwrap();
            drop(lock);
            let r = run_gate(&p, &home, &repo, Options::default());
            drop(tmp);
            !r.failures.iter().any(|f| f.contains("not signed with an allowed key"))
        };

        // 1. unsigned is refused.
        let mut data = serde_json::Map::new();
        data.insert("amendment".into(), "AMD-x".into());
        data.insert("decision".into(), "accept".into());
        assert!(!gate_signed_ok(data), "an unsigned confirm must be refused");

        let (tmp, paid, home, repo, _, amd) = case();
        crate::amend::confirm_signed(
            &paid,
            &[amd],
            crate::amend::Decision::Accept,
            None,
            Some(crate::amend::Signing { key: &keys.join("rogue"), signer: "alice@example" }),
        )
        .unwrap();
        let r = run_gate(&paid, &home, &repo, Options::default());
        assert!(r.failures.iter().any(|f| f.contains("not signed with an allowed key")), "a rogue-key signature must be refused");
        drop(tmp);

        // Legacy signatures remain invalid even when the signing key is allowed.
        let (tmp, p, home, repo, dir, amd) = case();
        let lock = p.lock().unwrap();
        let mut data = serde_json::Map::new();
        data.insert("amendment".into(), amd.clone().into());
        data.insert("decision".into(), "accept".into());
        data.insert("signer".into(), "alice@example".into());
        data.insert("signature".into(), sign(&keys.join("trusted"), &confirm_payload(&amd, "accept")).unwrap().into());
        crate::ledger::append(&dir.join(crate::change::LEDGER), &lock, crate::ledger::Kind::Confirm, data).unwrap();
        drop(lock);
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(r.failures.iter().any(|f| f.contains("not signed with an allowed key")), "a legacy signature must be refused");
        drop(tmp);

        let (tmp, p, home, repo, _, amd) = case();
        crate::amend::confirm_signed(
            &p,
            &[amd],
            crate::amend::Decision::Accept,
            None,
            Some(crate::amend::Signing { key: &keys.join("trusted"), signer: "alice@example" }),
        )
        .unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(
            !r.failures.iter().any(|f| f.contains("not signed with an allowed key")),
            "a content-bound allowed signature must be accepted"
        );
        drop(tmp);
    }

    fn run_keygen(path: &Path) {
        let st = std::process::Command::new("ssh-keygen")
            .args(["-t", "ed25519", "-N", "", "-f"])
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(st.success(), "ssh-keygen failed for {}", path.display());
    }

    /// The allowed-signers line for `pub_path`: principal, key type, key.
    fn signers_line(pub_path: &Path, principal: &str) -> String {
        let pub_text = std::fs::read_to_string(pub_path).unwrap();
        let fields: Vec<&str> = pub_text.split_whitespace().take(2).collect();
        assert_eq!(fields.len(), 2, "{pub_text}");
        format!("{principal} {} {}\n", fields[0], fields[1])
    }

    #[test]
    fn archived_change_files_are_in_scope() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        let id = proof::active(&p, &change::load_living(&p).unwrap().0).unwrap()[0].dir.id.to_string();
        crate::archive::archive(&p, &home, &repo, &id).unwrap();

        let c = change::new(&p, "Next", "main", false).unwrap();
        std::fs::write(
            c.path.join("tasks.md"),
            "- TSK-new2 Write more\n  covers: REQ-new2\n  verify: grep -q ok more.txt && echo 'test result: ok. 1 passed; 0 failed;'\n  files: more.txt\n",
        )
        .unwrap();
        std::fs::write(
            c.path.join("specs/more.md"),
            "## ADD REQ-new2 More\nrvow SHALL write more.\n### Scenario: s\n- WHEN it runs\n- THEN more.txt holds ok\n",
        )
        .unwrap();
        let prepared = crate::vow::prepare(&p, &home, c, &repo, &mut std::io::sink()).unwrap();
        crate::vow::commit(&p, prepared).unwrap();

        std::fs::write(repo.join("more.txt"), "ok").unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(!r.failures.iter().any(|f| f.contains("outside")), "{r:?}");
        std::fs::write(repo.join("stray.txt"), "x").unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        let outside = r.failures.iter().find(|f| f.contains("outside")).unwrap();
        assert!(outside.contains("stray.txt") && !outside.contains("out.txt"), "{r:?}");
    }

    #[test]
    fn branch_edits_detached_head_and_hand_moves_fail() {
        let (tmp, p, repo, _) = accepted();
        let home = tmp.0.join("home");
        let dir = std::fs::read_dir(p.root.join("active")).unwrap().next().unwrap().unwrap().path();
        let prop = dir.join("proposal.md");
        let text = std::fs::read_to_string(&prop).unwrap();
        std::fs::write(&prop, text.replace("branch: main", "branch: elsewhere")).unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(r.failures.iter().any(|f| f.contains("branch")), "{r:?}");
        std::fs::write(&prop, &text).unwrap();

        git(&repo, &["checkout", "-q", "--detach"]);
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(r.failures[0].contains("detached"), "{r:?}");
        git(&repo, &["checkout", "-q", "main"]);

        let back = p.root.join("backlog").join(dir.file_name().unwrap());
        std::fs::rename(&dir, &back).unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(r.failures[0].contains("sits in backlog"), "{r:?}");
    }

    #[test]
    fn drift_blocks_until_the_user_confirms() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        let dir = std::fs::read_dir(p.root.join("active")).unwrap().next().unwrap().unwrap().path();
        let tasks = dir.join("tasks.md");
        let text = std::fs::read_to_string(&tasks).unwrap();
        std::fs::write(&tasks, text.replace("grep -q ok out.txt", "true")).unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(r.failures.iter().any(|f| f.contains("weaken drift")), "{r:?}");

        crate::amend::amend(&p, &home, &repo, None, "simpler check", None).unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(r.failures.iter().any(|f| f.contains("waits for the user")), "{r:?}");

        crate::amend::confirm(
            &p,
            &[],
            crate::amend::Decision::Accept,
            Some(&{
                let (living, _) = change::load_living(&p).unwrap();
                crate::amend::digest(&crate::amend::pending(&proof::active(&p, &living).unwrap()))
            }),
        )
        .unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(r.failures.iter().all(|f| !f.contains("drift") && !f.contains("waits")), "{r:?}");
        assert!(r.failures.iter().any(|f| f.contains("is stale")) || r.warnings.iter().any(|w| w.contains("stale")), "{r:?}");
    }

    #[test]
    fn hooks_path_must_run_the_gate() {
        let (tmp, p, repo, _) = accepted();
        let home = tmp.0.join("home");
        git(&repo, &["config", "core.hooksPath", ".husky"]);
        let hooks_path_failure = |r: &Report| r.failures.iter().any(|f| f.contains("core.hooksPath"));
        assert!(hooks_path_failure(&run_gate(&p, &home, &repo, Options::default())));
        std::fs::create_dir_all(repo.join(".husky")).unwrap();
        let hook = repo.join(".husky/pre-push");
        std::fs::write(&hook, "npx lint\n'/opt/rvow' gate --strict --rerun\n").unwrap();
        assert!(hooks_path_failure(&run_gate(&p, &home, &repo, Options::default())));
        let binary = std::env::current_exe().unwrap();
        std::fs::write(&hook, crate::install::pre_push_script(&binary).unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(!hooks_path_failure(&run_gate(&p, &home, &repo, Options::default())));
    }

    #[test]
    fn checks_run_and_fail() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        std::fs::write(p.root.join("config.toml"), "checks = [\"false\"]\n").unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(r.failures.iter().any(|f| f == "check failed: false"), "{r:?}");
    }

    #[test]
    fn hand_moves_into_the_archive_fail() {
        let (tmp, p, repo, _) = accepted();
        let home = tmp.0.join("home");
        let dir = std::fs::read_dir(p.root.join("active")).unwrap().next().unwrap().unwrap().path();
        let name = dir.file_name().unwrap();
        for (stage, want) in [
            (Stage::Done, "sits in archive/done/ without an archive event"),
            (Stage::Dropped, "sits in archive/dropped/ without a drop event"),
            (Stage::Superseded, "sits in archive/superseded/ without a supersede event"),
        ] {
            let to = p.root.join(stage.dir()).join(name);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            let src = Stage::ALL.iter().map(|s| p.root.join(s.dir()).join(name)).find(|d| d.is_dir()).unwrap();
            std::fs::rename(&src, &to).unwrap();
            let r = run_gate(&p, &home, &repo, Options::default());
            assert!(r.failures.iter().any(|f| f.contains(want)), "{r:?}");
        }
    }

    #[test]
    fn living_specs_must_match_the_record() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        let spec = p.root.join("specs/cap/spec.md");
        std::fs::create_dir_all(spec.parent().unwrap()).unwrap();
        let text = "# Cap\n\n## REQ-q8w2c7 Old\nrvow SHALL stay.\n### Scenario: s\n- WHEN a\n- THEN b\n";
        std::fs::write(&spec, text).unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(r.passed(), "an empty record skips the check: {r:?}");

        let (living, _) = change::load_living(&p).unwrap();
        let mut rec = record::Record::new();
        rec.insert("REQ-q8w2c7".into(), record::Entry { hash: requirement_hash(&living[0].1.requirements[0]), legacy: false });
        rec.insert("REQ-7xq2a5".into(), record::Entry { hash: "x".into(), legacy: false });
        record::save(&p, &p.lock().unwrap(), &rec).unwrap();
        let edited = text.replace("stay", "move") + "\n## REQ-h4t6v9 New\nrvow SHALL be.\n### Scenario: s\n- WHEN a\n- THEN b\n";
        std::fs::write(&spec, edited).unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        let outside: Vec<&String> = r.failures.iter().filter(|f| f.ends_with("a living spec was edited outside a change")).collect();
        assert_eq!(outside.len(), 3, "{r:?}");
        assert!(outside[0].contains("REQ-q8w2c7 in specs/cap/spec.md does not match"));
        assert!(outside[1].contains("REQ-h4t6v9 in specs/cap/spec.md is not in"));
        assert!(outside[2].contains("REQ-7xq2a5 is in spec-record.json but in no living spec"));
    }
}
