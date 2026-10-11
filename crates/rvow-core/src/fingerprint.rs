//! What a proof depends on: the input files, the tool versions and the
//! declared environment variables. Together they form the staleness key.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::git;
use crate::runner;
use crate::store::{Mode, Project, REPO_DIR};
use crate::vow::sha256_hex;

const ENV_TIMEOUT: Duration = Duration::from_secs(30);
const LFS_POINTER: &[u8] = b"version https://git-lfs.github.com/spec/";

pub fn globset(patterns: &[String]) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        let glob = GlobBuilder::new(p)
            .literal_separator(true)
            .build()
            .map_err(|e| Error::new("glob", format!("bad glob {p:?}: {e}"), "fix the glob"))?;
        b.add(glob);
    }
    b.build().map_err(|e| Error::new("glob", e.to_string(), "fix the globs"))
}

/// Tracked files plus untracked files that git does not ignore.
pub fn candidate_files(top: &Path) -> Result<Vec<String>> {
    let out = git::run(top, &["ls-files", "-co", "--exclude-standard"])?;
    let mut files: Vec<String> = out.lines().map(String::from).collect();
    files.sort();
    files.dedup();
    Ok(files)
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
        Ok(_) => {
            let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
            Ok((sha256_hex(&bytes), bytes.starts_with(LFS_POINTER)))
        }
    }
}

/// Hashes every candidate file that matches `globs`, by path and content.
pub fn inputs(top: &Path, globs: &[String]) -> Result<Inputs> {
    let set = globset(globs)?;
    let mut list = String::new();
    let mut files = 0;
    let mut lfs_pointers = Vec::new();
    for rel in candidate_files(top)?.into_iter().filter(|f| set.is_match(f)) {
        let (hash, lfs) = file_hash(top, &rel)?;
        if lfs {
            lfs_pointers.push(rel.clone());
        }
        list.push_str(&rel);
        list.push('\0');
        list.push_str(&hash);
        list.push('\n');
        files += 1;
    }
    Ok(Inputs { hash: sha256_hex(list.as_bytes()), files, lfs_pointers })
}

/// Hashes every tracked file plus every untracked file that git does not
/// ignore, by path and content. Two runs on the same worktree share it,
/// while any fix anywhere changes it. In repo mode the in-repo rvow tree
/// (`rvow/`) is skipped: each prove appends to its ledger, so keeping it
/// would make every fingerprint differ.
pub fn worktree(top: &Path, project: &Project) -> Result<String> {
    let skip_repo = project.meta.mode == Mode::Repo;
    let prefix = format!("{REPO_DIR}/");
    let mut list = String::new();
    for rel in candidate_files(top)? {
        if skip_repo && (rel == REPO_DIR || rel.starts_with(&prefix)) {
            continue;
        }
        let (hash, _) = file_hash(top, &rel)?;
        list.push_str(&rel);
        list.push('\0');
        list.push_str(&hash);
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

pub(crate) fn env_with(top: &Path, config: &Config, overrides: &[(String, String)]) -> Result<Env> {
    let mut commands = BTreeMap::new();
    for cmd in &config.env {
        let out = runner::run(cmd, top, overrides, ENV_TIMEOUT, &mut std::io::sink())?;
        let first = out.tail.first().cloned().unwrap_or_default();
        let value = if out.passed() { first } else { format!("failed: {first}") };
        let masked = runner::mask(cmd, std::env::var("HOME").ok().as_deref(), &top.display().to_string());
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
                    let masked = runner::mask(&value, std::env::var("HOME").ok().as_deref(), &top.display().to_string());
                    if masked == value { value } else { format!("sha256:{}", sha256_hex(value.as_bytes())) }
                }
                Err(_) => String::new(),
            };
            (k.clone(), value)
        })
        .collect();
    let (cpu, cores, memory) = machine();
    Ok(Env { os: std::env::consts::OS.into(), arch: std::env::consts::ARCH.into(), commands, vars, cpu, cores, memory })
}

/// The CPU model, core count and memory, best-effort: any failure leaves an
/// empty model and no counts. Never an error, so a proof cannot fail because
/// the machine could not be described.
fn machine() -> (String, Option<u64>, Option<u64>) {
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
            user_keys: Default::default(),
            timeout_secs: 60,
            inputs: vec![],
            test_globs: vec![],
            protect_infra: vec![],
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
            user_keys: Default::default(),
            timeout_secs: 60,
            inputs: vec![],
            test_globs: vec![],
            protect_infra: vec![],
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
}
