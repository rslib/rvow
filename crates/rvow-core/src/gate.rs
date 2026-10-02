//! `rvow gate`: the deterministic check behind the Stop hook, pre-push and
//! archive.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::amend;
use crate::change::{self, CapSpec, LEDGER, Stage};
use crate::config;
use crate::diag::Severity;
use crate::drift;
use crate::error::Result;
use crate::fingerprint::{self, globset};
use crate::git;
use crate::ledger::{self, Kind};
use crate::proof::{self, Active, Now, State};
use crate::record;
use crate::runner;
use crate::store::{Mode, Project, REPO_DIR};
use crate::vow::requirement_hash;

const MAX_LISTED: usize = 20;

#[derive(Debug, Default)]
pub struct Report {
    pub failures: Vec<String>,
    pub warnings: Vec<String>,
    /// Each rerun task and how long it took, so slow checks are visible.
    pub timings: Vec<(String, u64)>,
    pub changes: usize,
}

impl Report {
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub strict: bool,
    pub rerun: bool,
}

/// A temporary detached worktree of HEAD, removed on drop. This is the one
/// place where rvow writes to git.
struct Worktree {
    repo: PathBuf,
    path: PathBuf,
}

impl Worktree {
    fn add(repo: &Path) -> Result<Worktree> {
        let mut b = [0u8; 6];
        getrandom::fill(&mut b).expect("operating system random source");
        let name: String = b.iter().map(|x| format!("{x:02x}")).collect();
        let path = std::env::temp_dir().join(format!("rvow-rerun-{name}"));
        let p = path.to_string_lossy().into_owned();
        git::run(
            repo,
            &["worktree", "add", "--detach", "--quiet", &p, "HEAD"],
        )?;
        Ok(Worktree {
            repo: repo.to_path_buf(),
            path,
        })
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        let p = self.path.to_string_lossy().into_owned();
        if git::run(&self.repo, &["worktree", "remove", "--force", &p]).is_err() {
            let _ = std::fs::remove_dir_all(&self.path);
            let _ = git::run(&self.repo, &["worktree", "prune"]);
        }
    }
}

fn base_ref(top: &Path, base: &str) -> Option<String> {
    [
        format!("refs/remotes/origin/{base}"),
        format!("refs/heads/{base}"),
    ]
    .into_iter()
    .find(|r| git::run(top, &["rev-parse", "--verify", "--quiet", r]).is_ok())
}

/// Files that differ from `mb` in the worktree, plus untracked files git does
/// not ignore.
fn changed_files(top: &Path, mb: &str) -> Result<Vec<String>> {
    let mut files: Vec<String> = git::run(top, &["diff", "--name-only", mb])?
        .lines()
        .map(String::from)
        .collect();
    files.extend(
        git::run(top, &["ls-files", "-o", "--exclude-standard"])?
            .lines()
            .map(String::from),
    );
    files.sort();
    files.dedup();
    Ok(files)
}

fn list(items: &[String]) -> String {
    let mut s = items
        .iter()
        .take(MAX_LISTED)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() > MAX_LISTED {
        s.push_str(&format!(" and {} more", items.len() - MAX_LISTED));
    }
    s
}

