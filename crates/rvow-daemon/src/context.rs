//! The environment a hook reads, and the hash of it, and the build id of a
//! binary: the version plus a stamp of the file it was read from.

use std::path::Path;

/// The version every binary of this workspace reports.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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

/// What a build stamp reads from a file: its modified time and its length, so a
/// binary that was rewritten is another build even when its version is the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    /// Nanoseconds since the epoch at the last change of the file.
    nanos: u128,
    /// The length of the file in bytes.
    len: u64,
}

impl Stamp {
    /// The short hex this stamp stands for: the two values folded into 32 bits,
    /// which no two builds of one binary share in practice.
    fn hex(self) -> String {
        let len = u128::from(self.len).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        format!("{:08x}", (self.nanos ^ len.rotate_left(17)) as u32)
    }
}

/// The stamp of the file at `path`, or `None` when its metadata cannot be read.
pub fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    let nanos = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    Some(Stamp { nanos, len: meta.len() })
}

/// Whether the file at `path` is no longer the one `start` stamped: a rewritten
/// binary, or one that is gone, is another build.
pub fn stamp_changed(path: &Path, start: Option<Stamp>) -> bool {
    stamp(path) != start
}

/// The build id of the binary at `path`: the version, then `+`, then a short
/// hex of the file's stamp. A file that cannot be read has the plain version.
pub fn build_id(path: &Path) -> String {
    match stamp(path) {
        Some(stamp) => format!("{VERSION}+{}", stamp.hex()),
        None => VERSION.to_owned(),
    }
}

/// The build id of the binary this process runs, which `rvow daemon --print-hello`
/// prints and the engine reports, so the two agree on one file.
pub fn build_id_here() -> String {
    match std::env::current_exe() {
        Ok(path) => build_id(&path),
        Err(_) => VERSION.to_owned(),
    }
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

    /// A temp folder holding one file named `rvow` with `text` in it.
    fn binary(text: &[u8]) -> (rvow_git::git::testrepo::TempDir, std::path::PathBuf) {
        let tmp = rvow_git::git::testrepo::TempDir::new();
        let path = tmp.0.join("rvow");
        std::fs::write(&path, text).unwrap();
        (tmp, path)
    }

    #[test]
    fn engine_build_id_follows_the_binary_file() {
        let (_tmp, path) = binary(b"one");
        let id = build_id(&path);
        assert!(id.starts_with(&format!("{VERSION}+")), "the version, then the stamp: {id}");
        assert_eq!(id, build_id(&path), "an untouched file is the same build");
        assert_eq!(stamp(&path), stamp(&path), "a stamp of a file that did not change is the same");

        // Another content, at another time and of another length, is another build.
        std::fs::write(&path, b"one and more of it").unwrap();
        assert_ne!(id, build_id(&path), "a rewritten binary is another build");

        // A file that cannot be read has no stamp, so it reports the plain version.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(build_id(&path), VERSION, "a file that is gone has no stamp");
    }

    #[test]
    fn engine_build_id_here_names_this_executable() {
        let exe = std::env::current_exe().unwrap();
        assert_eq!(build_id_here(), build_id(&exe), "this process is its own executable");
        assert!(build_id_here().starts_with(VERSION), "{}", build_id_here());
    }

    #[test]
    fn engine_build_stamp_changes_after_a_rewrite_and_a_removal() {
        let (_tmp, path) = binary(b"one");
        let start = stamp(&path);
        assert!(start.is_some(), "a file that is there has a stamp");
        assert!(!stamp_changed(&path, start), "nothing changed yet");

        std::fs::write(&path, b"another file entirely").unwrap();
        assert!(stamp_changed(&path, start), "a rewritten file is a changed build");

        let start = stamp(&path);
        std::fs::remove_file(&path).unwrap();
        assert!(stamp_changed(&path, start), "a file that is gone is a changed build");
    }
}
