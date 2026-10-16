use std::path::{Path, PathBuf};

use crate::vow::sha256_hex;
use crate::{git, install};

fn executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// The binary a hook line names. A relative path, or a name found in a
/// relative `PATH` entry, is refused: it moves with the working directory and
/// the repository can supply it.
fn binary(word: &str) -> Option<PathBuf> {
    let path = Path::new(word);
    if path.file_name()?.to_str()? != "rvow" {
        return None;
    }
    if word.contains('/') {
        return (path.is_absolute() && executable(path)).then(|| path.to_path_buf());
    }
    std::env::var_os("PATH")
        .and_then(|paths| std::env::split_paths(&paths).filter(|d| d.is_absolute()).map(|dir| dir.join(word)).find(|p| executable(p)))
}

/// The binary hash that `rvow install` recorded, from the record file or the
/// managed default hook.
fn recorded_hash(top: &Path) -> Option<String> {
    let common = git::run(top, &["rev-parse", "--git-common-dir"]).ok()?;
    let hooks = top.join(common).join("hooks");
    if let Ok(record) = std::fs::read_to_string(hooks.join(install::PRE_PUSH_RECORD))
        && let Some(hash) = record.lines().nth(1).and_then(|l| l.strip_prefix("sha256:"))
    {
        return Some(hash.trim().to_string());
    }
    let script = std::fs::read_to_string(hooks.join("pre-push")).ok()?;
    script.lines().find_map(|l| l.strip_prefix("want=")).map(|h| h.trim().to_string())
}

/// Whether `bin` is the binary that `rvow install` recorded. Without a record
/// a binary inside the repository is refused, since the repository supplies it.
fn trusted(top: &Path, bin: &Path) -> bool {
    match recorded_hash(top) {
        Some(want) => std::fs::read(bin).is_ok_and(|bytes| sha256_hex(&bytes) == want),
        None => !bin.canonicalize().is_ok_and(|b| top.canonicalize().is_ok_and(|t| b.starts_with(t))),
    }
}

fn managed(text: &str) -> bool {
    let Some((word, rest)) = text.lines().find_map(|l| l.strip_prefix("rvow=").and_then(install::shell_unquote_prefix)) else {
        return false;
    };
    let path = PathBuf::from(&word);
    rest.is_empty() && path.is_absolute() && executable(&path) && install::pre_push_script(&path).is_ok_and(|expected| expected == text)
}

/// Whether the hook runs as a shell script; another interpreter can skip it.
fn shell_script(text: &str) -> bool {
    let Some(shebang) = text.lines().next().and_then(|l| l.strip_prefix("#!")) else {
        return true;
    };
    let mut words = shebang.split_whitespace();
    let program = match words.next() {
        Some("/usr/bin/env") => words.next(),
        first => first
            .and_then(|p| p.rsplit_once('/'))
            .and_then(|(dir, name)| matches!(dir, "/bin" | "/usr/bin" | "/usr/local/bin" | "/opt/homebrew/bin").then_some(name)),
    };
    words.next().is_none() && program.is_some_and(|p| matches!(p, "sh" | "bash" | "dash" | "zsh" | "ksh"))
}

/// Whether `command` runs the trusted rvow binary with exactly `want` as its arguments.
fn gate_command(top: &Path, command: &str, want: &str) -> bool {
    let (word, args) = if let Some((word, args)) = install::shell_unquote_prefix(command) {
        // Text glued to the closing quote is part of the program name.
        if !args.starts_with([' ', '\t']) {
            return false;
        }
        (word, args.trim())
    } else {
        let Some((word, args)) = command.split_once(' ') else {
            return false;
        };
        (word.to_string(), args.trim())
    };
    args == want && binary(&word).is_some_and(|bin| trusted(top, &bin))
}

fn manager(top: &Path, text: &str) -> bool {
    if !shell_script(text) {
        return false;
    }
    let lines: Vec<_> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    // The line `rvow install` prints, as the first command: nothing runs before it.
    if lines.first().is_some_and(|first| gate_command(top, first, "gate --strict --rerun || exit 1")) {
        return true;
    }
    let Some((last, prior)) = lines.split_last() else {
        return false;
    };
    // Accept only an unconditional final command, not arbitrary shell control flow.
    if prior.iter().any(|l| {
        l.contains([';', '&', '|', '<', '>', '$', '`', '{', '}', '(', ')', '\\', '\'', '"'])
            || !l.split_whitespace().next().is_some_and(|c| matches!(c, "npx" | "npm" | "pnpm" | "yarn"))
    }) {
        return false;
    }
    gate_command(top, last.strip_prefix("exec ").unwrap_or(last), "gate --strict --rerun")
}

pub(super) fn failure(top: &Path) -> Option<String> {
    let configured = git::run(top, &["config", "--get", "core.hooksPath"]).ok().filter(|s| !s.is_empty());
    let common = git::run(top, &["rev-parse", "--git-common-dir"]).ok()?;
    let default = top.join(common).join("hooks/pre-push");
    let hook = configured.as_ref().map_or_else(|| default.clone(), |dir| top.join(dir).join("pre-push"));
    let text = std::fs::read_to_string(&hook).ok();
    let installed_default = default.parent().is_some_and(|dir| dir.join(install::PRE_PUSH_RECORD).exists())
        || std::fs::read_to_string(&default).is_ok_and(|t| t.contains("rvow pre-push gate"));
    if configured.is_none() && !installed_default && !text.as_ref().is_some_and(|t| t.contains("rvow")) {
        return None;
    }
    if executable(&hook) && text.as_ref().is_some_and(|t| managed(t) || manager(top, t)) {
        return None;
    }
    Some(match configured {
        Some(dir) => format!(
            "core.hooksPath {dir} pre-push hook {} is missing, non-executable or does not run an intact rvow gate; run rvow install and add the line it prints",
            hook.display()
        ),
        None => format!("installed pre-push hook {} is missing or was changed; run rvow install again", hook.display()),
    })
}
