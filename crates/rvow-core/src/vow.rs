//! `rvow accept`: validate a backlog change, run the baseline, mint real ids,
//! store the vowed content and move the change to `active/`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::change::{self, ChangeDir, LEDGER, Loaded, Stage};
use crate::config::{self, Config, ENFORCEMENT};
use crate::diag::Severity;
use crate::error::{Error, Result};
use crate::fingerprint;
use crate::git;
use crate::id::{Id, Ref};
use crate::ledger::{self, Kind};
use crate::lint::{Finding, clarifications};
use crate::proof;
use crate::registry;
use crate::runner::{self, Outcome};
use crate::spec::{DeltaOp, Requirement, Spec, StepKind};
use crate::store::{Lock, Project};
use crate::tasks::Task;

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Stores `bytes` in `objects/` under their SHA-256 and returns the hash.
pub fn put_object(project: &Project, bytes: &[u8]) -> Result<String> {
    let hash = sha256_hex(bytes);
    let path = project.root.join("objects").join(&hash);
    if !path.exists() {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes).map_err(|e| Error::io(&tmp, e))?;
        std::fs::rename(&tmp, &path).map_err(|e| Error::io(&path, e))?;
    }
    Ok(hash)
}

pub fn task_hash(t: &Task) -> String {
    let covers: Vec<String> = t.covers.iter().map(Ref::to_string).collect();
    let mut v = json!({
        "title": t.title,
        "verify": t.verify,
        "covers": covers,
        "files": t.files,
        "baseline_pass": t.baseline_pass,
        "timeout_secs": t.timeout.map(|d| d.as_secs()),
    });
    if let Some(command) = &t.mutants {
        v["mutants"] = command.clone().into();
    }
    sha256_hex(v.to_string().as_bytes())
}

pub fn requirement_hash(r: &Requirement) -> String {
    let scenarios: Vec<Value> = r
        .scenarios
        .iter()
        .map(|s| {
            let steps: Vec<Value> = s
                .steps
                .iter()
                .map(|st| {
                    let k = match st.kind {
                        StepKind::Given => "GIVEN",
                        StepKind::When => "WHEN",
                        StepKind::Then => "THEN",
                        StepKind::And => "AND",
                    };
                    json!([k, st.text])
                })
                .collect();
            json!({"title": s.title, "steps": steps})
        })
        .collect();
    let v = json!({
        "title": r.title,
        "text": r.text,
        "deprecated": r.deprecated,
        "scenarios": scenarios,
    });
    sha256_hex(v.to_string().as_bytes())
}

/// Replaces whole-token occurrences of `from`, so `REQ-new1` does not touch
/// `REQ-new12`.
pub(crate) fn replace_token(text: &str, from: &str, to: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(from) {
        let before = rest[..i].chars().last().or(out.chars().last());
        let before_ok = !before.is_some_and(|c| c.is_ascii_alphanumeric());
        let after = rest[i + from.len()..].chars().next();
        let after_ok = !after.is_some_and(|c| c.is_ascii_alphanumeric());
        out.push_str(&rest[..i]);
        out.push_str(if before_ok && after_ok { to } else { from });
        rest = &rest[i + from.len()..];
    }
    out.push_str(rest);
    out
}

#[derive(Debug, Clone)]
pub struct Baseline {
    pub task: Ref,
    pub outcome: Outcome,
}

/// A change that passed validation and its baseline run, ready to vow.
#[derive(Debug)]
pub struct Prepared {
    pub change: ChangeDir,
    pub loaded: Loaded,
    pub living: Vec<(String, Spec)>,
    pub top: PathBuf,
    pub branch: String,
    pub base_commit: String,
    pub config: Config,
    pub baseline: Vec<Baseline>,
    /// The failed test names each task's JUnit report named, by task id.
    pub junit: BTreeMap<String, Vec<String>>,
}

fn findings_error(code: &'static str, what: &str, items: &[String], fix: &str) -> Error {
    Error::new(code, format!("{what}:\n  {}", items.join("\n  ")), fix)
}

fn format_finding((path, d): &Finding) -> String {
    format!("{path}:{}: {}: {}", d.line, d.rule.code(), d.message)
}

