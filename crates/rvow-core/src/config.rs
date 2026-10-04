//! Settings from `$RVOW_HOME/config.toml` and the project's `config.toml`.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::store::{CONFIG_FILE, Mode, Project};
use crate::tasks::parse_duration;

/// Maximum UTF-8 bytes in project context or one artifact's rules.
pub const AUTHORING_BYTES: usize = 2000;

/// Artifact-specific authoring rules, merged by artifact across config files.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Rules {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tasks: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub design: Option<String>,
}

impl Rules {
    fn is_empty(&self) -> bool {
        self.proposal.is_none() && self.spec.is_none() && self.tasks.is_none() && self.design.is_none()
    }
}

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
    /// Ignored repo directories that the rerun worktree links in.
    pub rerun_links: Option<Vec<String>>,
    /// Executables whose content is part of a rerun cache key.
    pub tools: Option<Vec<String>>,
    pub env: Option<Vec<String>>,
    pub env_vars: Option<Vec<String>>,
    /// Global only: a name for this computer.
    pub machine: Option<String>,
    /// When true, a confirm event only decides an amendment when its ssh
    /// signature checks against `allowed_signers`.
    pub require_signature: Option<bool>,
    /// The allowed-signers file that signed confirmations are checked against.
    pub allowed_signers: Option<String>,
    /// The most questions a clarify round asks.
    pub interview_cap: Option<u64>,
    /// Global only: the ssh signing key for `rvow confirm --sign`.
    pub signing_key: Option<String>,
    /// Global only: a name for the agent that runs proofs.
    pub agent: Option<String>,
    pub context: Option<String>,
    pub rules: Option<Rules>,
}

/// The settings in force. Every field except `machine` is part of the vow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// When true, `rvow gate` refuses confirm events without a valid signature.
    pub require_signature: bool,
    /// The path of the allowed-signers file for signed confirmations.
    pub allowed_signers: String,
    /// The most questions a clarify round asks.
    pub interview_cap: u64,
    /// The ssh signing key for `rvow confirm --sign`.
    #[serde(skip)]
    pub signing_key: Option<String>,
    /// A name for the agent that runs proofs.
    #[serde(skip)]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub context: String,
    #[serde(default, skip_serializing_if = "Rules::is_empty")]
    pub rules: Rules,
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
    toml::from_str(&text).map_err(|e| Error::new("config", format!("{}: {e}", path.display()), "fix the key or value named above"))
}

/// Merges the global and the project config. The project value wins, and a
/// list replaces the global list. In repo mode only `machine` comes from the
/// global file, so a contributor's own settings cannot change a check.
pub fn load(project: &Project, home: &Path) -> Result<Config> {
    let global = read_file(&home.join(CONFIG_FILE))?;
    let local = read_file(&project.root.join(CONFIG_FILE))?;
    let global_only =
        [("machine", local.machine.is_some()), ("signing_key", local.signing_key.is_some()), ("agent", local.agent.is_some())];
    for (key, set) in &global_only {
        if *set {
            return Err(Error::new(
                "config",
                format!("{key} belongs in the global config only"),
                format!("move it to {}", home.join(CONFIG_FILE).display()),
            ));
        }
    }
    let (machine, signing_key, agent) = (global.machine.clone(), global.signing_key.clone(), global.agent.clone());
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
        .ok_or_else(|| Error::new("config", format!("timeout {timeout:?} is not a duration"), "write a value such as 90s, 20m or 1h"))?
        .as_secs();
    let interview_cap = pick!(interview_cap).unwrap_or(3);
    if !(1..=3).contains(&interview_cap) {
        return Err(Error::new(
            "config",
            format!("interview_cap {interview_cap} is outside 1..=3 in {}", project.root.join(CONFIG_FILE).display()),
            "set interview_cap to 1, 2 or 3 in the interview_cap key",
        ));
    }
    let context = pick!(context).unwrap_or_default();
    let local_rules = local.rules.clone().unwrap_or_default();
    let global_rules = global.rules.clone().unwrap_or_default();
    let rules = Rules {
        proposal: local_rules.proposal.or(global_rules.proposal),
        spec: local_rules.spec.or(global_rules.spec),
        tasks: local_rules.tasks.or(global_rules.tasks),
        design: local_rules.design.or(global_rules.design),
    };
    for (name, text) in [
        ("context", Some(context.as_str())),
        ("rules.proposal", rules.proposal.as_deref()),
        ("rules.spec", rules.spec.as_deref()),
        ("rules.tasks", rules.tasks.as_deref()),
        ("rules.design", rules.design.as_deref()),
    ] {
        if text.is_some_and(|text| text.len() > AUTHORING_BYTES) {
            return Err(Error::new(
                "config",
                format!("{name} exceeds {AUTHORING_BYTES} UTF-8 bytes in {}", project.root.join(CONFIG_FILE).display()),
                format!("shorten {name} to at most {AUTHORING_BYTES} UTF-8 bytes"),
            ));
        }
    }
    Ok(Config {
        base: pick!(base).unwrap_or_else(|| "main".into()),
        timeout_secs,
        inputs: pick!(inputs).unwrap_or_default(),
        test_globs: pick!(test_globs)
            .unwrap_or_else(|| strings(&["tests/**", "**/*_test.*", "**/test_*.py", "**/*.test.*", "**/*.spec.*"])),
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
        require_signature: pick!(require_signature).unwrap_or(false),
        allowed_signers: pick!(allowed_signers).unwrap_or_default(),
        interview_cap,
        signing_key,
        agent,
        context,
        rules,
    })
}

