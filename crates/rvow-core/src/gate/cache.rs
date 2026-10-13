//! The rerun cache: `rerun-cache.jsonl` records clean passes so the gate can
//! skip a task whose key has not changed. Each line chains to the one before
//! it and carries an HMAC under a key that agents cannot read.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{self, Config};
use crate::error::{Error, Result};
use crate::fingerprint::{self, Env};
use crate::git;
use crate::ledger::{append_line, now_ms, short_hash};
use crate::runner::Outcome;
use crate::seal;
use crate::store::{Lock, Project};
use crate::tasks::Task;
use crate::vow::{sha256_hex, task_hash};

pub const CACHE_FILE: &str = "rerun-cache.jsonl";
const KEY_FORMAT: u32 = 1;
const CHECK_KEY_FORMAT: u32 = 1;
const BAD_MAC: &str = "has an invalid MAC";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Body {
    unit: String,
    key: String,
    tests_run: Option<u64>,
    duration_ms: u64,
    rvow: String,
    machine: String,
    time: u64,
    prev: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    #[serde(flatten)]
    body: Body,
    mac: String,
}

/// A trusted entry that matches a unit, a key and the age limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub tests_run: Option<u64>,
    pub duration_ms: u64,
    pub time: u64,
}

pub struct Cache {
    path: PathBuf,
    home: PathBuf,
    entries: Vec<Body>,
    warnings: Vec<String>,
    last: String,
}

fn body_text(body: &Body) -> Result<String> {
    serde_json::to_string(body).map_err(|e| Error::internal(format!("cache entry does not serialize: {e}")))
}

impl Cache {
    /// Reads the cache and checks the chain and every MAC. A bad line is
    /// ignored with a warning; it never fails the open.
    pub fn open(project: &Project, home: &Path) -> Result<Cache> {
        let path = project.root.join(CACHE_FILE);
        let mut cache = Cache { path, home: home.to_path_buf(), entries: Vec::new(), warnings: Vec::new(), last: String::new() };
        let text = match std::fs::read_to_string(&cache.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(cache),
            Err(e) => return Err(Error::io(&cache.path, e)),
        };
        let key = seal::read_key(home);
        let mut broken = false;
        for (i, line) in text.lines().enumerate() {
            let entry = serde_json::from_str::<Entry>(line).ok();
            let verdict = match (&entry, &key) {
                _ if broken => Some("follows a broken link"),
                (None, _) => Some("is not a cache entry"),
                (Some(e), _) if e.body.prev != cache.last => Some("does not chain to the line before it"),
                (_, None) => Some("cannot be checked without the cache key"),
                (Some(e), Some(k)) => match body_text(&e.body).is_ok_and(|t| seal::verify(k, &t, &e.mac)) {
                    true => None,
                    false => Some(BAD_MAC),
                },
            };
            cache.last = short_hash(line.as_bytes());
            match verdict {
                None => cache.entries.extend(entry.map(|e| e.body)),
                Some(why) => {
                    broken |= why != BAD_MAC;
                    cache.warnings.push(format!("{} line {} {why}, so it was ignored", cache.path.display(), i + 1));
                }
            }
        }
        Ok(cache)
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The newest trusted entry for `unit` with this key, not older than `max_age_ms`.
    pub fn lookup(&self, unit: &str, key: &str, now_ms: u64, max_age_ms: u64) -> Option<Hit> {
        self.entries.iter().rev().find(|e| e.unit == unit && e.key == key && now_ms.saturating_sub(e.time) <= max_age_ms).map(|e| Hit {
            tests_run: e.tests_run,
            duration_ms: e.duration_ms,
            time: e.time,
        })
    }

    fn secret(&self) -> Result<Vec<u8>> {
        seal::key(&self.home)
    }

    /// Appends one entry. The caller holds the project lock. A cache file with
    /// a bad line is first moved aside, so a fresh chain starts clean.
    pub fn record(&mut self, _lock: &Lock, unit: &str, key: &str, tests_run: Option<u64>, duration_ms: u64, machine: &str) -> Result<()> {
        let secret = self.secret()?;
        if !self.warnings.is_empty() {
            let aside = self.path.with_extension(format!("jsonl.bad-{}", now_ms()));
            std::fs::rename(&self.path, &aside).map_err(|e| Error::io(&self.path, e))?;
            self.entries.clear();
            self.warnings.clear();
            self.last.clear();
        }
        let body = Body {
            unit: unit.into(),
            key: key.into(),
            tests_run,
            duration_ms,
            rvow: env!("CARGO_PKG_VERSION").into(),
            machine: machine.into(),
            time: now_ms(),
            prev: self.last.clone(),
        };
        let mac = seal::mac(&secret, &body_text(&body)?)?;
        let line = serde_json::to_string(&Entry { body: body.clone(), mac })
            .map_err(|e| Error::internal(format!("cache entry does not serialize: {e}")))?;
        append_line(&self.path, &line)?;
        self.last = short_hash(line.as_bytes());
        self.entries.push(body);
        Ok(())
    }

    /// Records a clean pass. A failure, a timeout or a run of 0 tests writes
    /// nothing and returns `false`.
    pub fn record_pass(&mut self, lock: &Lock, unit: &str, key: &str, outcome: &Outcome, machine: &str) -> Result<bool> {
        if !outcome.passed() || outcome.tests_run == Some(0) {
            return Ok(false);
        }
        self.record(lock, unit, key, outcome.tests_run, outcome.duration_ms, machine)?;
        Ok(true)
    }
}

/// The resolved path and SHA-256 of an executable, found by path or on `PATH`,
/// through symlinks.
pub(crate) fn tool_id(name: &str) -> Option<(String, String)> {
    let candidates: Vec<PathBuf> = if name.contains('/') {
        vec![PathBuf::from(name)]
    } else {
        std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(name)).collect()
    };
    candidates.iter().find_map(|p| {
        let real = p.canonicalize().ok()?;
        let meta = std::fs::metadata(&real).ok()?;
        #[cfg(unix)]
        if std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o111 == 0 {
            return None;
        }
        meta.is_file().then(|| std::fs::read(&real).ok()).flatten().map(|b| (real.to_string_lossy().into_owned(), sha256_hex(&b)))
    })
}

