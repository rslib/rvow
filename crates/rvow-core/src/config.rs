//! Settings from `$RVOW_HOME/config.toml` and the project's `config.toml`.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::store::{CONFIG_FILE, Mode, Project};
use crate::tasks::parse_duration;

/// One config file as written. Every key is optional.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct File {
    pub base: Option<String>,
    pub timeout: Option<String>,
    pub inputs: Option<Vec<String>>,
    pub test_globs: Option<Vec<String>>,
    pub protect_infra: Option<Vec<String>>,
    pub checks: Option<Vec<String>>,
    pub setup: Option<Vec<String>>,
    pub rerun_env: Option<BTreeMap<String, String>>,
    pub env: Option<Vec<String>>,
    pub env_vars: Option<Vec<String>>,
    /// Global only: a name for this computer.
    pub machine: Option<String>,
}

/// The settings in force. Every field except `machine` is part of the vow.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Config {
    pub base: String,
    pub timeout_secs: u64,
    pub inputs: Vec<String>,
    pub test_globs: Vec<String>,
    pub protect_infra: Vec<String>,
    pub checks: Vec<String>,
    pub setup: Vec<String>,
    pub rerun_env: BTreeMap<String, String>,
    pub env: Vec<String>,
    pub env_vars: Vec<String>,
    #[serde(skip)]
    pub machine: Option<String>,
}

/// Files that enforce rvow itself. Always protected, whatever the config says.
pub const ENFORCEMENT: &[&str] = &[".claude/settings.json", ".claude/settings.local.json"];

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn read_file(path: &Path) -> Result<File> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(File::default()),
        Err(e) => return Err(Error::io(path, e)),
    };
    toml::from_str(&text).map_err(|e| {
        Error::new(
            "config",
            format!("{}: {e}", path.display()),
            "fix the key or value named above",
        )
    })
}

/// Merges the global and the project config. The project value wins, and a
/// list replaces the global list. In repo mode only `machine` comes from the
/// global file, so a contributor's own settings cannot change a check.
pub fn load(project: &Project, home: &Path) -> Result<Config> {
    let global = read_file(&home.join(CONFIG_FILE))?;
    let local = read_file(&project.root.join(CONFIG_FILE))?;
    if local.machine.is_some() {
        return Err(Error::new(
            "config",
            "machine belongs in the global config only",
            format!("move it to {}", home.join(CONFIG_FILE).display()),
        ));
    }
    let machine = global.machine.clone();
    let global = match project.meta.mode {
        Mode::Home => global,
        Mode::Repo => File::default(),
    };
    macro_rules! pick {
        ($f:ident) => {
            local.$f.clone().or(global.$f.clone())
        };
    }
    let timeout = pick!(timeout).unwrap_or_else(|| "10m".into());
    let timeout_secs = parse_duration(&timeout)
        .ok_or_else(|| {
            Error::new(
                "config",
                format!("timeout {timeout:?} is not a duration"),
                "write a value such as 90s, 20m or 1h",
            )
        })?
        .as_secs();
    Ok(Config {
        base: pick!(base).unwrap_or_else(|| "main".into()),
        timeout_secs,
        inputs: pick!(inputs).unwrap_or_default(),
        test_globs: pick!(test_globs).unwrap_or_else(|| {
            strings(&[
                "tests/**",
                "**/*_test.*",
                "**/test_*.py",
                "**/*.test.*",
                "**/*.spec.*",
            ])
        }),
        protect_infra: pick!(protect_infra).unwrap_or_else(|| {
            strings(&[
                "**/__snapshots__/**",
                "**/*.snap",
                "**/conftest.py",
                "**/pytest.ini",
                "**/jest.config.*",
                "**/vitest.config.*",
                ".cargo/config.toml",
                "Makefile",
                "justfile",
                "package.json",
            ])
        }),
        checks: pick!(checks).unwrap_or_default(),
        setup: pick!(setup).unwrap_or_default(),
        rerun_env: pick!(rerun_env).unwrap_or_default(),
        env: pick!(env).unwrap_or_default(),
        env_vars: pick!(env_vars).unwrap_or_default(),
        machine,
    })
}

impl Config {
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }

    /// The canonical form that the vow hashes.
    pub fn canonical(&self) -> String {
        serde_json::to_string(self).expect("Config serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;
    use crate::store::init;

    #[test]
    fn project_wins_and_lists_replace() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &home, Mode::Home).unwrap();
        std::fs::write(
            home.join(CONFIG_FILE),
            "base = \"trunk\"\ninputs = [\"a\", \"b\"]\nmachine = \"laptop\"\n",
        )
        .unwrap();
        std::fs::write(
            p.root.join(CONFIG_FILE),
            "inputs = [\"c\"]\ntimeout = \"90s\"\n",
        )
        .unwrap();
        let c = load(&p, &home).unwrap();
        assert_eq!(c.base, "trunk");
        assert_eq!(c.inputs, ["c"]);
        assert_eq!(c.timeout_secs, 90);
        assert_eq!(c.machine.as_deref(), Some("laptop"));
        assert!(!c.canonical().contains("laptop"));
    }

    #[test]
    fn repo_mode_ignores_global_checks() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join(CONFIG_FILE),
            "base = \"trunk\"\nmachine = \"m\"\n",
        )
        .unwrap();
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &home, Mode::Repo).unwrap();
        let c = load(&p, &home).unwrap();
        assert_eq!(c.base, "main");
        assert_eq!(c.machine.as_deref(), Some("m"));
    }

    #[test]
    fn bad_config_is_an_error() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &home, Mode::Home).unwrap();
        std::fs::write(p.root.join(CONFIG_FILE), "unknown = 1\n").unwrap();
        assert_eq!(load(&p, &home).unwrap_err().code, "config");
        std::fs::write(p.root.join(CONFIG_FILE), "machine = \"x\"\n").unwrap();
        assert_eq!(load(&p, &home).unwrap_err().code, "config");
        std::fs::write(p.root.join(CONFIG_FILE), "timeout = \"soon\"\n").unwrap();
        assert_eq!(load(&p, &home).unwrap_err().code, "config");
    }
}