fn base_commit(top: &Path, base: &str) -> Result<String> {
    match git::base_ref(top, base) {
        Some(r) => git::run(top, &["merge-base", "HEAD", &r]),
        None => Err(Error::new(
            "base",
            format!("neither origin/{base} nor {base} exists"),
            "set base in config.toml to the branch this work merges into",
        )),
    }
}

/// Validates a backlog change and runs every verify command once. Fails when a
/// check would pass before any work starts.
pub fn prepare(project: &Project, home: &Path, change: ChangeDir, cwd: &Path, echo: &mut dyn Write) -> Result<Prepared> {
    if change.stage != Stage::Backlog {
        return Err(Error::new(
            "stage",
            format!("{} is in {}, not in backlog", change.id, change.stage.name()),
            "only a backlog change can be accepted; run rvow ls to find one",
        ));
    }
    let top = git::toplevel(cwd)?;
    let (living, living_findings) = change::load_living(project)?;
    let loaded = change::load(&change.path, &living)?;
    let branch = git::branch(&top)?.ok_or_else(|| Error::new("detached", "HEAD is detached", "check out the change's branch"))?;
    if branch != loaded.proposal.branch {
        return Err(Error::new(
            "branch",
            format!("the change belongs to {:?} but HEAD is on {branch:?}", loaded.proposal.branch),
            format!("run git checkout {}", loaded.proposal.branch),
        ));
    }

    let touched_caps: HashSet<String> = {
        let living_view = crate::lint::Living { specs: &living };
        let index: HashMap<&Id, &str> = living_view.index();
        let mut caps: HashSet<String> = loaded.deltas.iter().map(|(c, _)| c.clone()).collect();
        let refs = loaded
            .deltas
            .iter()
            .flat_map(|(_, f)| f.deltas.iter().map(|d| &d.req.id))
            .chain(loaded.tasks.iter().flat_map(|t| t.covers.iter()));
        for r in refs {
            if let Ref::Id(id) = r
                && let Some(cap) = index.get(id)
            {
                caps.insert(cap.to_string());
            }
        }
        caps
    };
    let mut errors: Vec<String> = loaded.errors().map(format_finding).collect();
    errors.extend(
        living_findings
            .iter()
            .filter(|(p, d)| d.severity() == Severity::Error && touched_caps.iter().any(|c| *p == format!("specs/{c}/spec.md")))
            .map(format_finding),
    );
    if !errors.is_empty() {
        return Err(findings_error("invalid", "the change does not validate", &errors, "fix each finding, then run rvow accept again"));
    }
    if loaded.tasks.is_empty() {
        return Err(Error::new("no-tasks", "tasks.md has no task", "add tasks as `- TSK-new1 <title>` with verify: and files:"));
    }
    let open: Vec<String> = clarifications(&loaded.texts).into_iter().map(|(p, l)| format!("{p}:{l}")).collect();
    if !open.is_empty() {
        return Err(findings_error(
            "clarification",
            "open questions remain",
            &open,
            "answer each [NEEDS CLARIFICATION] and remove the marker",
        ));
    }

    let runners = config::runners(project)?;
    let missing: Vec<String> =
        crate::plan::missing_programs_filled(&top, &loaded.tasks, &crate::plan::filled_commands(&runners, &loaded.tasks))
            .into_iter()
            .map(|(task, prog)| format!("{task} runs {prog}, which is neither on PATH nor in the repository"))
            .collect();
    if !missing.is_empty() {
        return Err(findings_error(
            "missing-program",
            "some verify commands cannot run",
            &missing,
            "fix the command; a check that cannot run fails for the wrong reason",
        ));
    }
    let config = config::load(project, home)?;
    let base_commit = base_commit(&top, &config.base)?;
    let mut baseline = Vec::new();
    let mut junit: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut refused = Vec::new();
    for task in &loaded.tasks {
        let plan = crate::template::plan(&runners, task)?;
        if let Some(path) = plan.missing_need(&top) {
            return Err(Error::new(
                "needs",
                format!("{} needs {}, which does not exist", task.id, path),
                format!("configure {} first (for example with `cmake --preset ...`), then run rvow accept again", path),
            ));
        }
        let cmd = plan.command.as_str();
        let _ = writeln!(echo, "baseline {}: {cmd}", task.id);
        let timeout = task.timeout.or(plan.timeout).unwrap_or(config.timeout());
        let outcome = runner::run(cmd, &top, &plan.env, timeout, echo)?;
        let ran_tests = outcome.tests_run != Some(0);
        if outcome.passed() && ran_tests && !task.baseline_pass {
            refused.push(format!(
                "{} passes before any work ({} tests run): {cmd}",
                task.id,
                outcome.tests_run.map_or("unknown".into(), |n| n.to_string())
            ));
        }
        if task.baseline_pass && !outcome.passed() {
            refused.push(format!("{} says baseline: pass but fails now: {cmd}", task.id));
        }
        // A runner that names {junit} writes a report: the failed test names
        // it holds are the task's proof target, so it must name at least one.
        if let Some(report) = &plan.junit {
            let failed = match crate::junit::read(report) {
                Ok(cases) => cases.iter().filter(|c| c.failed).map(|c| c.name.clone()).collect::<Vec<_>>(),
                Err(e) => {
                    refused.push(format!("{}: {}", task.id, e.message));
                    Vec::new()
                }
            };
            if failed.is_empty() {
                refused.push(format!("{}: its JUnit report names no failed test, so no test is the proof target", task.id));
            }
            junit.insert(task.id.to_string(), failed);
        }
        baseline.push(Baseline { task: task.id.clone(), outcome });
    }
    if !refused.is_empty() {
        return Err(findings_error(
            "baseline",
            "the baseline run refused some tasks",
            &refused,
            "make each check fail before the work, or mark a check that must stay green with baseline: pass",
        ));
    }
    Ok(Prepared { change, loaded, living, top, branch, base_commit, config, baseline, junit })
}

