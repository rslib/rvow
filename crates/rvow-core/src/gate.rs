//! `rvow gate`: the deterministic check behind the Stop hook, pre-push and
//! archive.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::amend;
use crate::change::{self, LEDGER, Stage};
use crate::config;
use crate::diag::Severity;
use crate::drift;
use crate::error::Result;
use crate::fingerprint::{self, globset};
use crate::git;
use crate::ledger::{self, Kind};
use crate::proof::{self, Active, Now, State};
use crate::runner;
use crate::store::{Mode, Project, REPO_DIR};

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
    let (all, _) = change::list(project)?;
    for c in all.iter().filter(|c| c.stage == Stage::Backlog) {
        let (events, _) = ledger::read(&c.path.join(LEDGER))?;
        if events.iter().any(|e| e.kind == Kind::Vow) {
            r.failures.push(format!(
                "{} has a vow but sits in backlog/; it was moved by hand",
                c.id
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

        crate::amend::amend(&p, &home, &repo, None, "simpler check").unwrap();
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
}