/// Checks the active changes of the current branch.
pub fn gate(
    project: &Project,
    home: &Path,
    cwd: &Path,
    opts: Options,
    echo: &mut dyn Write,
) -> Result<Report> {
    let top = git::toplevel(cwd)?;
    let (living, _) = change::load_living(project)?;
    let changes = proof::active(project, &living)?;
    let mut r = Report::default();

    for c in &changes {
        if c.vow().is_none() {
            r.failures.push(format!(
                "{} is in active/ without a vow event; it was moved by hand",
                c.dir.id
            ));
        }
    }
    check_lifecycle(project, &mut r)?;
    check_record(project, &living, &mut r)?;

    if let Ok(dir) = git::run(&top, &["config", "--get", "core.hooksPath"])
        && !dir.is_empty()
    {
        let hook = top.join(&dir).join("pre-push");
        let runs_gate =
            std::fs::read_to_string(&hook).is_ok_and(|t| t.contains("rvow") && t.contains("gate"));
        if !runs_gate {
            r.failures.push(format!(
                "core.hooksPath points to {dir}, whose pre-push hook does not run rvow gate; run rvow install or add the line it prints"
            ));
        }
    }
    let Some(branch) = git::branch(&top)? else {
        if !changes.is_empty() {
            r.failures.push(format!(
                "HEAD is detached while {} changes are active; check out a branch",
                changes.len()
            ));
        }
        return Ok(r);
    };
    let queue = proof::on_branch(&changes, &branch);
    r.changes = queue.len();
    if queue.is_empty() {
        return Ok(r);
    }

    let config = config::load(project, home)?;
    let now = Now {
        top: &top,
        config: &config,
        env: fingerprint::env(&top, &config)?,
        living: &living,
    };
    for c in &queue {
        check_change(project, c, &now, opts, &mut r)?;
    }
    check_scope(project, &top, &config.base, &queue, &mut r)?;

    let tree = if opts.rerun {
        Some(Worktree::add(&top)?)
    } else {
        None
    };
    let run_dir = tree.as_ref().map_or(top.clone(), |t| t.path.clone());
    let env: Vec<(String, String)> = if opts.rerun {
        config.rerun_env.clone().into_iter().collect()
    } else {
        Vec::new()
    };
    if opts.rerun {
        for cmd in &config.setup {
            let _ = writeln!(echo, "setup: {cmd}");
            let o = runner::run(cmd, &run_dir, &env, config.timeout(), echo)?;
            if !o.passed() {
                r.failures.push(format!("setup command failed: {cmd}"));
                return Ok(r);
            }
        }
        for c in &queue {
            for t in &c.loaded.tasks {
                let cmd = t.verify.as_deref().unwrap_or("");
                let _ = writeln!(echo, "rerun {}: {cmd}", t.id);
                let o = runner::run(
                    cmd,
                    &run_dir,
                    &env,
                    t.timeout.unwrap_or(config.timeout()),
                    echo,
                )?;
                r.timings.push((t.id.to_string(), o.duration_ms));
                if !(o.passed() && o.tests_run != Some(0)) {
                    let why = if o.timed_out {
                        "timed out".to_string()
                    } else if o.passed() {
                        "ran 0 tests".to_string()
                    } else {
                        format!("exit {}", o.exit.map_or("signal".into(), |e| e.to_string()))
                    };
                    r.failures
                        .push(format!("{} failed on rerun ({why}): {cmd}", t.id));
                }
            }
        }
    }
    for cmd in &config.checks {
        let _ = writeln!(echo, "check: {cmd}");
        let o = runner::run(cmd, &run_dir, &env, config.timeout(), echo)?;
        if !o.passed() {
            r.failures.push(format!("check failed: {cmd}"));
        }
    }
    Ok(r)
}

/// The strict gate for one active change alone, as `archive` runs it: the
/// vow, validation, drift, pending amendments and every task proved.
pub fn change_gate(
    project: &Project,
    home: &Path,
    cwd: &Path,
    c: &Active,
    living: &[CapSpec],
) -> Result<Report> {
    let top = git::toplevel(cwd)?;
    let config = config::load(project, home)?;
    let now = Now {
        top: &top,
        config: &config,
        env: fingerprint::env(&top, &config)?,
        living,
    };
    let mut r = Report {
        changes: 1,
        ..Report::default()
    };
    if c.vow().is_none() {
        r.failures.push(format!(
            "{} is in active/ without a vow event; it was moved by hand",
            c.dir.id
        ));
    }
    let opts = Options {
        strict: true,
        rerun: false,
    };
    check_change(project, c, &now, opts, &mut r)?;
    Ok(r)
}

/// A change outside active/ whose ledger does not hold the event that moves it
/// there was moved by hand.
fn check_lifecycle(project: &Project, r: &mut Report) -> Result<()> {
    let (all, _) = change::list(project)?;
    for c in all.iter().filter(|c| c.stage != Stage::Active) {
        let (events, _) = ledger::read(&c.path.join(LEDGER))?;
        let has = |k: Kind| events.iter().any(|e| e.kind == k);
        let problem = match c.stage {
            Stage::Backlog if has(Kind::Vow) => "has a vow but sits in backlog/",
            Stage::Done if !has(Kind::Archive) => "sits in archive/done/ without an archive event",
            Stage::Dropped if !has(Kind::Drop) => "sits in archive/dropped/ without a drop event",
            Stage::Superseded
                if !events
                    .iter()
                    .any(|e| e.kind == Kind::Supersede && e.data.contains_key("superseded_by")) =>
            {
                "sits in archive/superseded/ without a supersede event"
            }
            _ => continue,
        };
        r.failures
            .push(format!("{} {problem}; it was moved by hand", c.id));
    }
    Ok(())
}