/// The result of a vow.
#[derive(Debug)]
pub struct Vowed {
    pub id: Id,
    pub path: PathBuf,
    /// Placeholder to real id, in mint order.
    pub minted: Vec<(String, Id)>,
    pub event: String,
}

pub fn protected_files(top: &Path, config: &Config) -> Result<BTreeMap<String, PathBuf>> {
    let patterns: Vec<String> =
        config.test_globs.iter().chain(&config.protect_infra).cloned().chain(ENFORCEMENT.iter().map(|s| s.to_string())).collect();
    let set = fingerprint::globset(&patterns)?;
    let mut files: BTreeMap<String, PathBuf> =
        fingerprint::candidate_files(top)?.into_iter().filter(|f| set.is_match(f)).map(|f| (f.clone(), top.join(f))).collect();
    if !config.allowed_signers.is_empty() {
        let given = Path::new(&config.allowed_signers);
        let file = if given.is_absolute() { given.to_path_buf() } else { top.join(given) };
        if file.is_file() {
            files.insert(format!("allowed_signers:{}", file.display()), file);
        }
    }
    Ok(files)
}

/// Mints a real id for every placeholder of a change, rewrites each Markdown
/// file of the change and returns the mapping in mint order.
pub fn mint_placeholders(project: &Project, lock: &Lock, change: &ChangeDir, loaded: &Loaded) -> Result<Vec<(String, Id)>> {
    let mut placeholders: Vec<Ref> = Vec::new();
    let mut seen = HashSet::new();
    let all_refs = loaded
        .deltas
        .iter()
        .flat_map(|(_, f)| f.deltas.iter().map(|d| &d.req.id))
        .chain(loaded.tasks.iter().flat_map(|t| std::iter::once(&t.id).chain(&t.covers).chain(&t.depends_on)));
    for r in all_refs {
        if matches!(r, Ref::Placeholder { .. }) && seen.insert(r.clone()) {
            placeholders.push(r.clone());
        }
    }
    let mut minted = Vec::new();
    for p in &placeholders {
        let id = registry::mint(project, lock, p.kind(), Some(&change.id))?;
        minted.push((p.to_string(), id));
    }
    for (rel, text) in &loaded.texts {
        let mut new = text.clone();
        for (from, to) in &minted {
            new = replace_token(&new, from, &to.to_string());
        }
        if &new != text {
            let path = change.path.join(rel);
            std::fs::write(&path, new).map_err(|e| Error::io(&path, e))?;
        }
    }
    Ok(minted)
}