/// The SHA-256 of an executable, found by path or on `PATH`, through symlinks.
fn tool_hash(name: &str) -> Option<String> {
    tool_id(name).map(|(_, hash)| hash)
}

/// The `tools` map shared by the task and check cache keys. `None` when a
/// listed tool is missing, so the caller must not cache.
pub fn tool_fingerprint(project: &Project, home: &Path) -> Result<Option<BTreeMap<String, String>>> {
    let mut tools = BTreeMap::new();
    for name in config::tools(project, home)? {
        let Some(hash) = tool_hash(&name) else { return Ok(None) };
        tools.insert(name, hash);
    }
    Ok(Some(tools))
}

/// The key of a check result: the command text, the HEAD tree id and the tool
/// fingerprint. `None` when a listed tool is missing.
pub fn check_key(top: &Path, cmd: &str, tools: Option<&BTreeMap<String, String>>) -> Result<Option<String>> {
    let Some(tools) = tools else { return Ok(None) };
    let tree = git::run(top, &["rev-parse", "HEAD^{tree}"])?;
    let value = serde_json::json!([CHECK_KEY_FORMAT, env!("CARGO_PKG_VERSION"), cmd, tree.trim(), tools]);
    Ok(Some(sha256_hex(value.to_string().as_bytes())))
}

/// The cache key of a task, or `None` when the task must not be cached: an
/// `env` command failed or a listed tool is missing. The tree comes from HEAD,
/// which the gate's clean worktree matches, so no file is read.
pub fn cache_key(
    project: &Project,
    home: &Path,
    top: &Path,
    config: &Config,
    task: &Task,
    requirements: &BTreeMap<String, String>,
    env: &Env,
) -> Result<Option<String>> {
    if env.commands.values().any(|v| v.starts_with("failed: ")) {
        return Ok(None);
    }
    let Some(tools) = tool_fingerprint(project, home)? else { return Ok(None) };
    let listing = git::run(top, &["ls-tree", "-r", "-z", "HEAD"])?;
    let tree: Vec<(&str, &str, &str)> = listing
        .split('\0')
        .filter_map(|e| {
            let (meta, path) = e.split_once('\t')?;
            let mut parts = meta.split(' ');
            Some((parts.next()?, parts.nth(1)?, path))
        })
        .collect();
    for glob in &task.files {
        let one = fingerprint::globset(std::slice::from_ref(glob))?;
        if !tree.iter().any(|(_, _, path)| one.is_match(path)) {
            return Err(Error::new(
                "cache-scope",
                format!("{}: the files glob {glob:?} matches no file at HEAD", task.id),
                format!("fix the files: line of {} in tasks.md, or commit the files it names", task.id),
            ));
        }
    }
    let mut globs = task.files.clone();
    globs.extend(config.inputs.iter().cloned());
    let scope = fingerprint::globset(&globs)?;
    let files: Vec<&(&str, &str, &str)> = tree.iter().filter(|(_, _, path)| scope.is_match(path)).collect();
    let value = serde_json::json!([
        KEY_FORMAT,
        env!("CARGO_PKG_VERSION"),
        task_hash(task),
        requirements,
        files,
        env.os,
        env.arch,
        env.commands,
        env.vars,
        config.rerun_env,
        tools,
    ]);
    Ok(Some(sha256_hex(value.to_string().as_bytes())))
}

/// The final key of every task: its own key, plus the final key of each task
/// it `depends_on`. A task whose own key, a dependency, or the chain is not
/// usable (unknown id, cycle, uncacheable) gets `None`.
pub fn dependent_keys(tasks: &[Task], own: &BTreeMap<String, Option<String>>) -> BTreeMap<String, Option<String>> {
    fn visit(
        id: &str,
        tasks: &[Task],
        own: &BTreeMap<String, Option<String>>,
        done: &mut BTreeMap<String, Option<String>>,
        stack: &mut Vec<String>,
    ) -> Option<String> {
        if let Some(key) = done.get(id) {
            return key.clone();
        }
        if stack.iter().any(|s| s == id) {
            return None;
        }
        let task = tasks.iter().find(|t| t.id.to_string() == id)?;
        let base = own.get(id).cloned().flatten();
        stack.push(id.to_string());
        let deps: Vec<Option<String>> = task.depends_on.iter().map(|d| visit(&d.to_string(), tasks, own, done, stack)).collect();
        stack.pop();
        let key = base.and_then(|base| {
            if deps.is_empty() {
                return Some(base);
            }
            let deps: Option<Vec<String>> = deps.into_iter().collect();
            Some(sha256_hex(serde_json::json!([base, deps?]).to_string().as_bytes()))
        });
        done.insert(id.to_string(), key.clone());
        key
    }
    let mut done = BTreeMap::new();
    for t in tasks {
        let id = t.id.to_string();
        visit(&id, tasks, own, &mut done, &mut Vec::new());
        done.entry(id).or_insert(None);
    }
    done
}
