//! The environment a hook reads, and the hash of it.

/// The environment a hook reads: a change in any of these names a different
/// environment, so a state the engine built for one is not reused for another.
/// `HOME`, `RVOW_HOME`, `XDG_CONFIG_HOME` and `XDG_DATA_HOME` decide where the
/// store is; `CLAUDE_CONFIG_DIR` and `PI_CODING_AGENT_DIR` where the harness
/// files are; `RVOW_AGENT` and the grammar pair what a call means; `PATH` and
/// the git pair which programs a hook runs.
pub const ENV_VARS: &[&str] = &[
    "HOME",
    "PATH",
    "RVOW_HOME",
    "RVOW_AGENT",
    "CLAUDE_CONFIG_DIR",
    "PI_CODING_AGENT_DIR",
    "RVOW_GRAMMAR_URL",
    "RVOW_GRAMMAR_OFFLINE",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "RVOW_GIT",
    "EVAL_RVOW_GIT",
];

/// The hash of the environment `get` reads: one `NAME=value\n` per name in
/// `ENV_VARS` order, with a name that is not set written as `NAME\n`, so an
/// absent variable and an empty one differ.
pub fn env_hash_from(get: impl Fn(&str) -> Option<String>) -> String {
    let mut text = String::new();
    for name in ENV_VARS {
        text.push_str(name);
        if let Some(value) = get(name) {
            text.push('=');
            text.push_str(&value);
        }
        text.push('\n');
    }
    rvow_base::hash::sha256_hex(text.as_bytes())
}

/// The hash of this process's environment.
pub fn env_hash() -> String {
    env_hash_from(|name| std::env::var(name).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lookup of every name, with `home` for `HOME` and `same` for the rest.
    fn under(home: &'static str) -> impl Fn(&str) -> Option<String> {
        move |name| Some(if name == "HOME" { home.to_string() } else { "same".to_string() })
    }

    #[test]
    fn engine_context_env_hash_differs_by_home() {
        let a = env_hash_from(under("/a"));
        assert_eq!(a.len(), 64, "sha256 hex");
        assert_eq!(a, env_hash_from(under("/a")), "the same environment hashes the same");
        assert_ne!(a, env_hash_from(under("/b")), "another HOME is another environment");
    }

    #[test]
    fn engine_context_env_hash_absent_and_empty_differ() {
        let absent = env_hash_from(|_| None);
        let empty = env_hash_from(|_| Some(String::new()));
        assert_ne!(absent, empty, "an absent variable is not an empty one");
        let base = env_hash_from(|_| Some("v".to_string()));
        for name in ENV_VARS {
            let other = env_hash_from(|n| Some(if n == *name { "w".to_string() } else { "v".to_string() }));
            assert_ne!(base, other, "{name} is not in the hash");
        }
    }
}
