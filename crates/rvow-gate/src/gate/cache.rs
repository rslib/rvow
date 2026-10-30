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

pub use rvow_base::layout::CACHE_FILE;

const KEY_FORMAT: u32 = 2;
const CHECK_KEY_FORMAT: u32 = 1;
const BAD_MAC: &str = "has an invalid MAC";
/// A cache file past this size is compacted to the newest entry per key, so a
/// project that runs for months reads a bounded file.
const CACHE_BYTES_LIMIT: u64 = 128 * 1024;
/// The bytes a line adds to its body text: the MAC and the JSON around it.
const LINE_OVERHEAD: u64 = 96;

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
    /// The newest position in `entries` for each `(unit, key)`, so a lookup
    /// is one probe instead of a walk back over every entry.
    index: std::collections::HashMap<(String, String), usize>,
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
        let mut cache = Cache {
            path,
            home: home.to_path_buf(),
            entries: Vec::new(),
            index: std::collections::HashMap::new(),
            warnings: Vec::new(),
            last: String::new(),
        };
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
                None => {
                    if let Some(e) = entry {
                        cache.remember(e.body);
                    }
                }
                Some(why) => {
                    broken |= why != BAD_MAC;
                    cache.warnings.push(format!("{} line {} {why}, so it was ignored", cache.path.display(), i + 1));
                }
            }
        }
        Ok(cache)
    }

    /// Whether the file has grown past the size that compacts it.
    fn over_limit(&self) -> bool {
        std::fs::metadata(&self.path).is_ok_and(|m| m.len() > CACHE_BYTES_LIMIT)
    }

    /// Rewrites the file with the newest entry per `(unit, key)`, the most
    /// recent first, up to half of `CACHE_BYTES_LIMIT`, so the next records do
    /// not compact again at once. Keys change with each edit and commit, so the
    /// oldest ones go; a dropped entry only reruns its task. The kept entries
    /// chain again and are signed under the same key, so each one still verifies.
    fn compact(&mut self, secret: &[u8]) -> Result<()> {
        let mut newest: Vec<usize> = self.index.values().copied().collect();
        newest.sort_unstable_by_key(|&at| std::cmp::Reverse((self.entries.get(at).map_or(0, |e| e.time), at)));
        let mut kept = Vec::new();
        let mut size = 0u64;
        for at in newest {
            let Some(entry) = self.entries.get(at) else { continue };
            size += body_text(entry)?.len() as u64 + LINE_OVERHEAD;
            if size > CACHE_BYTES_LIMIT / 2 {
                break;
            }
            kept.push(at);
        }
        kept.sort_unstable();
        let mut text = String::new();
        let mut entries = Vec::with_capacity(kept.len());
        let mut index = std::collections::HashMap::with_capacity(kept.len());
        let mut last = String::new();
        for at in kept {
            let Some(entry) = self.entries.get(at) else { continue };
            let mut body = entry.clone();
            body.prev = last;
            let mac = seal::mac(secret, &body_text(&body)?)?;
            let line = serde_json::to_string(&Entry { body: body.clone(), mac })
                .map_err(|e| Error::internal(format!("cache entry does not serialize: {e}")))?;
            last = short_hash(line.as_bytes());
            index.insert((body.unit.clone(), body.key.clone()), entries.len());
            entries.push(body);
            text.push_str(&line);
            text.push('\n');
        }
        // A reader that does not hold the lock sees the old file or the new one,
        // never a torn line.
        let tmp = self.path.with_extension(format!("jsonl.tmp-{}", std::process::id()));
        std::fs::write(&tmp, &text).map_err(|e| Error::io(&tmp, e))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| Error::io(&self.path, e))?;
        self.entries = entries;
        self.index = index;
        self.last = last;
        Ok(())
    }

    /// Appends one entry, which the index then names as the newest for its key.
    fn remember(&mut self, body: Body) {
        self.index.insert((body.unit.clone(), body.key.clone()), self.entries.len());
        self.entries.push(body);
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The newest trusted entry for `unit` with this key, not older than `max_age_ms`.
    pub fn lookup(&self, unit: &str, key: &str, now_ms: u64, max_age_ms: u64) -> Option<Hit> {
        // Only the newest entry for the key is asked. An older one usually has an
        // older time too, and a miss only reruns the check.
        let at = *self.index.get(&(unit.to_string(), key.to_string()))?;
        let entry = self.entries.get(at)?;
        (now_ms.saturating_sub(entry.time) <= max_age_ms).then_some(Hit {
            tests_run: entry.tests_run,
            duration_ms: entry.duration_ms,
            time: entry.time,
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
            self.index.clear();
            self.warnings.clear();
            self.last.clear();
        } else if self.over_limit() {
            self.compact(&secret)?;
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
        self.remember(body);
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

/// The SHA-256 of a file, read through one fixed buffer, so a large binary is
/// never held whole. `None` when it cannot be read.
fn file_sha256(path: &Path) -> Option<String> {
    rvow_base::hash::sha256_file_hex(path).ok()
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
        meta.is_file().then(|| file_sha256(&real)).flatten().map(|hash| (real.to_string_lossy().into_owned(), hash))
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

/// The key of a check result on a tree with uncommitted changes: the command text, the content of every
/// file of the work tree and the tool fingerprint. `None` when a listed tool is missing. A retry on the
/// same content finds it, and any other edit gives another key.
pub fn dirty_check_key(project: &Project, top: &Path, cmd: &str, tools: Option<&BTreeMap<String, String>>) -> Result<Option<String>> {
    let Some(tools) = tools else { return Ok(None) };
    let tree = crate::fingerprint::worktree(top, project)?;
    let value = serde_json::json!([CHECK_KEY_FORMAT, "dirty", env!("CARGO_PKG_VERSION"), cmd, tree, tools]);
    Ok(Some(sha256_hex(value.to_string().as_bytes())))
}

/// What every task key of one gate run shares: the HEAD listing, the tool
/// hashes and the runner map. A run reads the tree once and each tool binary
/// once, however many tasks it keys.
pub struct KeyBase<'a> {
    listing: String,
    tools: Option<BTreeMap<String, String>>,
    runners: &'a BTreeMap<String, crate::template::Runner>,
}

impl<'a> KeyBase<'a> {
    pub fn new(project: &Project, home: &Path, top: &Path, runners: &'a BTreeMap<String, crate::template::Runner>) -> Result<KeyBase<'a>> {
        Ok(KeyBase { listing: git::run(top, &["ls-tree", "-r", "-z", "HEAD"])?, tools: tool_fingerprint(project, home)?, runners })
    }

    /// The `tools` map, or `None` when a listed tool is missing, which keeps
    /// every key of the run uncacheable.
    pub fn tools(&self) -> Option<&BTreeMap<String, String>> {
        self.tools.as_ref()
    }

    /// The `(mode, type, path)` triples of the listing, borrowed from it.
    fn entries(&self) -> impl Iterator<Item = (&str, &str, &str)> {
        self.listing.split('\0').filter_map(|e| {
            let (meta, path) = e.split_once('\t')?;
            let mut parts = meta.split(' ');
            Some((parts.next()?, parts.nth(1)?, path))
        })
    }
}

/// The cache key of a task, or `None` when the task must not be cached: an
/// `env` command failed or a listed tool is missing. The tree comes from HEAD,
/// which the gate's clean worktree matches, so no file is read. A caller with
/// the tasks of one run builds one `KeyBase` and uses `cache_key_with`.
pub fn cache_key(
    project: &Project,
    home: &Path,
    top: &Path,
    config: &Config,
    task: &Task,
    requirements: &BTreeMap<String, String>,
    env: &Env,
) -> Result<Option<String>> {
    let runners = config::runners(project)?;
    let base = KeyBase::new(project, home, top, &runners)?;
    cache_key_with(&base, config, task, requirements, env)
}

/// `cache_key` under a key base the run already built.
pub fn cache_key_with(
    base: &KeyBase<'_>,
    config: &Config,
    task: &Task,
    requirements: &BTreeMap<String, String>,
    env: &Env,
) -> Result<Option<String>> {
    if env.commands.values().any(|v| v.starts_with("failed: ")) {
        return Ok(None);
    }
    let Some(tools) = base.tools.as_ref() else { return Ok(None) };
    for glob in &task.files {
        let one = fingerprint::globset(std::slice::from_ref(glob))?;
        if !base.entries().any(|(_, _, path)| one.is_match(path)) {
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
    let files: Vec<(&str, &str, &str)> = base.entries().filter(|(_, _, path)| scope.is_match(path)).collect();
    // A runner change keeps the verify text but runs a new command, so the
    // runner stamp keys the entry: the old proof's cache entry goes stale.
    // A verify that names no known runner keys on nothing, as before.
    let runner = crate::proof::runner_stamp(base.runners, task);
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
        runner,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;
    use crate::store::{Mode, init};

    /// A repository with `f00.txt` and its neighbours committed, a home for the
    /// cache key and a config that names one tool, so a key covers the tree
    /// listing, a tool binary and the runners.
    fn key_fixture(files: usize) -> (TempDir, Project, PathBuf, PathBuf) {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &home, Mode::Home).unwrap();
        for i in 0..files {
            std::fs::write(repo.join(format!("f{i:02}.txt")), "x").unwrap();
        }
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "files"]);
        std::fs::write(p.root.join(crate::store::CONFIG_FILE), "tools = [\"git\"]\n").unwrap();
        (tmp, p, repo, home)
    }

    /// `n` tasks of one file each, as a change's `tasks.md` holds them.
    fn tasks(n: usize) -> Vec<Task> {
        let text: String = (0..n).map(|i| format!("- TSK-new{} T\n  verify: true\n  files: f{i:02}.txt\n", i + 1)).collect();
        crate::tasks::parse_tasks(&text).0
    }

    fn reqs() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    /// The keys of the first `n` tasks and the git processes that made them,
    /// with the one key base a gate run builds.
    fn keys_and_spawns(
        n: usize,
        p: &Project,
        home: &Path,
        repo: &Path,
        config: &Config,
        runners: &BTreeMap<String, crate::template::Runner>,
        env: &Env,
    ) -> (usize, Vec<String>) {
        let before = git::spawns();
        let base = KeyBase::new(p, home, repo, runners).unwrap();
        let keys: Vec<String> = tasks(n).iter().map(|t| cache_key_with(&base, config, t, &reqs(), env).unwrap().unwrap()).collect();
        (git::spawns() - before, keys)
    }

    /// The per-task key the shared base replaced: the same listing, tools and
    /// runners, asked again for every task.
    fn reference_key(p: &Project, home: &Path, repo: &Path, config: &Config, task: &Task, env: &Env) -> String {
        let tools = tool_fingerprint(p, home).unwrap().unwrap();
        let listing = git::run(repo, &["ls-tree", "-r", "-z", "HEAD"]).unwrap();
        let tree: Vec<(&str, &str, &str)> = listing
            .split('\0')
            .filter_map(|e| {
                let (meta, path) = e.split_once('\t')?;
                let mut parts = meta.split(' ');
                Some((parts.next()?, parts.nth(1)?, path))
            })
            .collect();
        let mut globs = task.files.clone();
        globs.extend(config.inputs.iter().cloned());
        let scope = fingerprint::globset(&globs).unwrap();
        let files: Vec<&(&str, &str, &str)> = tree.iter().filter(|(_, _, path)| scope.is_match(path)).collect();
        let runners = config::runners(p).unwrap();
        let runner = crate::proof::runner_stamp(&runners, task);
        let value = serde_json::json!([
            KEY_FORMAT,
            env!("CARGO_PKG_VERSION"),
            task_hash(task),
            reqs(),
            files,
            env.os,
            env.arch,
            env.commands,
            env.vars,
            config.rerun_env,
            tools,
            runner,
        ]);
        sha256_hex(value.to_string().as_bytes())
    }

    /// One key base serves every task of a run: the keys of 8 tasks start as
    /// many git processes as the keys of 2, and the base is the one listing.
    #[test]
    fn scale_cachekey_two_and_eight_tasks_start_the_same_git_processes() {
        let (_tmp, p, repo, home) = key_fixture(8);
        let config = crate::config::load(&p, &home).unwrap();
        let runners = config::runners(&p).unwrap();
        let env = crate::fingerprint::env(&repo, &config).unwrap();
        // A warm-up key pays for the one-time git resolution.
        let _ = keys_and_spawns(2, &p, &home, &repo, &config, &runners, &env);
        let (two, keys2) = keys_and_spawns(2, &p, &home, &repo, &config, &runners, &env);
        let (eight, keys8) = keys_and_spawns(8, &p, &home, &repo, &config, &runners, &env);
        assert_eq!(keys2.len(), 2);
        assert_eq!(keys8.len(), 8);
        assert_eq!(two, 1, "one HEAD listing per run");
        assert_eq!(eight, two, "the keys of 8 tasks start {eight} git processes against {two} for 2");
    }

    /// The key of a task is the one the per-task computation produced, so the
    /// cache of an older rvow still answers.
    #[test]
    fn scale_cachekey_matches_the_per_task_key() {
        let (_tmp, p, repo, home) = key_fixture(2);
        let config = crate::config::load(&p, &home).unwrap();
        let runners = config::runners(&p).unwrap();
        let env = crate::fingerprint::env(&repo, &config).unwrap();
        let base = KeyBase::new(&p, &home, &repo, &runners).unwrap();
        for task in tasks(2) {
            let want = reference_key(&p, &home, &repo, &config, &task, &env);
            let got = cache_key_with(&base, &config, &task, &reqs(), &env).unwrap().unwrap();
            assert_eq!(got, want, "{} keys on what it keyed on before", task.id);
        }
    }

    /// A tool binary is hashed through one fixed buffer: a file larger than the
    /// buffer hashes as the whole file does.
    #[test]
    fn scale_cachekey_tool_hash_reads_through_one_buffer() {
        let tmp = TempDir::new();
        let path = tmp.0.join("tool.bin");
        let bytes: Vec<u8> = (0..(64 * 1024 * 3 + 7)).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(file_sha256(&path).as_deref(), Some(sha256_hex(&bytes).as_str()), "3 buffers of bytes hash as one file");
    }

    /// A cache file that only grows is compacted to the newest entry per key
    /// once it passes the limit, and the newest entry still answers.
    #[test]
    fn scale_cachekey_rerun_cache_compacts_past_its_limit() {
        let (_tmp, p, repo, home) = key_fixture(1);
        let lock = p.lock().unwrap();
        let mut cache = Cache::open(&p, &home).unwrap();
        for i in 0..2000u64 {
            cache.record(&lock, "t1", "k", Some(1), i, "m").unwrap();
            cache.record(&lock, "t2", "k", Some(1), i, "m").unwrap();
        }
        let size = std::fs::metadata(&cache.path).unwrap().len();
        assert!(size <= CACHE_BYTES_LIMIT, "2000 entries left {size} bytes, over the limit of {CACHE_BYTES_LIMIT}");
        drop(cache);
        let _ = repo;
        let reopened = Cache::open(&p, &home).unwrap();
        assert!(reopened.warnings().is_empty(), "a compacted file stays trusted: {:?}", reopened.warnings());
        assert_eq!(reopened.lookup("t1", "k", now_ms(), u64::MAX).map(|h| h.duration_ms), Some(1999), "the newest entry survives");
        assert_eq!(reopened.lookup("t2", "k", now_ms(), u64::MAX).map(|h| h.duration_ms), Some(1999), "and so does the other key's");
    }

    #[test]
    fn scale_cachekey_many_distinct_keys_stay_bounded_and_append() {
        let (_tmp, p, _repo, home) = key_fixture(1);
        let lock = p.lock().unwrap();
        let mut cache = Cache::open(&p, &home).unwrap();
        for i in 0..3000u64 {
            cache.record(&lock, "t", &format!("{i:064}"), Some(1), i, "m").unwrap();
        }
        let size = std::fs::metadata(&cache.path).unwrap().len();
        assert!(size <= CACHE_BYTES_LIMIT, "3000 keys left {size} bytes, over the limit of {CACHE_BYTES_LIMIT}");
        cache.record(&lock, "t", &format!("{:064}", 3000), Some(1), 3000, "m").unwrap();
        assert!(std::fs::metadata(&cache.path).unwrap().len() > size, "the record after a compaction appends, it does not compact again");
        drop(cache);
        let reopened = Cache::open(&p, &home).unwrap();
        assert!(reopened.warnings().is_empty(), "{:?}", reopened.warnings());
        assert!(reopened.lookup("t", &format!("{:064}", 3000), now_ms(), u64::MAX).is_some(), "the newest key is kept");
        assert!(reopened.lookup("t", &format!("{:064}", 0), now_ms(), u64::MAX).is_none(), "the oldest key is dropped");
    }

    fn body(unit: &str, key: &str, time: u64) -> Body {
        Body {
            unit: unit.into(),
            key: key.into(),
            tests_run: None,
            duration_ms: 1,
            rvow: "0".into(),
            machine: "m".into(),
            time,
            prev: String::new(),
        }
    }

    /// A cache holding `bodies` in file order, with the index `open` builds.
    fn cache_of(bodies: Vec<Body>) -> Cache {
        let mut cache = Cache {
            path: PathBuf::new(),
            home: PathBuf::new(),
            entries: Vec::new(),
            index: std::collections::HashMap::new(),
            warnings: Vec::new(),
            last: String::new(),
        };
        for body in bodies {
            cache.remember(body);
        }
        cache
    }

    #[test]
    fn cache_limit_gate_cache_lookup_returns_the_newest_matching_entry() {
        let cache = cache_of(vec![body("t1", "k", 10), body("t2", "k", 20), body("t1", "k", 30), body("t1", "other", 40)]);
        assert_eq!(cache.lookup("t1", "k", 100, 1000).map(|h| h.time), Some(30), "the newest entry for the key answers");
        assert_eq!(cache.lookup("t2", "k", 100, 1000).map(|h| h.time), Some(20), "another unit keeps its own entry");
        assert!(cache.lookup("t3", "k", 100, 1000).is_none(), "an unknown unit has no entry");
        assert!(cache.lookup("t1", "k", 100, 20).is_none(), "the newest entry is too old to answer");
    }
}
