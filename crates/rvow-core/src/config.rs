//! Settings from `$RVOW_HOME/config.toml` and the project's `config.toml`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::store::{CONFIG_FILE, Mode, Project};
use crate::tasks::parse_duration;

/// How long a rerun pass answers for a task: 168 hours.
pub const DEFAULT_CACHE_AGE: Duration = Duration::from_secs(168 * 3600);

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

/// One `setup` or `checks` entry: a plain command that always runs, or a
/// command with `files` globs that runs only when a rerun task or a changed
/// file of the branch matches one of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Step {
    Plain(String),
    Scoped { run: String, files: Vec<String> },
}

impl Step {
    pub fn run(&self) -> &str {
        match self {
            Step::Plain(run) | Step::Scoped { run, .. } => run,
        }
    }

    pub fn files(&self) -> Option<&[String]> {
        match self {
            Step::Plain(_) => None,
            Step::Scoped { files, .. } => Some(files),
        }
    }
}

/// One config file as written. Every key is optional.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct File {
    pub base: Option<String>,
    pub timeout: Option<String>,
    pub inputs: Option<Vec<String>>,
    pub test_globs: Option<Vec<String>>,
    pub protect_infra: Option<Vec<String>>,
    pub checks: Option<Vec<Step>>,
    pub setup: Option<Vec<Step>>,
    pub rerun_env: Option<BTreeMap<String, String>>,
    /// Ignored repo directories that the rerun worktree links in.
    pub rerun_links: Option<Vec<String>>,
    /// Executables whose content is part of a rerun cache key.
    pub tools: Option<Vec<String>>,
    /// How long a cached rerun pass stays valid, such as "24h", or "off".
    pub rerun_cache: Option<String>,
    /// Keep the rerun cache on when `rerun_links` is set.
    pub cache_with_links: Option<bool>,
    /// Rerun a task in a clean checkout even when its proof is current and its files are committed.
    pub rerun_proven: Option<bool>,
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
    /// The `[runners.<name>]` templates a task verify can name with `@<name>`.
    pub runners: Option<BTreeMap<String, crate::template::Runner>>,
    /// Per language, a repo-relative path to a tree-sitter query file that
    /// marks test units and names them, added to the shipped query.
    pub test_queries: Option<BTreeMap<String, String>>,
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
    pub checks: Vec<Step>,
    pub setup: Vec<Step>,
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
    /// Per language, a repo-relative path to a tree-sitter query file that
    /// marks test units and names them, added to the shipped query.
    pub test_queries: BTreeMap<String, String>,
    /// The keys that the user's config files set; the rest are defaults of
    /// this rvow version. Outside the canonical form, like `machine`.
    #[serde(skip)]
    pub user_keys: BTreeSet<String>,
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
        test_globs: pick!(test_globs).unwrap_or_else(|| {
            strings(&[
                "tests/**",
                "**/tests/**",
                "**/__tests__/**",
                "**/spec/**",
                "**/src/test/**",
                "**/*_test.*",
                "**/test_*.py",
                "**/*.test.*",
                "**/*.spec.*",
                "**/*_spec.rb",
                "**/*Test.java",
            ])
        }),
        protect_infra: pick!(protect_infra).unwrap_or_else(|| {
            strings(&[
                "**/__snapshots__/**",
                "**/*.snap",
                "**/conftest.py",
                "**/pytest.ini",
                "**/tox.ini",
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
        test_queries: pick!(test_queries).unwrap_or_default(),
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
        user_keys: user_keys(project, home)?,
    })
}

/// Default values of earlier rvow versions, by `Config` key. A baseline from
/// before `config_keys` was recorded may hold one for a key no file set.
const EARLIER_DEFAULTS: &[(&str, &[&str])] = &[
    ("test_globs", &["tests/**", "**/*_test.*", "**/test_*.py", "**/*.test.*", "**/*.spec.*"]),
    (
        "protect_infra",
        &[
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
        ],
    ),
];

/// Whether `value` is the default of `key` in an earlier rvow version.
pub(crate) fn earlier_default(key: &str, value: &serde_json::Value) -> bool {
    EARLIER_DEFAULTS.iter().any(|(k, list)| *k == key && *value == serde_json::json!(list))
}

/// The keys of `Config` that the user's config files set, as `load` merges
/// them. A key missing here comes from a default of this rvow version.
fn user_keys(project: &Project, home: &Path) -> Result<BTreeSet<String>> {
    let mut files = vec![read_file(&project.root.join(CONFIG_FILE))?];
    if project.meta.mode == Mode::Home {
        files.push(read_file(&home.join(CONFIG_FILE))?);
    }
    let mut keys = BTreeSet::new();
    for file in files {
        let value = serde_json::to_value(&file).map_err(|e| Error::internal(format!("config file does not serialize: {e}")))?;
        for (key, v) in value.as_object().into_iter().flatten() {
            if !v.is_null() {
                keys.insert(if key == "timeout" { "timeout_secs".to_string() } else { key.clone() });
            }
        }
    }
    Ok(keys)
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

/// The `[runners]` table of the project config, which a task verify names
/// with `@<runner> key=value`. It stays out of `Config` on purpose: `Config`
/// is what a vow hashes as the effective config, and a runner edit is its own
/// runner drift per task, not a `config changed` item.
pub fn runners(project: &Project) -> Result<BTreeMap<String, crate::template::Runner>> {
    crate::template::resolve(read_file(&project.root.join(CONFIG_FILE))?.runners.unwrap_or_default())
}

/// How the gate rerun uses its cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachePolicy {
    pub off: bool,
    pub max_age: Duration,
    pub with_links: bool,
    /// When true, a proven task reruns too; by default its current proof is trusted.
    pub rerun_proven: bool,
}

/// The `rerun_cache` and `cache_with_links` keys, merged like `tools`. They
/// stay out of `Config` because they tune this machine's cache, not the vow.
pub fn cache_policy(project: &Project, home: &Path) -> Result<CachePolicy> {
    let local = read_file(&project.root.join(CONFIG_FILE))?;
    let global = match project.meta.mode {
        Mode::Home => read_file(&home.join(CONFIG_FILE))?,
        Mode::Repo => File::default(),
    };
    let value = local.rerun_cache.or(global.rerun_cache);
    let (off, max_age) = match value.as_deref() {
        None => (false, DEFAULT_CACHE_AGE),
        Some("off") => (true, DEFAULT_CACHE_AGE),
        Some(text) => {
            let age = parse_duration(text).ok_or_else(|| {
                Error::new(
                    "config",
                    format!("rerun_cache {text:?} is not a duration or \"off\""),
                    "write a value such as 24h, 90m or \"off\"",
                )
            })?;
            (false, age)
        }
    };
    Ok(CachePolicy {
        off,
        max_age,
        with_links: local.cache_with_links.or(global.cache_with_links).unwrap_or(false),
        rerun_proven: local.rerun_proven.or(global.rerun_proven).unwrap_or(false),
    })
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

    /// The default globs protect a nested Rust test folder, the JavaScript,
    /// Ruby and Java layouts and the tox setup, until the project sets its own.
    #[test]
    fn default_globs_cover_common_layouts() {
        let tmp = TempDir::new();
        let home = tmp.0.join("home");
        let repo = repo_with_commit(&tmp.0, "r");
        let p = init(&repo, &home, Mode::Home).unwrap();
        let c = load(&p, &home).unwrap();
        let guarded: Vec<String> = c.test_globs.iter().chain(&c.protect_infra).cloned().collect();
        let set = crate::fingerprint::globset(&guarded).unwrap();
        for f in [
            "crates/x/tests/cli.rs",
            "tests/cli.rs",
            "web/__tests__/a.js",
            "app/models/user_spec.rb",
            "src/test/java/FooTest.java",
            "lib/FooTest.java",
            "tox.ini",
            "a/b/tox.ini",
        ] {
            assert!(set.is_match(f), "{f} is not guarded");
        }
        for f in ["src/a.rs", "app/models/user.rb", "src/main/java/Foo.java"] {
            assert!(!set.is_match(f), "{f} is guarded");
        }
        std::fs::write(p.root.join(CONFIG_FILE), "test_globs = [\"t/**\"]\nprotect_infra = [\"Makefile\"]\n").unwrap();
        let c = load(&p, &home).unwrap();
        let guarded: Vec<String> = c.test_globs.iter().chain(&c.protect_infra).cloned().collect();
        let set = crate::fingerprint::globset(&guarded).unwrap();
        assert!(!set.is_match("crates/x/tests/cli.rs"));
        assert!(!set.is_match("tox.ini"));
        assert!(set.is_match("t/a.rs"));
        assert!(set.is_match("Makefile"));
    }
}