/// The `rerun_links` key, merged like the keys of `load`. It stays out of
/// `Config` because it names the machine's own build output, not the vow.
pub fn rerun_links(project: &Project, home: &Path) -> Result<Vec<String>> {
    let local = read_file(&project.root.join(CONFIG_FILE))?;
    let global = match project.meta.mode {
        Mode::Home => read_file(&home.join(CONFIG_FILE))?,
        Mode::Repo => File::default(),
    };
    Ok(local.rerun_links.or(global.rerun_links).unwrap_or_default())
}

/// The `tools` key, merged like `rerun_links` and kept out of `Config` for the
/// same reason: it names this machine's binaries, not the vow.
pub fn tools(project: &Project, home: &Path) -> Result<Vec<String>> {
    let local = read_file(&project.root.join(CONFIG_FILE))?;
    let global = match project.meta.mode {
        Mode::Home => read_file(&home.join(CONFIG_FILE))?,
        Mode::Repo => File::default(),
    };
    Ok(local.tools.or(global.tools).unwrap_or_default())
}

impl Config {
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }

    /// The canonical form that the vow hashes.
    pub fn canonical(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|e| Error::internal(format!("config does not serialize: {e}")))
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
        std::fs::write(home.join(CONFIG_FILE), "base = \"trunk\"\ninputs = [\"a\", \"b\"]\nmachine = \"laptop\"\n").unwrap();
        std::fs::write(p.root.join(CONFIG_FILE), "inputs = [\"c\"]\ntimeout = \"90s\"\n").unwrap();
        let c = load(&p, &home).unwrap();
        assert_eq!(c.base, "trunk");
        assert_eq!(c.inputs, ["c"]);
        assert_eq!(c.timeout_secs, 90);
        assert_eq!(c.machine.as_deref(), Some("laptop"));
        assert!(!c.canonical().unwrap().contains("laptop"));
    }

    #[test]
    fn repo_mode_ignores_global_checks() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join(CONFIG_FILE), "base = \"trunk\"\nmachine = \"m\"\n").unwrap();
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

    #[test]
    fn promised_keys_are_read_and_global_only_are_enforced() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &home, Mode::Home).unwrap();
        std::fs::write(
            home.join(CONFIG_FILE),
            "interview_cap = 2\nmachine = \"laptop\"\nsigning_key = \"/home/u/.ssh/rvow\"\nagent = \"pi\"\n",
        )
        .unwrap();
        let c = load(&p, &home).unwrap();
        assert_eq!(c.interview_cap, 2);
        assert_eq!(c.machine.as_deref(), Some("laptop"));
        assert_eq!(c.signing_key.as_deref(), Some("/home/u/.ssh/rvow"));
        assert_eq!(c.agent.as_deref(), Some("pi"));
        let canon = c.canonical().unwrap();
        assert!(canon.contains(&format!("\"interview_cap\":{}", 2)), "{canon}");
        assert!(!canon.contains("laptop") && !canon.contains("/home/u") && !canon.contains("pi"), "{canon}");

        // The global-only keys are refused in the project config.
        for key in ["machine", "signing_key", "agent"] {
            std::fs::write(p.root.join(CONFIG_FILE), format!("{key} = \"x\"\n")).unwrap();
            assert_eq!(load(&p, &home).unwrap_err().code, "config", "{key}");
        }
    }
}