/// `rvow ids`: mints ids for the placeholders of `target`, or of every active
/// change on the current branch, and returns the mapping in mint order.
pub fn mint_active(project: &Project, cwd: &Path, target: Option<&str>) -> Result<Vec<(String, Id)>> {
    let lock = project.lock()?;
    let (living, _) = change::load_living(project)?;
    let dirs = match target {
        Some(t) => {
            let c = change::resolve(project, t)?;
            if c.stage != Stage::Active {
                let fix = if c.stage == Stage::Backlog {
                    "placeholders stay valid in backlog; accept mints them"
                } else {
                    "only an active change has placeholders to mint"
                };
                return Err(Error::new("stage", format!("{} is in {}, not in active", c.id, c.stage.name()), fix));
            }
            vec![c]
        }
        None => {
            let branch = git::branch(cwd)?;
            proof::active(project, &living)?.into_iter().filter(|a| a.branch() == branch.as_deref()).map(|a| a.dir).collect()
        }
    };
    let mut out = Vec::new();
    for dir in dirs {
        let loaded = change::load(&dir.path, &living)?;
        out.extend(mint_placeholders(project, &lock, &dir, &loaded)?);
    }
    Ok(out)
}

/// Stores the content of a change and returns the snapshot that a vow or an
/// amendment records: branch, effective config, every Markdown file, the hash
/// of each task and delta requirement, and the protected files.
pub fn snapshot(project: &Project, top: &Path, config: &Config, loaded: &Loaded, branch: &str) -> Result<Map<String, Value>> {
    let mut files = Map::new();
    for (rel, text) in &loaded.texts {
        files.insert(rel.clone(), put_object(project, text.as_bytes())?.into());
    }
    let mut protected = Map::new();
    for (rel, path) in protected_files(top, config)? {
        let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        protected.insert(rel, put_object(project, &bytes)?.into());
    }
    let tasks: Map<String, Value> = loaded.tasks.iter().map(|t| (t.id.to_string(), task_hash(t).into())).collect();
    let requirements: Map<String, Value> = loaded
        .deltas
        .iter()
        .flat_map(|(_, f)| f.deltas.iter())
        .filter(|d| d.op.has_body())
        .map(|d| (d.req.id.to_string(), requirement_hash(&d.req).into()))
        .collect();
    let mut data = Map::new();
    data.insert("branch".into(), branch.into());
    data.insert("config".into(), put_object(project, config.canonical()?.as_bytes())?.into());
    data.insert("config_keys".into(), json!(config.user_keys));
    data.insert("files".into(), files.into());
    data.insert("tasks".into(), tasks.into());
    data.insert("requirements".into(), requirements.into());
    data.insert("protected".into(), put_object(project, Value::Object(protected).to_string().as_bytes())?.into());
    data.insert("runners".into(), runner_record(project, loaded)?);
    Ok(data)
}

/// The hash of each runner a task names, keyed by task id and runner name,
/// for runner drift. Only a runner some task uses is recorded, so a runner
/// that no task names can change with no drift.
pub fn runner_record(project: &Project, loaded: &Loaded) -> Result<Value> {
    let runners = config::runners(project)?;
    let mut out = Map::new();
    for task in &loaded.tasks {
        let Some((name, _)) = crate::template::reference(task.verify.as_deref().unwrap_or("")) else { continue };
        if let Some(runner) = runners.get(name) {
            out.insert(task.id.to_string(), json!({ name: runner.hash() }));
        }
    }
    Ok(Value::Object(out))
}

