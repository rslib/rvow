//! What a proof depends on: the input files, the tool versions and the
//! declared environment variables. Together they form the staleness key.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::git;
use crate::runner;
use crate::store::{Mode, Project, REPO_DIR};
use crate::vow::sha256_hex;

pub use rvow_base::glob::globset;

const ENV_TIMEOUT: Duration = Duration::from_secs(30);
const LFS_POINTER: &[u8] = b"version https://git-lfs.github.com/spec/";
/// The most file hashes this process keeps: a repository with more files than
/// this clears the map and keeps the newest.
const HASH_LIMIT: usize = 8192;

/// Tracked files plus untracked files that git does not ignore.
pub fn candidate_files(top: &Path) -> Result<Vec<String>> {
    Ok(candidates(top)?.into_iter().map(|(rel, _)| rel).collect())
}

/// Every candidate file of `top`, each with whether git leaves it untracked.
/// One `ls-files` listing answers both lists `worktree` needs, since `-t` tags
/// an untracked file with `?`; the sort and the dedup keep the order the two
/// listings of the old code gave.
fn candidates(top: &Path) -> Result<Vec<(String, bool)>> {
    let out = git::run(top, &["ls-files", "-co", "-t", "--exclude-standard"])?;
    let mut files: Vec<(String, bool)> = Vec::new();
    for line in out.lines() {
        let Some((rel, untracked)) = tagged(line) else { continue };
        files.push((rel.to_string(), untracked));
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// The path of one `ls-files -t` line and whether git leaves it untracked.
fn tagged(line: &str) -> Option<(&str, bool)> {
    match line.strip_prefix("? ") {
        Some(rel) => Some((rel, true)),
        None => line.get(2..).map(|rel| (rel, false)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inputs {
    pub hash: String,
    pub files: usize,
    /// Files that are still Git LFS pointers.
    pub lfs_pointers: Vec<String>,
}

fn file_hash(top: &Path, rel: &str) -> Result<(String, bool)> {
    let path = top.join(rel);
    let meta = std::fs::symlink_metadata(&path);
    match meta {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(("deleted".into(), false)),
        Err(e) => Err(Error::io(&path, e)),
        Ok(m) if m.file_type().is_symlink() => {
            let target = std::fs::read_link(&path).map_err(|e| Error::io(&path, e))?;
            Ok((format!("link:{}", target.display()), false))
        }
        Ok(m) if m.is_dir() => {
            // A submodule: its commit and its uncommitted state.
            let head = git::run(&path, &["rev-parse", "HEAD"]).unwrap_or_default();
            let dirty = git::run(&path, &["status", "--porcelain"]).unwrap_or_default();
            Ok((format!("sub:{}", sha256_hex(format!("{head}\n{dirty}").as_bytes())), false))
        }
        Ok(m) => {
            let (len, mtime) = (m.len(), m.modified().ok());
            let inode = inode_of(&m);
            if let Some(known) = kept_hash(&path, len, mtime, inode) {
                return Ok(known);
            }
            READS.with(|n| n.set(n.get() + 1));
            let (hash, lfs) = stream_hash(&path).map_err(|e| Error::io(&path, e))?;
            // A file written within the clock's last ticks can change again
            // with the same size and mtime, so its stat cannot vouch for it yet.
            let settled = mtime.and_then(|m| SystemTime::now().duration_since(m).ok()).is_some_and(|age| age >= RACY_WINDOW);
            if settled && let Ok(mut kept) = HASHES.get_or_init(Default::default).lock() {
                keep_hash(&mut kept, path, Hashed { hash: hash.clone(), lfs, len, mtime, inode });
            }
            Ok((hash, lfs))
        }
    }
}

/// How old a file's mtime must be before its hash is kept by its stat, as git
/// does for a racily clean index entry; 2 s covers the coarsest common clock.
const RACY_WINDOW: Duration = Duration::from_secs(2);

/// The bytes `stream_hash` reads at a time, so a large file costs no more memory.
const HASH_CHUNK: usize = 64 * 1024;

/// The lowercase hex SHA-256 of the file at `path`, the same as `sha256_hex`
/// of its whole text, and whether it starts as a git LFS pointer.
fn stream_hash(path: &Path) -> std::io::Result<(String, bool)> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut buf = vec![0u8; HASH_CHUNK];
    let mut sha = Sha256::new();
    let mut head = Vec::with_capacity(LFS_POINTER.len());
    loop {
        let n = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if head.len() < LFS_POINTER.len() {
            let take = (LFS_POINTER.len() - head.len()).min(n);
            head.extend_from_slice(&buf[..take]);
        }
        sha.update(&buf[..n]);
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut hex = String::with_capacity(64);
    for b in sha.finalize().iter() {
        hex.push(HEX[usize::from(b >> 4)] as char);
        hex.push(HEX[usize::from(b & 0x0f)] as char);
    }
    Ok((hex, head == LFS_POINTER))
}

/// One file's hash, with the stat it was made from: a file whose size, mtime
/// and inode all match kept ones is that same file, so its hash stands.
struct Hashed {
    hash: String,
    lfs: bool,
    len: u64,
    mtime: Option<SystemTime>,
    inode: Option<u64>,
}

/// The file hashes this process kept, by path.
static HASHES: OnceLock<Mutex<HashMap<PathBuf, Hashed>>> = OnceLock::new();

/// The hash and LFS flag this process kept for `path`, when its stat is the one
/// the hash was made from.
fn kept_hash(path: &Path, len: u64, mtime: Option<SystemTime>, inode: Option<u64>) -> Option<(String, bool)> {
    let kept = HASHES.get_or_init(Default::default).lock().ok()?;
    kept.get(path).filter(|h| h.len == len && h.mtime == mtime && h.inode == inode).map(|h| (h.hash.clone(), h.lfs))
}

/// Keeps one file hash, clearing the map first when it is full.
fn keep_hash(map: &mut HashMap<PathBuf, Hashed>, path: PathBuf, known: Hashed) {
    if map.len() >= HASH_LIMIT {
        map.clear();
    }
    map.insert(path, known);
}

/// How many files this process has read for a hash. A test reads it before and
/// after a fingerprint to prove an unchanged tree reads no file again.
#[doc(hidden)]
pub fn file_reads() -> usize {
    READS.with(std::cell::Cell::get)
}

thread_local! {
    /// The files this thread has read for a hash.
    static READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many file hashes this process holds. A test reads it to prove the limit.
#[cfg(test)]
fn hash_cache_len() -> usize {
    HASHES.get().and_then(|kept| kept.lock().ok()).map(|kept| kept.len()).unwrap_or(0)
}

/// The inode of a file, `None` where the platform has none.
fn inode_of(m: &std::fs::Metadata) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(m.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = m;
        None
    }
}

/// Hashes every candidate file that matches `globs`, by path and content.
pub fn inputs(top: &Path, globs: &[String]) -> Result<Inputs> {
    inputs_in(top, &candidate_files(top)?, globs)
}

/// Hashes, from a given candidate list, every file that matches `globs`.
pub fn inputs_in(top: &Path, files: &[String], globs: &[String]) -> Result<Inputs> {
    let set = globset(globs)?;
    let mut list = String::new();
    let mut count = 0;
    let mut lfs_pointers = Vec::new();
    for rel in files.iter().filter(|f| set.is_match(f)) {
        let (hash, lfs) = file_hash(top, rel)?;
        if lfs {
            lfs_pointers.push(rel.clone());
        }
        list.push_str(rel);
        list.push('\0');
        list.push_str(&hash);
        list.push('\n');
        count += 1;
    }
    Ok(Inputs { hash: sha256_hex(list.as_bytes()), files: count, lfs_pointers })
}

/// Hashes every tracked file plus every untracked file that git does not
/// ignore, by path, content and whether git tracks it, so a check that reads
/// the index sees a `git add` as a new tree. Two runs on the same worktree
/// share it, while any fix anywhere changes it. In repo mode the in-repo rvow tree
/// (`rvow/`) is skipped: each prove appends to its ledger, so keeping it
/// would make every fingerprint differ.
pub fn worktree(top: &Path, project: &Project) -> Result<String> {
    let skip_repo = project.meta.mode == Mode::Repo;
    let prefix = format!("{REPO_DIR}/");
    let mut list = String::new();
    for (rel, untracked) in candidates(top)? {
        if skip_repo && (rel == REPO_DIR || rel.starts_with(&prefix)) {
            continue;
        }
        let (hash, _) = file_hash(top, &rel)?;
        list.push_str(&rel);
        list.push('\0');
        list.push_str(&hash);
        if untracked {
            list.push_str("\0untracked");
        }
        list.push('\n');
    }
    Ok(sha256_hex(list.as_bytes()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Env {
    pub os: String,
    pub arch: String,
    /// Each configured command and the first line of its output.
    pub commands: BTreeMap<String, String>,
    /// Configured values, with secret-named variables stored as stable digests.
    pub vars: BTreeMap<String, String>,
    /// The CPU model, for people; never part of the staleness key.
    pub cpu: String,
    /// The number of cores, for people; never part of the staleness key.
    pub cores: Option<u64>,
    /// The installed memory in bytes, for people; never part of the stale key.
    pub memory: Option<u64>,
}

pub fn env(top: &Path, config: &Config) -> Result<Env> {
    env_with(top, config, &[])
}

/// The environment to compare with recorded proofs, without the CPU model,
/// cores and memory: no proof key reads them, and on macOS each one starts a
/// `sysctl` process. A proof that records its environment uses `env`.
pub fn check_env(top: &Path, config: &Config) -> Result<Env> {
    env_parts(top, config, &[], false)
}

pub fn env_with(top: &Path, config: &Config, overrides: &[(String, String)]) -> Result<Env> {
    env_parts(top, config, overrides, true)
}

fn env_parts(top: &Path, config: &Config, overrides: &[(String, String)], with_machine: bool) -> Result<Env> {
    // Never kept across calls: a configured command such as `rustc --version`
    // answers differently once a tool is upgraded under a long-lived daemon.
    let parts = parts_of(top, config, overrides)?;
    let (cpu, cores, memory) = if with_machine { machine() } else { (String::new(), None, None) };
    Ok(Env {
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        commands: parts.commands,
        vars: parts.vars,
        cpu,
        cores,
        memory,
    })
}

/// The two maps a proof records of one environment.
#[derive(Clone)]
struct Parts {
    commands: BTreeMap<String, String>,
    vars: BTreeMap<String, String>,
}

/// Runs the configured env commands and reads the configured variables.
fn parts_of(top: &Path, config: &Config, overrides: &[(String, String)]) -> Result<Parts> {
    let home = std::env::var("HOME").ok();
    let repo = top.display().to_string();
    let mut commands = BTreeMap::new();
    for cmd in &config.env {
        let out = runner::run(cmd, top, overrides, ENV_TIMEOUT, &mut std::io::sink())?;
        let first = out.tail.first().cloned().unwrap_or_default();
        let value = if out.passed() { first } else { format!("failed: {first}") };
        let masked = runner::mask(cmd, home.as_deref(), &repo);
        let name = if masked == *cmd { masked } else { format!("{masked} [sha256:{}]", sha256_hex(cmd.as_bytes())) };
        commands.insert(name, value);
    }
    let vars = config
        .env_vars
        .iter()
        .map(|k| {
            let raw =
                overrides.iter().rev().find(|(name, _)| name == k).map(|(_, value)| Ok(value.clone())).unwrap_or_else(|| std::env::var(k));
            let value = match raw {
                Ok(value) if runner::names_secret(k) => format!("sha256:{}", sha256_hex(value.as_bytes())),
                Ok(value) => {
                    let masked = runner::mask(&value, home.as_deref(), &repo);
                    if masked == value { value } else { format!("sha256:{}", sha256_hex(value.as_bytes())) }
                }
                Err(_) => String::new(),
            };
            (k.clone(), value)
        })
        .collect();
    Ok(Parts { commands, vars })
}

/// The machine description, read once per process: on macOS each field starts
/// a `sysctl` process, and none of them changes while the process lives.
fn machine() -> (String, Option<u64>, Option<u64>) {
    MACHINE.get_or_init(read_machine).clone()
}

/// The machine description this process read, if it read it.
static MACHINE: OnceLock<(String, Option<u64>, Option<u64>)> = OnceLock::new();

/// How many times this process read the machine description. A test proves the
/// CPU, the cores and the memory are asked for once, whatever the number of
/// calls a hook makes.
#[doc(hidden)]
pub fn machine_reads() -> usize {
    MACHINE_READS.load(std::sync::atomic::Ordering::Relaxed)
}

static MACHINE_READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The CPU model, core count and memory, best-effort: any failure leaves an
/// empty model and no counts. Never an error, so a proof cannot fail because
/// the machine could not be described.
fn read_machine() -> (String, Option<u64>, Option<u64>) {
    MACHINE_READS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    #[cfg(target_os = "macos")]
    {
        let cpu = sysctl("machdep.cpu.brand_string").unwrap_or_default();
        let cores = sysctl("hw.ncpu").and_then(|s| s.parse().ok());
        let memory = sysctl("hw.memsize").and_then(|s| s.parse().ok());
        (cpu, cores, memory)
    }
    #[cfg(target_os = "linux")]
    {
        (linux_cpu_model(), linux_cores(), linux_memory())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        (String::new(), None, None)
    }
}

#[cfg(target_os = "macos")]
fn sysctl(name: &str) -> Option<String> {
    let out = std::process::Command::new("sysctl").args(["-n", name]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

#[cfg(target_os = "linux")]
fn linux_cpu_model() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("model name"))
                .and_then(|value| value.split_once(':'))
                .map(|(_, name)| name.trim().to_string())
        })
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn linux_cores() -> Option<u64> {
    std::fs::read_to_string("/proc/cpuinfo").ok().map(|t| t.lines().filter(|l| l.starts_with("processor")).count()).map(|n| n as u64)
}

#[cfg(target_os = "linux")]
fn linux_memory() -> Option<u64> {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("MemTotal"))
                .and_then(|value| value.split_whitespace().nth(1))
                .and_then(|value| value.parse::<u64>().ok())
        })
        .and_then(|kb| kb.checked_mul(1024))
}

impl Env {
    /// The parts of the environment that make a proof stale when they change.
    pub fn key(&self) -> String {
        let v = serde_json::json!({"commands": self.commands, "vars": self.vars});
        sha256_hex(v.to_string().as_bytes())
    }
}

/// A detected CI provider: its short name, the run id and a URL for that run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ci {
    pub provider: String,
    pub run: String,
    pub url: String,
}

enum Provider {
    Github,
    Gitlab,
    Circle,
    Buildkite,
    Azure,
    Jenkins,
    Generic,
}

impl Provider {
    fn name(&self) -> &'static str {
        match self {
            Self::Github => "github-actions",
            Self::Gitlab => "gitlab-ci",
            Self::Circle => "circleci",
            Self::Buildkite => "buildkite",
            Self::Azure => "azure-pipelines",
            Self::Jenkins => "jenkins",
            Self::Generic => "generic",
        }
    }
}

/// The CI provider the process runs under, read from the environment, if any.
pub fn ci() -> Option<Ci> {
    ci_from(&|k| std::env::var(k).ok())
}

/// Detect the CI provider from `get`, which maps a variable name to its value.
/// `ci()` wraps this with the process environment; tests call it directly with
/// a fixed map so they do not mutate the process-wide environment.
pub fn ci_from(get: &dyn Fn(&str) -> Option<String>) -> Option<Ci> {
    if get("GITHUB_ACTIONS").as_deref() != Some("true") {
        let enabled = |key: &str| get(key).is_some_and(|value| value.eq_ignore_ascii_case("true") || value == "1");
        for (marker, provider, run_key, url_key) in [
            ("GITLAB_CI", Provider::Gitlab, "CI_PIPELINE_ID", "CI_PIPELINE_URL"),
            ("CIRCLECI", Provider::Circle, "CIRCLE_BUILD_NUM", "CIRCLE_BUILD_URL"),
            ("BUILDKITE", Provider::Buildkite, "BUILDKITE_BUILD_ID", "BUILDKITE_BUILD_URL"),
        ] {
            if enabled(marker) {
                return Some(Ci {
                    provider: provider.name().into(),
                    run: get(run_key).unwrap_or_default(),
                    url: get(url_key).unwrap_or_default(),
                });
            }
        }
        if enabled("TF_BUILD") {
            let run = get("BUILD_BUILDID").unwrap_or_default();
            let collection = get("SYSTEM_COLLECTIONURI").unwrap_or_default();
            let project = get("SYSTEM_TEAMPROJECT").unwrap_or_default();
            let url = if collection.is_empty() || project.is_empty() || run.is_empty() {
                String::new()
            } else {
                format!("{}/{}/_build/results?buildId={run}", collection.trim_end_matches('/'), project.replace(' ', "%20"))
            };
            return Some(Ci { provider: Provider::Azure.name().into(), run, url });
        }
        if get("JENKINS_URL").is_some_and(|url| !url.is_empty()) {
            return Some(Ci {
                provider: Provider::Jenkins.name().into(),
                run: get("BUILD_NUMBER").unwrap_or_default(),
                url: get("BUILD_URL").unwrap_or_default(),
            });
        }
        return enabled("CI").then(|| Ci { provider: Provider::Generic.name().into(), run: String::new(), url: String::new() });
    }
    // GitHub builds a run URL from three variables; the run id alone has none.
    let run = get("GITHUB_RUN_ID").unwrap_or_default();
    let repo = get("GITHUB_REPOSITORY").unwrap_or_default();
    let server = get("GITHUB_SERVER_URL").unwrap_or_else(|| "https://github.com".into());
    let url = if run.is_empty() || repo.is_empty() { String::new() } else { format!("{server}/{repo}/actions/runs/{run}") };
    Some(Ci { provider: Provider::Github.name().into(), run, url })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_fingerprint_stream_hash_matches_the_whole_file_hash() {
        let dir = std::env::temp_dir().join(format!("rvow-stream-hash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("f");
        for len in [0, 1, HASH_CHUNK - 1, HASH_CHUNK, HASH_CHUNK + 1, 3 * HASH_CHUNK + 7] {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
            std::fs::write(&file, &bytes).unwrap();
            assert_eq!(stream_hash(&file).unwrap(), (sha256_hex(&bytes), false), "{len} bytes");
        }
        let mut lfs = LFS_POINTER.to_vec();
        lfs.extend_from_slice(b"v1\noid sha256:00\n");
        std::fs::write(&file, &lfs).unwrap();
        assert_eq!(stream_hash(&file).unwrap(), (sha256_hex(&lfs), true));
        std::fs::write(&file, &LFS_POINTER[..10]).unwrap();
        assert!(!stream_hash(&file).unwrap().1, "a prefix of the pointer line is no pointer");
        let _ = std::fs::remove_dir_all(&dir);
    }
    use crate::git::testrepo::*;

    #[test]
    fn inputs_follow_content_not_commits() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/a.rs"), "a").unwrap();
        std::fs::write(repo.join(".gitignore"), "target/\n").unwrap();
        std::fs::create_dir_all(repo.join("target")).unwrap();
        std::fs::write(repo.join("target/x.rs"), "x").unwrap();
        let globs = vec!["src/**".to_string(), "target/**".to_string()];

        let before = inputs(&repo, &globs).unwrap();
        assert_eq!(before.files, 1, "ignored files never count");
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "c"]);
        assert_eq!(inputs(&repo, &globs).unwrap(), before, "a commit changes nothing");

        std::fs::write(repo.join("src/a.rs"), "b").unwrap();
        assert_ne!(inputs(&repo, &globs).unwrap().hash, before.hash);
        std::fs::write(repo.join("src/a.rs"), "a").unwrap();
        assert_eq!(inputs(&repo, &globs).unwrap(), before);

        std::fs::write(repo.join("src/b.rs"), "new").unwrap();
        assert_eq!(inputs(&repo, &globs).unwrap().files, 2, "untracked files count");
    }

    #[test]
    fn folder_entry_hashes_the_files_under_it() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        std::fs::create_dir_all(repo.join("crates/a/src")).unwrap();
        std::fs::create_dir_all(repo.join("crates/ab")).unwrap();
        std::fs::write(repo.join("crates/a/src/x.rs"), "x").unwrap();
        std::fs::write(repo.join("crates/ab/y.rs"), "y").unwrap();
        let globs = vec!["crates/a".to_string()];

        let before = inputs(&repo, &globs).unwrap();
        assert_eq!(before.files, 1, "the file in the folder counts and the sibling does not");
        std::fs::write(repo.join("crates/ab/y.rs"), "z").unwrap();
        assert_eq!(inputs(&repo, &globs).unwrap(), before, "a sibling change leaves the proof fresh");
        std::fs::write(repo.join("crates/a/src/x.rs"), "changed").unwrap();
        assert_ne!(inputs(&repo, &globs).unwrap().hash, before.hash, "a change in the folder makes the proof stale");
    }

    #[test]
    fn worktree_changes_when_a_new_file_is_staged() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = rvow_repo::store::init(&repo, &tmp.0.join("home"), crate::store::Mode::Home).unwrap();
        std::fs::write(repo.join("new.rs"), "new").unwrap();
        let untracked = worktree(&repo, &p).unwrap();
        git(&repo, &["add", "new.rs"]);
        let staged = worktree(&repo, &p).unwrap();
        assert_ne!(untracked, staged, "a check that reads the index can pass only after the add");
        git(&repo, &["commit", "-q", "-m", "c"]);
        assert_eq!(worktree(&repo, &p).unwrap(), staged, "a commit changes nothing");
    }

    #[test]
    fn lfs_pointers_are_reported() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        std::fs::write(repo.join("big.bin"), "version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 3\n").unwrap();
        let i = inputs(&repo, &["*.bin".to_string()]).unwrap();
        assert_eq!(i.lfs_pointers, ["big.bin"]);
    }

    #[test]
    fn env_key_tracks_commands_and_vars() {
        let tmp = TempDir::new();
        let mut cfg = crate::config::Config {
            base: "main".into(),
            land_branches: vec![],
            user_keys: Default::default(),
            timeout_secs: 60,
            inputs: vec![],
            test_globs: vec![],
            protect_infra: vec![],
            test_queries: Default::default(),
            checks: vec![],
            setup: vec![],
            rerun_env: Default::default(),
            env: vec!["echo v1".into()],
            env_vars: vec!["RVOW_TEST_UNSET_VAR".into()],
            machine: None,
            require_signature: false,
            allowed_signers: String::new(),
            interview_cap: 3,
            signing_key: None,
            agent: None,
            context: String::new(),
            rules: Default::default(),
            task_mode: None,
        };
        let a = env(&tmp.0, &cfg).unwrap();
        assert_eq!(a.commands["echo v1"], "v1");
        assert_eq!(a.vars["RVOW_TEST_UNSET_VAR"], "");
        cfg.env = vec!["echo v2".into()];
        assert_ne!(env(&tmp.0, &cfg).unwrap().key(), a.key());
    }

    #[test]
    fn env_machine_fields_stay_outside_the_key() {
        let tmp = TempDir::new();
        let cfg = crate::config::Config {
            base: "main".into(),
            land_branches: vec![],
            user_keys: Default::default(),
            timeout_secs: 60,
            inputs: vec![],
            test_globs: vec![],
            protect_infra: vec![],
            test_queries: Default::default(),
            checks: vec![],
            setup: vec![],
            rerun_env: Default::default(),
            env: vec!["echo v1".into()],
            env_vars: vec![],
            machine: None,
            require_signature: false,
            allowed_signers: String::new(),
            interview_cap: 3,
            signing_key: None,
            agent: None,
            context: String::new(),
            rules: Default::default(),
            task_mode: None,
        };
        let e = env(&tmp.0, &cfg).unwrap();
        // The machine fields are present and differ from machine to machine,
        // but they never move the staleness key.
        let mut other = e.clone();
        other.cpu = "A different CPU".into();
        other.cores = e.cores.map(|c| c + 4);
        other.memory = e.memory.map(|m| m / 2);
        assert_ne!(&other.cpu, &e.cpu, "the CPU fields differ between machines");
        assert_eq!(other.key(), e.key(), "machine fields never make a proof stale");
    }

    #[test]
    fn ci_detection_names_github_actions_with_run_id_and_url() {
        let env = |k: &str| -> Option<String> {
            match k {
                "GITHUB_ACTIONS" => Some("true".into()),
                "GITHUB_RUN_ID" => Some("1658821493".into()),
                "GITHUB_REPOSITORY" => Some("octocat/Hello-World".into()),
                _ => None,
            }
        };
        let c = ci_from(&env).unwrap();
        assert_eq!(c.provider, "github-actions");
        assert_eq!(c.run, "1658821493");
        assert_eq!(c.url, "https://github.com/octocat/Hello-World/actions/runs/1658821493");
    }

    #[test]
    fn ci_detection_off_without_the_actions_marker() {
        let env = |k: &str| -> Option<String> {
            match k {
                "GITHUB_RUN_ID" => Some("1".into()),
                "GITHUB_REPOSITORY" => Some("o/r".into()),
                _ => None,
            }
        };
        assert_eq!(ci_from(&env), None);
    }

    /// A second fingerprint of an unchanged tree hashes no file again, and one
    /// `ls-files` listing replaces the two the old `worktree` ran.
    #[test]
    fn scale_fingerprint_second_pass_reads_no_file_and_starts_fewer_git_processes() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = rvow_repo::store::init(&repo, &tmp.0.join("home"), crate::store::Mode::Home).unwrap();
        for i in 0..8 {
            std::fs::write(repo.join(format!("f{i}.rs")), "x").unwrap();
        }
        settle(&repo, 10);
        // The two listings the old `worktree` ran, counted as it ran them.
        let before = git::spawns();
        let _ = candidate_files(&repo).unwrap();
        let _ = git::run(&repo, &["ls-files", "-o", "--exclude-standard"]).unwrap();
        let old = git::spawns() - before;

        let reads = file_reads();
        let before = git::spawns();
        let first = worktree(&repo, &p).unwrap();
        let listing = git::spawns() - before;
        assert!(file_reads() > reads, "the first pass hashes the files");

        let reads = file_reads();
        let before = git::spawns();
        assert_eq!(worktree(&repo, &p).unwrap(), first, "the tree did not change");
        let second = git::spawns() - before;
        assert_eq!(file_reads(), reads, "the second pass reads no file");
        assert_eq!((listing, second), (1, 1), "one listing answers both lists of the worktree");
        assert!((listing, second) < (old, old), "a pass starts fewer git processes than the {old} of the old listings");
    }

    /// A file whose stat changed is hashed again, and no other file is read.
    #[test]
    fn scale_fingerprint_a_changed_file_hashes_again() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = rvow_repo::store::init(&repo, &tmp.0.join("home"), crate::store::Mode::Home).unwrap();
        std::fs::write(repo.join("f.rs"), "one").unwrap();
        settle(&repo, 10);
        let one = worktree(&repo, &p).unwrap();

        let reads = file_reads();
        // The same length, so only the stat tells the two apart.
        std::fs::write(repo.join("f.rs"), "two").unwrap();
        let file = std::fs::File::options().write(true).open(repo.join("f.rs")).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(5)).unwrap();
        let two = worktree(&repo, &p).unwrap();
        assert_ne!(two, one, "a rewritten file makes the tree fingerprint differ");
        assert_eq!(file_reads() - reads, 1, "only the changed file is hashed again");

        let reads = file_reads();
        assert_eq!(worktree(&repo, &p).unwrap(), two);
        assert_eq!(file_reads(), reads, "an unchanged file is not read again");
    }

    /// Sets the mtime of every work tree file of `repo` to `secs` ago, so the
    /// hash cache may keep them.
    fn settle(repo: &Path, secs: u64) {
        let at = SystemTime::now() - Duration::from_secs(secs);
        for rel in candidate_files(repo).unwrap() {
            let file = std::fs::File::options().write(true).open(repo.join(rel)).unwrap();
            file.set_modified(at).unwrap();
        }
    }

    #[test]
    fn scale_fingerprint_a_fresh_file_is_hashed_each_time() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = rvow_repo::store::init(&repo, &tmp.0.join("home"), crate::store::Mode::Home).unwrap();
        settle(&repo, 10);
        std::fs::write(repo.join("f.rs"), "one").unwrap();
        let _ = worktree(&repo, &p).unwrap();
        let reads = file_reads();
        let _ = worktree(&repo, &p).unwrap();
        assert_eq!(file_reads() - reads, 1, "a file written within the racy window is read again");
    }

    /// The hash cache clears itself when it is full, so it holds at most its
    /// limit however many files a repository has.
    #[test]
    fn scale_fingerprint_hash_cache_stays_under_its_limit() {
        let mut map = HashMap::new();
        for i in 0..(HASH_LIMIT * 2 + 4) {
            keep_hash(&mut map, PathBuf::from(i.to_string()), Hashed { hash: "h".into(), lfs: false, len: 0, mtime: None, inode: None });
            assert!(map.len() <= HASH_LIMIT, "{} hashes kept at {i}", map.len());
        }
        assert!(map.contains_key(&PathBuf::from((HASH_LIMIT * 2 + 3).to_string())), "a full map clears and takes the newest hash");

        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = rvow_repo::store::init(&repo, &tmp.0.join("home"), crate::store::Mode::Home).unwrap();
        std::fs::write(repo.join("f.rs"), "x").unwrap();
        let _ = worktree(&repo, &p).unwrap();
        assert!(hash_cache_len() <= HASH_LIMIT, "{} hashes kept", hash_cache_len());
    }

    /// The hook path answers the configured commands from the process, not
    /// once per call, and a changed command list asks again.
    #[test]
    fn scale_fingerprint_machine_is_read_once_per_process() {
        let _ = machine();
        let _ = machine();
        assert_eq!(machine_reads(), 1, "the CPU, the cores and the memory are read once per process");
    }
}