/// Every living requirement matches the hash in spec-record.json. A project
/// from before the record existed has none, and the check waits for one.
fn check_record(project: &Project, living: &[CapSpec], r: &mut Report) -> Result<()> {
    let mut record = record::load(project)?;
    if record.is_empty() {
        return Ok(());
    }
    for (cap, spec) in living {
        for req in &spec.requirements {
            let id = req.id.to_string();
            let what = match record.remove(&id) {
                None => "is not in spec-record.json",
                Some(e) if e.hash != requirement_hash(req) => "does not match its recorded hash",
                Some(_) => continue,
            };
            r.failures.push(format!(
                "{id} in specs/{cap}/spec.md {what}; a living spec was edited outside a change"
            ));
        }
    }
    for id in record.keys() {
        r.failures.push(format!(
            "{id} is in spec-record.json but in no living spec; a living spec was edited outside a change"
        ));
    }
    Ok(())
}

fn check_change(
    project: &Project,
    c: &Active,
    now: &Now,
    opts: Options,
    r: &mut Report,
) -> Result<()> {
    let id = &c.dir.id;
    for (path, d) in c.loaded.errors() {
        r.failures
            .push(format!("{id}: {path}:{}: {}", d.line, d.message));
    }
    for d in &c.ledger_diags {
        let line = format!("{id}: {LEDGER}:{}: {}", d.line, d.message);
        match d.severity() {
            Severity::Error => r.failures.push(line),
            Severity::Warning => r.warnings.push(line),
        }
    }
    for item in drift::drift(project, now.top, now.config, c)?
        .into_iter()
        .filter(|i| i.class.blocks())
    {
        r.failures.push(format!(
            "{id}: {} drift: {}; record it with rvow amend --reason and ask the user to confirm",
            item.class.name(),
            item.what
        ));
    }
    for p in amend::pending(std::slice::from_ref(c)) {
        r.failures.push(format!(
            "{id}: amendment {} waits for the user's confirmation",
            p.id
        ));
    }
    if opts.rerun {
        return Ok(());
    }
    for t in &c.loaded.tasks {
        let s = proof::state(c, t, now)?;
        let line = match &s {
            State::Stale(why) => format!("{} is stale ({})", t.id, why.join(", ")),
            other => format!("{} is {}", t.id, other.name()),
        };
        match s {
            State::Proved => {}
            State::Stale(_) | State::Flaky if !opts.strict => r.warnings.push(line),
            _ => r.failures.push(line),
        }
    }
    Ok(())
}