/// The living hash of each requirement that a CHANGE, DEPRECATE, MOVE or
/// REMOVE names: the base that `archive` expects to find.
pub fn bases(loaded: &Loaded, living: &[(String, Spec)]) -> Map<String, Value> {
    let living_hash: HashMap<String, String> =
        living.iter().flat_map(|(_, s)| s.requirements.iter()).map(|r| (r.id.to_string(), requirement_hash(r))).collect();
    let mut out = Map::new();
    for (_, file) in &loaded.deltas {
        for d in file.deltas.iter().filter(|d| d.op != DeltaOp::Add) {
            let id = d.req.id.to_string();
            if let Some(h) = living_hash.get(&id) {
                out.insert(id, h.clone().into());
            }
        }
    }
    out
}

/// Mints real ids, stores the vowed content and records the vow.
pub fn commit(project: &Project, prepared: Prepared) -> Result<Vowed> {
    let Prepared { change, loaded, living, top, branch, base_commit, config, baseline, junit } = prepared;
    let lock = project.lock()?;

    let minted = mint_placeholders(project, &lock, &change, &loaded)?;
    let loaded = change::load(&change.path, &living)?;
    if let Some(f) = loaded.errors().next() {
        return Err(Error::new("invalid", format!("after minting ids: {}", format_finding(f)), "report this as an rvow bug"));
    }

    let mut data = snapshot(project, &top, &config, &loaded, &branch)?;
    let minted_names: HashMap<String, String> = minted.iter().map(|(from, to)| (from.clone(), to.to_string())).collect();
    let baseline: Map<String, Value> = baseline
        .into_iter()
        .map(|b| {
            let key = b.task.to_string();
            let key = minted_names.get(&key).cloned().unwrap_or(key);
            let v = json!({"exit": b.outcome.exit, "tests_run": b.outcome.tests_run, "timed_out": b.outcome.timed_out});
            (key, v)
        })
        .collect();

    data.insert("change".into(), change.id.to_string().into());
    data.insert("base_commit".into(), base_commit.into());
    data.insert("bases".into(), bases(&loaded, &living).into());
    data.insert("baseline".into(), baseline.into());
    let junit: Map<String, Value> = junit
        .into_iter()
        .map(|(task, names)| {
            let key = minted_names.get(&task).cloned().unwrap_or(task);
            (key, Value::Array(names.into_iter().map(Value::String).collect()))
        })
        .collect();
    data.insert("junit".into(), junit.into());
    let event = ledger::append(&change.path.join(LEDGER), &lock, Kind::Vow, data)?;

    let dest = project.root.join(Stage::Active.dir()).join(change::file_name(&change.path)?);
    std::fs::rename(&change.path, &dest).map_err(|e| Error::io(&dest, e))?;
    Ok(Vowed { id: change.id, path: dest, minted, event: event.hash })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;
    use crate::store::{Mode, init};

    #[test]
    fn token_replace_is_whole_word() {
        assert_eq!(
            replace_token("REQ-new1 REQ-new12 xREQ-new1 (REQ-new1)", "REQ-new1", "REQ-abc"),
            "REQ-abc REQ-new12 xREQ-new1 (REQ-abc)"
        );
    }

    fn setup(tasks: &str) -> (TempDir, Project, PathBuf, ChangeDir) {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "r");
        std::fs::create_dir_all(repo.join("tests")).unwrap();
        std::fs::write(repo.join("tests/t.sh"), "exit 0").unwrap();
        let p = init(&repo, &home, Mode::Home).unwrap();
        let c = change::new(&p, "Output tail", "main", false).unwrap();
        std::fs::write(c.path.join("tasks.md"), tasks).unwrap();
        std::fs::write(
            c.path.join("specs/ledger.md"),
            "## ADD REQ-new1 Tail\nrvow SHALL keep the tail.\n### Scenario: s\n- WHEN a run ends\n- THEN the tail is kept\n",
        )
        .unwrap();
        (tmp, p, repo, c)
    }

    #[test]
    fn accept_mints_ids_records_the_vow_and_moves_the_change() {
        let (tmp, p, repo, c) =
            setup("- TSK-new1 Tail\n  covers: REQ-new1\n  verify: echo 'test result: ok. 0 passed; 0 failed;'\n  files: src/**\n");
        let prepared = prepare(&p, &tmp.0.join("home"), c, &repo, &mut std::io::sink()).unwrap();
        assert_eq!(prepared.baseline[0].outcome.tests_run, Some(0));
        let vowed = commit(&p, prepared).unwrap();
        assert_eq!(vowed.minted.len(), 2);
        assert!(vowed.path.starts_with(p.root.join("active")));
        let tasks = std::fs::read_to_string(vowed.path.join("tasks.md")).unwrap();
        assert!(!tasks.contains("-new1"), "{tasks}");
        let (events, diags) = ledger::read(&vowed.path.join(LEDGER)).unwrap();
        assert!(diags.is_empty());
        assert_eq!(events.len(), 1);
        let protected = &events[0].data["protected"];
        let obj = std::fs::read_to_string(p.root.join("objects").join(protected.as_str().unwrap())).unwrap();
        assert!(obj.contains("tests/t.sh"), "{obj}");
        assert_eq!(registry::load(&p).unwrap().len(), 3);
    }

    #[test]
    fn baseline_refuses_a_check_that_already_passes() {
        let (tmp, p, repo, c) =
            setup("- TSK-new1 Tail\n  covers: REQ-new1\n  verify: echo 'test result: ok. 2 passed; 0 failed;'\n  files: src/**\n");
        let err = prepare(&p, &tmp.0.join("home"), c, &repo, &mut std::io::sink()).unwrap_err();
        assert_eq!(err.code, "baseline");
    }

    #[test]
    fn baseline_pass_must_hold_and_branch_must_match() {
        let (tmp, p, repo, c) = setup("- TSK-new1 Tail\n  covers: REQ-new1\n  verify: false\n  files: src/**\n  baseline: pass\n");
        let home = tmp.0.join("home");
        let err = prepare(&p, &home, c.clone(), &repo, &mut std::io::sink()).unwrap_err();
        assert_eq!(err.code, "baseline");
        git(&repo, &["checkout", "-q", "-b", "other"]);
        let err = prepare(&p, &home, c, &repo, &mut std::io::sink()).unwrap_err();
        assert_eq!(err.code, "branch");
    }

    #[test]
    fn invalid_and_unanswered_changes_are_refused() {
        let (tmp, p, repo, c) = setup("- TSK-new1 No verify\n  files: src/**\n");
        let home = tmp.0.join("home");
        let err = prepare(&p, &home, c.clone(), &repo, &mut std::io::sink()).unwrap_err();
        assert_eq!(err.code, "invalid");
        std::fs::write(c.path.join("tasks.md"), "- TSK-new1 T\n  covers: REQ-new1\n  verify: false\n  files: src/**\n").unwrap();
        let prop = std::fs::read_to_string(c.path.join("proposal.md")).unwrap();
        std::fs::write(c.path.join("proposal.md"), prop + "[NEEDS CLARIFICATION: which?]\n").unwrap();
        let err = prepare(&p, &home, c, &repo, &mut std::io::sink()).unwrap_err();
        assert_eq!(err.code, "clarification");
    }

    #[test]
    fn ids_mints_for_active_changes_on_the_branch() {
        let (_tmp, p, repo, _) = crate::proof::tests::accepted();
        let dir = std::fs::read_dir(p.root.join("active")).unwrap().next().unwrap().unwrap().path();
        let tasks = dir.join("tasks.md");
        let mut text = std::fs::read_to_string(&tasks).unwrap();
        text.push_str("- TSK-new1 More\n  verify: true\n  files: a\n");
        std::fs::write(&tasks, text).unwrap();

        let out = mint_active(&p, &repo, None).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "TSK-new1");
        let text = std::fs::read_to_string(&tasks).unwrap();
        assert!(text.contains(&out[0].1.to_string()) && !text.contains("TSK-new1"));
        assert!(mint_active(&p, &repo, None).unwrap().is_empty());

        git(&repo, &["checkout", "-q", "-b", "other"]);
        let backlog = change::new(&p, "Later", "other", true).unwrap();
        let err = mint_active(&p, &repo, Some(&backlog.id.to_string())).unwrap_err();
        assert_eq!(err.code, "stage");
        assert!(err.fix.contains("accept mints them"));
    }
}
