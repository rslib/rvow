//! Portable accepted verification data. Bundles run checks but grant no plan-edit authority.

use crate::error::{Error, Result};
use crate::{change, config, fingerprint, gate, git, guard, ledger, proof, runner, store, tasks, vow};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
enum Format {
    #[serde(rename = "rvow-bundle-v1")]
    V1,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    id: String,
    tasks: String,
    requirements: BTreeMap<String, BTreeMap<String, String>>,
}

/// Exported checks from accepted changes, not an importable plan or ledger.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    format: Format,
    project: String,
    identity: store::Meta,
    config: config::Config,
    changes: Vec<Change>,
}

/// Results remain consumable by the existing run-file ingest command.
pub struct Verification {
    pub runs: Vec<ledger::Run>,
    pub failures: Vec<String>,
}

/// Rejects absolute paths and parent traversal in a run's home-store locator.
pub fn safe_ingest_project(folder: &str) -> bool {
    safe_project(folder)
}

fn safe_project(folder: &str) -> bool {
    !folder.is_empty() && !folder.contains('\\') && Path::new(folder).components().all(|c| matches!(c, Component::Normal(_)))
}

/// Exports only valid, accepted branch changes. Pending decisions and drift block export.
pub fn export(project: &store::Project, home: &Path, cwd: &Path) -> Result<Bundle> {
    let top = git::toplevel(cwd)?;
    let branch = git::branch(&top)?
        .ok_or_else(|| Error::new("detached", "cannot export a bundle from detached HEAD", "checkout the accepted branch"))?;
    let folder = project.root.strip_prefix(home).ok().and_then(Path::to_str).filter(|s| safe_project(s)).ok_or_else(|| {
        Error::new(
            "bundle-mode",
            format!("{} is not a home-store project", project.root.display()),
            "use the normal gate for a repository-mode store",
        )
    })?;
    let mut config = config::load(project, home)?;
    let (living, _) = change::load_living(project)?;
    let active = proof::active(project, &living)?;
    let queue = proof::on_branch(&active, &branch);
    if queue.is_empty() {
        return Err(Error::new("no-change", format!("{branch} has no accepted changes to export"), "accept a change on this branch first"));
    }
    let now = proof::Now { top: &top, config: &config, env: fingerprint::env(&top, &config)?, living: &living };
    let mut report = gate::Report::default();
    gate::check_lifecycle(project, &mut report)?;
    gate::check_record(project, &living, &mut report)?;
    let mut changes = Vec::new();
    for c in queue {
        if c.vow().is_none() || !c.moved_bases(&living).is_empty() {
            return Err(Error::new(
                "bundle-baseline",
                format!("{} has no valid accepted baseline", c.dir.id),
                "restore the accepted baseline or ask the user to accept its amendment",
            ));
        }
        gate::check_change(project, c, &now, &gate::Options { rerun: true, ..Default::default() }, &mut report)?;
        for t in &c.loaded.tasks {
            if proof::state(c, t, &now)? == proof::State::Blocked {
                return Err(Error::new(
                    "blocked",
                    format!("{} cannot be exported while blocked", t.id),
                    "ask the user to confirm an amendment that changes the task",
                ));
            }
        }
        let path = c.dir.path.join("tasks.md");
        changes.push(Change {
            id: c.dir.id.to_string(),
            tasks: std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?,
            requirements: c.loaded.tasks.iter().map(|t| (t.id.to_string(), proof::covered(t, &c.loaded, &living))).collect(),
        });
    }
    if !report.failures.is_empty() {
        return Err(Error::new(
            "bundle-invalid",
            report.failures.join("\n"),
            "resolve the reported plan or signature problem before exporting",
        ));
    }
    for (name, value) in &config.rerun_env {
        if !value.is_empty() && (runner::names_secret(name) || runner::mask(value, None, "__bundle__") != *value) {
            return Err(Error::new(
                "bundle-secret",
                format!("rerun_env.{name} would export a credential"),
                "provide the secret in the CI environment instead",
            ));
        }
    }
    for command in config.env.iter().map(String::as_str).chain(config.setup.iter().chain(&config.checks).map(|s| s.run())) {
        if runner::mask(command, None, "__bundle__") != command {
            return Err(Error::new(
                "bundle-secret",
                "a configured command contains an embedded credential",
                "use a CI environment variable instead",
            ));
        }
    }
    for c in &changes {
        for task in tasks::parse_tasks(&c.tasks).0 {
            if runner::mask(task.verify_command()?, None, "__bundle__") != task.verify_command()? {
                return Err(Error::new(
                    "bundle-secret",
                    format!("{} contains an embedded credential", task.id),
                    "use a CI environment variable instead",
                ));
            }
        }
    }
    config.context.clear();
    config.rules = config::Rules::default();
    config.allowed_signers.clear();
    Ok(Bundle { format: Format::V1, project: folder.to_string(), identity: project.meta.clone(), config, changes })
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
impl Scratch {
    fn new() -> Result<Self> {
        let nonce: String = Error::random::<16>()?.iter().map(|b| format!("{b:02x}")).collect();
        let path = std::env::temp_dir().join(format!("rvow-bundle-{nonce}"));
        std::fs::create_dir(&path).map_err(|e| Error::io(&path, e))?;
        std::fs::create_dir(path.join("objects")).map_err(|e| Error::io(&path, e))?;
        Ok(Self(path))
    }
}

/// Executes the exported checks in the checkout without creating an accepted local plan.
pub fn verify(bundle: &Bundle, cwd: &Path, echo: &mut dyn Write) -> Result<Verification> {
    let top = git::toplevel(cwd)?;
    let remote = git::origin_url(&top).and_then(|url| git::remote_path(&url));
    if !safe_project(&bundle.project)
        || git::is_shallow(&top)?
        || git::root_commits(&top)? != bundle.identity.roots
        || remote != bundle.identity.remote
    {
        return Err(Error::new(
            "bundle-project",
            format!("{} does not match the bundle's project identity", top.display()),
            "use the matching checkout with complete git history",
        ));
    }
    let scratch = Scratch::new()?;
    let capture = store::Project { root: scratch.0.clone(), meta: bundle.identity.clone() };
    let cfg = &bundle.config;
    let overrides: Vec<_> = cfg.rerun_env.clone().into_iter().collect();
    let mut result = Verification { runs: Vec::new(), failures: Vec::new() };
    let mut setup_failure = None;
    for step in &cfg.setup {
        let cmd = step.run();
        let outcome = runner::run(cmd, &top, &overrides, cfg.timeout(), echo)?;
        if !outcome.passed() {
            result.failures.push(format!("setup failed: {cmd}"));
            setup_failure = Some(outcome);
            break;
        }
    }
    let env = fingerprint::env_with(&top, cfg, &overrides)?;
    for c in &bundle.changes {
        let (tasks, diags) = tasks::parse_tasks(&c.tasks);
        if !diags.is_empty() || tasks.is_empty() {
            return Err(Error::new(
                "bundle-tasks",
                format!("{} contains invalid tasks", c.id),
                "export a new bundle from the accepted plan",
            ));
        }
        for task in tasks {
            let cmd = task.verify_command()?;
            let reqs = c.requirements.get(&task.id.to_string()).ok_or_else(|| {
                Error::new("bundle-task", format!("{} has no covered-requirement identity", task.id), "export a new bundle")
            })?;
            let globs: Vec<_> = task.files.iter().chain(&cfg.inputs).cloned().collect();
            let inputs = fingerprint::inputs(&top, &globs)?;
            let started = ledger::now_ms();
            let outcome = match &setup_failure {
                Some(outcome) => outcome.clone(),
                None => runner::run(cmd, &top, &overrides, task.timeout.unwrap_or(cfg.timeout()), echo)?,
            };
            let unchanged = inputs.hash == fingerprint::inputs(&top, &globs)?.hash;
            let passed = setup_failure.is_none() && outcome.passed() && outcome.tests_run != Some(0) && unchanged;
            if !passed {
                result.failures.push(format!("{} failed portable verification", task.id));
            }
            let thash = vow::task_hash(&task);
            let env_key = env.key();
            let key = vow::sha256_hex(json!([thash, reqs, inputs.hash, env_key]).to_string().as_bytes());
            let mut data = json!({
                "task":task.id.to_string(), "task_hash":thash, "requirements":reqs,
                "verify":cmd, "proved":passed, "exit":outcome.exit, "timed_out":outcome.timed_out,
                "tests_run":outcome.tests_run, "started":started, "duration_ms":outcome.duration_ms,
                "tail":outcome.tail, "inputs":inputs.hash, "input_files":inputs.files,
                "env":env, "env_key":env_key, "key":key, "project_identity":bundle.identity,
                "run":{"invocation":format!("bundle-{started}-{}",task.id), "rvow":env!("CARGO_PKG_VERSION"),
                    "machine":proof::machine(cfg),"agent":proof::agent(cfg),"head":git::run(&top,&["rev-parse","HEAD"])? ,
                    "branch":git::branch(&top)?,"ci":fingerprint::ci()}
            })
            .as_object()
            .cloned()
            .ok_or_else(|| Error::internal("run data is not an object"))?;
            if passed {
                let tests = guard::capture(&capture, &top, cfg, &task)?;
                let mut objects = Map::new();
                for hash in tests.values().filter_map(Value::as_str) {
                    let path = scratch.0.join("objects").join(hash);
                    let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
                    objects.insert(hash.into(), text.into());
                }
                data.insert("tests".into(), tests.into());
                data.insert("test_objects".into(), objects.into());
            }
            result.runs.push(ledger::Run { project: bundle.project.clone(), change: c.id.clone(), data });
        }
    }
    if setup_failure.is_none() {
        for step in &cfg.checks {
            let cmd = step.run();
            if !runner::run(cmd, &top, &overrides, cfg.timeout(), echo)?.passed() {
                result.failures.push(format!("check failed: {cmd}"));
            }
        }
    }
    Ok(result)
}
