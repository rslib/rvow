//! The rerun cache: `rerun-cache.jsonl` records clean passes so the gate can
//! skip a task whose key has not changed. Each line chains to the one before
//! it and carries an HMAC under a key that agents cannot read.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::config::{self, Config};
use crate::error::{Error, Result};
use crate::fingerprint::{self, Env};
use crate::git;
use crate::ledger::{append_line, now_ms, short_hash};
use crate::runner::Outcome;
use crate::store::{Lock, Project};
use crate::tasks::Task;
use crate::vow::{sha256_hex, task_hash};

pub const CACHE_FILE: &str = "rerun-cache.jsonl";
const KEY_FOLDER: &str = "hidden-checks";
const KEY_FILE: &str = ".cache-key";
const KEY_FORMAT: u32 = 1;
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
    key_path: PathBuf,
    entries: Vec<Body>,
    warnings: Vec<String>,
    last: String,
}

fn mac_of(key: &[u8], body: &Body) -> Result<Hmac<Sha256>> {
    let text = serde_json::to_string(body).map_err(|e| Error::internal(format!("cache entry does not serialize: {e}")))?;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|e| Error::internal(format!("bad cache key: {e}")))?;
    mac.update(text.as_bytes());
    Ok(mac)
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

impl Cache {
    /// Reads the cache and checks the chain and every MAC. A bad line is
    /// ignored with a warning; it never fails the open.
    pub fn open(project: &Project, home: &Path) -> Result<Cache> {
        let path = project.root.join(CACHE_FILE);
        let key_path = home.join(KEY_FOLDER).join(KEY_FILE);
        let mut cache = Cache { path, key_path, entries: Vec::new(), warnings: Vec::new(), last: String::new() };
        let text = match std::fs::read_to_string(&cache.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(cache),
            Err(e) => return Err(Error::io(&cache.path, e)),
        };
        let key = std::fs::read(&cache.key_path).ok().filter(|k| k.len() == 32);
        let mut broken = false;
        for (i, line) in text.lines().enumerate() {
            let entry = serde_json::from_str::<Entry>(line).ok();
            let verdict = match (&entry, &key) {
                _ if broken => Some("follows a broken link"),
                (None, _) => Some("is not a cache entry"),
                (Some(e), _) if e.body.prev != cache.last => Some("does not chain to the line before it"),
                (_, None) => Some("cannot be checked without the cache key"),
                (Some(e), Some(k)) => match hex_decode(&e.mac).map(|m| mac_of(k, &e.body).map(|mac| mac.verify_slice(&m).is_ok())) {
                    Some(Ok(true)) => None,
                    _ => Some(BAD_MAC),
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
        if let Ok(k) = std::fs::read(&self.key_path)
            && k.len() == 32
        {
            return Ok(k);
        }
        if let Some(dir) = self.key_path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        }
        let secret = Error::random::<32>()?;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut file = opts.open(&self.key_path).map_err(|e| Error::io(&self.key_path, e))?;
        file.write_all(&secret).map_err(|e| Error::io(&self.key_path, e))?;
        Ok(secret.to_vec())
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
        let mac: String = mac_of(&secret, &body)?.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect();
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

/// The SHA-256 of an executable, found by path or on `PATH`, through symlinks.
fn tool_hash(name: &str) -> Option<String> {
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
        meta.is_file().then(|| std::fs::read(&real).ok()).flatten().map(|b| sha256_hex(&b))
    })
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
    let mut tools = BTreeMap::new();
    for name in config::tools(project, home)? {
        let Some(hash) = tool_hash(&name) else { return Ok(None) };
        tools.insert(name, hash);
    }
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
