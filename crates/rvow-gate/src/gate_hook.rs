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
    if !rest.is_empty() || !path.is_absolute() || !executable(&path) {
        return false;
    }
    // The script rvow writes, and the same script without the redirect that
    // hands git's ref lines to `gate --push`: both are rvow's own text.
    install::pre_push_script(&path).is_ok_and(|expected| expected == text || expected.replace(install::PRE_PUSH_REFS_REDIRECT, "") == text)
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

/// The gate arguments an unconditional final command may run: the form `rvow
/// install` writes now, that form with git's ref lines redirected in, and the
/// form an older install wrote, so a hook rvow has not refreshed yet still
/// counts as a gate.
const GATE_ARGS: [&str; 3] = ["gate --push", "gate --push < \"$refs\"", "gate --strict --rerun"];

/// The same for the line `rvow install` prints for another hook manager, which
/// has to leave the hook when the gate fails.
const GATE_LINE_ARGS: [&str; 2] = ["gate --push || exit 1", "gate --strict --rerun || exit 1"];

/// Whether `command` runs the trusted rvow binary with one of `want` as its arguments.
fn gate_command(top: &Path, command: &str, want: &[&str]) -> bool {
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
    want.contains(&args) && binary(&word).is_some_and(|bin| trusted(top, &bin))
}

fn manager(top: &Path, text: &str) -> bool {
    if !shell_script(text) {
        return false;
    }
    let lines: Vec<_> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    // The line `rvow install` prints, as the first command: nothing runs before it.
    if lines.first().is_some_and(|first| gate_command(top, first, &GATE_LINE_ARGS)) {
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
    gate_command(top, last.strip_prefix("exec ").unwrap_or(last), &GATE_ARGS)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::{TempDir, repo_with_commit};

    fn executable(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// A repository whose gate `rvow install` wrote, and the binary it recorded.
    fn installed() -> (TempDir, PathBuf, PathBuf) {
        let tmp = TempDir::new();
        let top = repo_with_commit(&tmp.0, "r");
        let bin = tmp.0.join("bin/rvow");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, "#!/bin/sh\nexit 0\n").unwrap();
        executable(&bin);
        crate::install::pre_push(&top, &bin).unwrap();
        (tmp, top, bin)
    }

    #[test]
    fn pre_push_hook_accepts_old_and_new_gate_forms() {
        assert_eq!(GATE_ARGS[1], format!("gate --push{}", crate::install::PRE_PUSH_REFS_REDIRECT), "the refs redirect moved");
        let (_tmp, top, bin) = installed();
        let hook = top.join(".git/hooks/pre-push");
        let written = std::fs::read_to_string(&hook).unwrap();
        assert!(managed(&written), "the script rvow writes is not a gate: {written}");
        assert!(failure(&top).is_none());
        let without = written.replace(crate::install::PRE_PUSH_REFS_REDIRECT, "");
        assert_ne!(without, written);
        std::fs::write(&hook, &without).unwrap();
        assert!(managed(&without), "the script without the refs redirect is not a gate: {without}");
        assert!(failure(&top).is_none());

        let line = crate::install::shell_quote(&bin.display().to_string());
        for text in [
            format!("#!/bin/sh\n{line} gate --strict --rerun\n"),
            format!("#!/bin/sh\n{line} gate --strict --rerun || exit 1\n"),
            format!("#!/bin/sh\n{line} gate --push\n"),
            format!("#!/bin/sh\n{line} gate --push < \"$refs\"\n"),
            format!("#!/bin/sh\n{line} gate --push || exit 1\n"),
        ] {
            std::fs::write(&hook, &text).unwrap();
            executable(&hook);
            assert!(manager(&top, &text), "{text}");
            assert!(failure(&top).is_none(), "{text}");
        }

        // A binary in the repository that is not the one install recorded.
        let elsewhere = top.join("rvow");
        std::fs::write(&elsewhere, "#!/bin/sh\nexit 3\n").unwrap();
        executable(&elsewhere);
        let other = crate::install::shell_quote(&elsewhere.display().to_string());
        for text in [
            format!("#!/bin/sh\nif [ -n \"$1\" ]; then\n  {line} gate --push\nfi\n"),
            format!("#!/bin/sh\necho hi\n{line} gate --push\n"),
            format!("#!/bin/sh\nexit 0\n{line} gate --push\n"),
            format!("#!/bin/sh\n# {line} gate --push\n"),
            format!("#!/bin/sh\n{other} gate --push\n"),
            format!("#!/bin/sh\n{line} gate --push --force\n"),
        ] {
            std::fs::write(&hook, &text).unwrap();
            executable(&hook);
            assert!(!manager(&top, &text), "not a gate: {text}");
            assert!(failure(&top).is_some(), "not a gate: {text}");
        }
    }
}
