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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Env {
    pub os: String,
    pub arch: String,
    /// Each configured command and the first line of its output.
    pub commands: BTreeMap<String, String>,
    /// Each configured variable and its value, or empty when unset.
    pub vars: BTreeMap<String, String>,
}

pub fn env(top: &Path, config: &Config) -> Result<Env> {
    let mut commands = BTreeMap::new();
    for cmd in &config.env {
        let out = runner::run(cmd, top, &[], ENV_TIMEOUT, &mut std::io::sink())?;
        let first = out.tail.first().cloned().unwrap_or_default();
        let value = if out.passed() { first } else { format!("failed: {first}") };
        commands.insert(cmd.clone(), value);
    }
    let vars = config.env_vars.iter().map(|k| (k.clone(), std::env::var(k).unwrap_or_default())).collect();
    Ok(Env { os: std::env::consts::OS.into(), arch: std::env::consts::ARCH.into(), commands, vars })
}

impl Env {
    /// The parts of the environment that make a proof stale when they change.
    pub fn key(&self) -> String {
        let v = serde_json::json!({"commands": self.commands, "vars": self.vars});
        sha256_hex(v.to_string().as_bytes())
    }
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
        };
        let a = env(&tmp.0, &cfg).unwrap();
        assert_eq!(a.commands["echo v1"], "v1");
        assert_eq!(a.vars["RVOW_TEST_UNSET_VAR"], "");
        cfg.env = vec!["echo v2".into()];
        assert_ne!(env(&tmp.0, &cfg).unwrap().key(), a.key());
    }
}