fn check_scope(
    project: &Project,
    top: &Path,
    base: &str,
    queue: &[&Active],
    r: &mut Report,
) -> Result<()> {
    let Some(base_ref) = base_ref(top, base) else {
        r.failures.push(format!(
            "neither origin/{base} nor {base} exists; set base in config.toml"
        ));
        return Ok(());
    };
    let mb = git::run(top, &["merge-base", "HEAD", &base_ref])?;
    for c in queue {
        let vowed = c
            .vow()
            .and_then(|v| v.data.get("base_commit"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if !vowed.is_empty() && git::run(top, &["merge-base", "--is-ancestor", vowed, &mb]).is_err()
        {
            r.failures.push(format!(
                "{}: the merge base with {base_ref} is not after the base recorded at accept; {base} moved backwards",
                c.dir.id
            ));
        }
    }
    let globs: Vec<String> = queue
        .iter()
        .flat_map(|c| c.loaded.tasks.iter().flat_map(|t| t.files.iter().cloned()))
        .collect();
    let set = globset(&globs)?;
    let repo_dir = format!("{REPO_DIR}/");
    let outside: Vec<String> = changed_files(top, &mb)?
        .into_iter()
        .filter(|f| !(project.meta.mode == Mode::Repo && f.starts_with(&repo_dir)))
        .filter(|f| !set.is_match(f))
        .collect();
    if !outside.is_empty() {
        r.failures.push(format!(
            "{} changed files lie outside every task's files: {}",
            outside.len(),
            list(&outside)
        ));
    }
    Ok(())
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
    fn gate_follows_task_states_scope_and_rerun() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(
            r.failures.iter().any(|f| f.ends_with("is missing")),
            "{r:?}"
        );

        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(r.passed(), "{r:?}");

        std::fs::write(repo.join("stray.txt"), "x").unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(r.failures[0].contains("stray.txt"), "{r:?}");
        std::fs::remove_file(repo.join("stray.txt")).unwrap();

        // The rerun uses HEAD, where out.txt is not committed yet.
        let opts = Options {
            strict: true,
            rerun: true,
        };
        let r = run_gate(&p, &home, &repo, opts);
        assert!(
            r.failures.iter().any(|f| f.contains("failed on rerun")),
            "{r:?}"
        );
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
        let r = run_gate(
            &p,
            &home,
            &repo,
            Options {
                strict: true,
                rerun: false,
            },
        );
        assert!(!r.passed(), "strict fails on stale");
    }

    #[test]
    fn branch_edits_detached_head_and_hand_moves_fail() {
        let (tmp, p, repo, _) = accepted();
        let home = tmp.0.join("home");
        let dir = std::fs::read_dir(p.root.join("active"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
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
        let dir = std::fs::read_dir(p.root.join("active"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let tasks = dir.join("tasks.md");
        let text = std::fs::read_to_string(&tasks).unwrap();
        std::fs::write(&tasks, text.replace("grep -q ok out.txt", "true")).unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(
            r.failures.iter().any(|f| f.contains("weaken drift")),
            "{r:?}"
        );

        crate::amend::amend(&p, &home, &repo, None, "simpler check", None).unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(
            r.failures.iter().any(|f| f.contains("waits for the user")),
            "{r:?}"
        );

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
        assert!(
            r.failures
                .iter()
                .all(|f| !f.contains("drift") && !f.contains("waits")),
            "{r:?}"
        );
        assert!(
            r.failures.iter().any(|f| f.contains("is stale"))
                || r.warnings.iter().any(|w| w.contains("stale")),
            "{r:?}"
        );
    }

    #[test]
    fn hooks_path_must_run_the_gate() {
        let (tmp, p, repo, _) = accepted();
        let home = tmp.0.join("home");
        git(&repo, &["config", "core.hooksPath", ".husky"]);
        let hooks_path_failure =
            |r: &Report| r.failures.iter().any(|f| f.contains("core.hooksPath"));
        assert!(hooks_path_failure(&run_gate(
            &p,
            &home,
            &repo,
            Options::default()
        )));
        std::fs::create_dir_all(repo.join(".husky")).unwrap();
        std::fs::write(
            repo.join(".husky/pre-push"),
            "npx lint\n'/opt/rvow' gate --strict --rerun\n",
        )
        .unwrap();
        assert!(!hooks_path_failure(&run_gate(
            &p,
            &home,
            &repo,
            Options::default()
        )));
    }

    #[test]
    fn checks_run_and_fail() {
        let (tmp, p, repo, task) = accepted();
        let home = tmp.0.join("home");
        std::fs::write(repo.join("out.txt"), "ok").unwrap();
        prove(&p, &home, &repo, &task, &mut std::io::sink()).unwrap();
        std::fs::write(p.root.join("config.toml"), "checks = [\"false\"]\n").unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(
            r.failures.iter().any(|f| f == "check failed: false"),
            "{r:?}"
        );
    }

    #[test]
    fn hand_moves_into_the_archive_fail() {
        let (tmp, p, repo, _) = accepted();
        let home = tmp.0.join("home");
        let dir = std::fs::read_dir(p.root.join("active"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let name = dir.file_name().unwrap();
        for (stage, want) in [
            (
                Stage::Done,
                "sits in archive/done/ without an archive event",
            ),
            (
                Stage::Dropped,
                "sits in archive/dropped/ without a drop event",
            ),
            (
                Stage::Superseded,
                "sits in archive/superseded/ without a supersede event",
            ),
        ] {
            let to = p.root.join(stage.dir()).join(name);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            let src = Stage::ALL
                .iter()
                .map(|s| p.root.join(s.dir()).join(name))
                .find(|d| d.is_dir())
                .unwrap();
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
        let text =
            "# Cap\n\n## REQ-q8w2c7 Old\nrvow SHALL stay.\n### Scenario: s\n- WHEN a\n- THEN b\n";
        std::fs::write(&spec, text).unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        assert!(r.passed(), "an empty record skips the check: {r:?}");

        let (living, _) = change::load_living(&p).unwrap();
        let mut rec = record::Record::new();
        rec.insert(
            "REQ-q8w2c7".into(),
            record::Entry {
                hash: requirement_hash(&living[0].1.requirements[0]),
                legacy: false,
            },
        );
        rec.insert(
            "REQ-7xq2a5".into(),
            record::Entry {
                hash: "x".into(),
                legacy: false,
            },
        );
        record::save(&p, &p.lock().unwrap(), &rec).unwrap();
        let edited = text.replace("stay", "move")
            + "\n## REQ-h4t6v9 New\nrvow SHALL be.\n### Scenario: s\n- WHEN a\n- THEN b\n";
        std::fs::write(&spec, edited).unwrap();
        let r = run_gate(&p, &home, &repo, Options::default());
        let outside: Vec<&String> = r
            .failures
            .iter()
            .filter(|f| f.ends_with("a living spec was edited outside a change"))
            .collect();
        assert_eq!(outside.len(), 3, "{r:?}");
        assert!(outside[0].contains("REQ-q8w2c7 in specs/cap/spec.md does not match"));
        assert!(outside[1].contains("REQ-h4t6v9 in specs/cap/spec.md is not in"));
        assert!(outside[2].contains("REQ-7xq2a5 is in spec-record.json but in no living spec"));
    }
}
