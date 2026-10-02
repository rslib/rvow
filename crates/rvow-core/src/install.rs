//! `rvow install`: the skill, the Claude Code hooks, the pi extension and the
//! pre-push gate.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::git;
use crate::vow::sha256_hex;

pub const SKILL: &str = include_str!("../../../skills/rvow/SKILL.md");
pub const PI_EXTENSION: &str = include_str!("../../../extensions/pi/rvow.ts");
const MOD_PLUGIN_JSON: &str = include_str!("../../../extensions/claude/.claude-plugin/plugin.json");
const MOD_HOOKS_JSON: &str = include_str!("../../../extensions/claude/hooks/hooks.json");
const MOD_REGISTER: &str = include_str!("../../../extensions/claude/hooks/register.tsx");
const MOD_TYPES: &str = include_str!("../../../extensions/claude/types/index.d.ts");
pub const MARKETPLACE_DIR: &str = "claude-plugin";
const MARK: &str = "rvow-managed sha256:";
const PRE_PUSH_MARK: &str = "# rvow pre-push gate";
const TOOLS: &str = "Bash|Edit|Write|MultiEdit|NotebookEdit|mcp__.*";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Written(PathBuf),
    Unchanged(PathBuf),
    /// The user edited the file, so rvow leaves it alone.
    KeptEdit(PathBuf),
    /// Another hook manager owns the hook; the user adds this line.
    AddLine {
        path: PathBuf,
        line: String,
    },
}

/// Writes `body` with a marker line holding its hash. An existing file is
/// replaced only when it still matches its own marker, so a user's edits
/// survive an upgrade.
fn write_managed(path: &Path, body: &str, comment: &str) -> Result<Outcome> {
    let marked = format!("{comment} {MARK}{}\n{body}", sha256_hex(body.as_bytes()));
    if let Ok(old) = std::fs::read_to_string(path) {
        if old == marked {
            return Ok(Outcome::Unchanged(path.to_path_buf()));
        }
        let unedited = old.split_once('\n').is_some_and(|(first, rest)| {
            first.split_once(MARK).is_some_and(|(_, h)| {
                h.trim_end_matches(" -->").trim() == sha256_hex(rest.as_bytes())
            })
        });
        if !unedited {
            return Ok(Outcome::KeptEdit(path.to_path_buf()));
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    std::fs::write(path, marked).map_err(|e| Error::io(path, e))?;
    Ok(Outcome::Written(path.to_path_buf()))
}

/// The skill goes to `<claude_dir>/skills/rvow/SKILL.md`. Its front matter must
/// stay first, so the marker follows it as an HTML comment.
pub fn skill(claude_dir: &Path) -> Result<Outcome> {
    let path = claude_dir.join("skills/rvow/SKILL.md");
    let (front, body) = split_front_matter(SKILL);
    let marked_body = format!("<!-- {MARK}{} -->\n{body}", sha256_hex(body.as_bytes()));
    let full = format!("{front}{marked_body}");
    if let Ok(old) = std::fs::read_to_string(&path) {
        if old == full {
            return Ok(Outcome::Unchanged(path));
        }
        let (_, old_body) = split_front_matter(&old);
        let unedited = old_body.split_once('\n').is_some_and(|(first, rest)| {
            first.split_once(MARK).is_some_and(|(_, h)| {
                h.trim_end_matches(" -->").trim() == sha256_hex(rest.as_bytes())
            })
        });
        if !unedited {
            return Ok(Outcome::KeptEdit(path));
        }
    }
    let dir = path.parent().expect("skill path has a parent");
    std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    std::fs::write(&path, full).map_err(|e| Error::io(&path, e))?;
    Ok(Outcome::Written(path))
}

fn split_front_matter(text: &str) -> (&str, &str) {
    if let Some(rest) = text.strip_prefix("---\n")
        && let Some(end) = rest.find("\n---\n")
    {
        let cut = 4 + end + 5;
        return (&text[..cut], &text[cut..]);
    }
    ("", text)
}

/// Writes the rvow Claude Code mod as a local marketplace in `dir`, with the
/// binary path filled in and the skill inside. rvow owns this folder, so every
/// file is rewritten to match.
pub fn claude_mod(dir: &Path, rvow: &Path) -> Result<Outcome> {
    let register = MOD_REGISTER.replace(
        "'__RVOW_BIN__'",
        &format!("{:?}", rvow.display().to_string()),
    );
    let version = format!(
        "{}+{}",
        env!("CARGO_PKG_VERSION"),
        &sha256_hex(format!("{register}{SKILL}{MOD_HOOKS_JSON}{MOD_TYPES}").as_bytes())[..12]
    );
    let mut plugin: Value = serde_json::from_str(MOD_PLUGIN_JSON).expect("plugin.json is JSON");
    plugin["version"] = version.into();
    let marketplace = json!({
        "name": "rvow",
        "owner": {"name": "rvow"},
        "metadata": {"description": "The rvow plugin, written by `rvow install`"},
        "plugins": [{
            "name": "rvow",
            "source": "./plugins/rvow",
            "description": plugin["description"].clone(),
        }],
    });
    let files = [
        (
            ".claude-plugin/marketplace.json",
            serde_json::to_string_pretty(&marketplace).expect("json") + "\n",
        ),
        (
            "plugins/rvow/.claude-plugin/plugin.json",
            serde_json::to_string_pretty(&plugin).expect("json") + "\n",
        ),
        ("plugins/rvow/hooks/hooks.json", MOD_HOOKS_JSON.to_string()),
        ("plugins/rvow/hooks/register.tsx", register),
        ("plugins/rvow/types/index.d.ts", MOD_TYPES.to_string()),
        ("plugins/rvow/skills/rvow/SKILL.md", SKILL.to_string()),
    ];
    let mut changed = false;
    for (rel, body) in files {
        let path = dir.join(rel);
        if std::fs::read_to_string(&path).is_ok_and(|old| old == body) {
            continue;
        }
        let parent = path.parent().expect("file has a parent");
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        std::fs::write(&path, body).map_err(|e| Error::io(&path, e))?;
        changed = true;
    }
    Ok(if changed {
        Outcome::Written(dir.to_path_buf())
    } else {
        Outcome::Unchanged(dir.to_path_buf())
    })
}

/// Removes the rvow entries that the settings-hook install wrote, so the mod
/// and the settings hooks never both answer. Other hooks stay.
pub fn remove_claude_settings(path: &Path) -> Result<Outcome> {
    let Ok(old) = std::fs::read_to_string(path) else {
        return Ok(Outcome::Unchanged(path.to_path_buf()));
    };
    let Ok(mut settings) = serde_json::from_str::<Value>(&old) else {
        return Ok(Outcome::Unchanged(path.to_path_buf()));
    };
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(Outcome::Unchanged(path.to_path_buf()));
    };
    for list in hooks.values_mut().filter_map(Value::as_array_mut) {
        list.retain(|x| !is_rvow_entry(x));
    }
    let text = serde_json::to_string_pretty(&settings).expect("settings serialize") + "\n";
    if text == old {
        return Ok(Outcome::Unchanged(path.to_path_buf()));
    }
    let backup = path.with_extension("json.rvow-backup");
    if !backup.exists() {
        std::fs::write(&backup, &old).map_err(|e| Error::io(&backup, e))?;
    }
    std::fs::write(path, text).map_err(|e| Error::io(path, e))?;
    Ok(Outcome::Written(path.to_path_buf()))
}

pub fn pi_extension(pi_dir: &Path) -> Result<Outcome> {
    write_managed(&pi_dir.join("extensions/rvow.ts"), PI_EXTENSION, "//")
}

fn is_rvow_entry(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hs| {
            hs.iter().any(|h| {
                h.get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|c| c.contains("hook claude"))
            })
        })
}

/// Merges the rvow hooks into a Claude Code settings file, replacing earlier
/// rvow entries and keeping everything else in place. The first write keeps a
/// copy of the original next to it.
pub fn claude_settings(path: &Path, rvow: &Path) -> Result<Outcome> {
    let old = match std::fs::read_to_string(path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(Error::io(path, e)),
    };
    let mut settings: Value = match &old {
        Some(t) if !t.trim().is_empty() => serde_json::from_str(t).map_err(|e| {
            Error::new(
                "settings",
                format!("{}: {e}", path.display()),
                "fix the JSON, then run rvow install again",
            )
        })?,
        _ => json!({}),
    };
    let bin = rvow.display().to_string();
    let entry = |matcher: Option<&str>, event: &str| {
        let hook = json!({"type": "command", "command": format!("'{bin}' hook claude {event}")});
        match matcher {
            Some(m) => json!({"matcher": m, "hooks": [hook]}),
            None => json!({"hooks": [hook]}),
        }
    };
    let wanted = [
        ("PreToolUse", entry(Some(TOOLS), "pre-tool-use")),
        ("SessionStart", entry(None, "session-start")),
        ("Stop", entry(None, "stop")),
    ];
    let root = settings
        .as_object_mut()
        .ok_or_else(|| Error::new("settings", "settings is not a JSON object", "fix the file"))?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| Error::new("settings", "\"hooks\" is not an object", "fix the file"))?;
    for (event, e) in wanted {
        let list = hooks.entry(event).or_insert_with(|| json!([]));
        let arr = list.as_array_mut().ok_or_else(|| {
            Error::new(
                "settings",
                format!("hooks.{event} is not a list"),
                "fix the file",
            )
        })?;
        arr.retain(|x| !is_rvow_entry(x));
        arr.push(e);
    }
    let text = serde_json::to_string_pretty(&settings).expect("settings serialize") + "\n";
    if old.as_deref() == Some(text.as_str()) {
        return Ok(Outcome::Unchanged(path.to_path_buf()));
    }
    if let Some(o) = &old {
        let backup = path.with_extension("json.rvow-backup");
        if !backup.exists() {
            std::fs::write(&backup, o).map_err(|e| Error::io(&backup, e))?;
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    std::fs::write(path, text).map_err(|e| Error::io(path, e))?;
    Ok(Outcome::Written(path.to_path_buf()))
}

/// The pre-push hook script. It refuses to run an rvow binary that changed
/// since install, so a binary the agent rebuilt cannot pass the gate.
pub fn pre_push_script(rvow: &Path) -> Result<String> {
    let bytes = std::fs::read(rvow).map_err(|e| Error::io(rvow, e))?;
    let bin = rvow.display();
    Ok(format!(
        "#!/bin/sh\n{PRE_PUSH_MARK}, written by rvow install\nrvow='{bin}'\nwant={}\nhave=$(shasum -a 256 \"$rvow\" | cut -d' ' -f1)\nif [ \"$have\" != \"$want\" ]; then\n  echo \"rvow: $rvow changed since rvow install; run rvow install again\" >&2\n  exit 1\nfi\nexec env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE \"$rvow\" gate --strict --rerun\n",
        sha256_hex(&bytes)
    ))
}

/// Writes `.git/hooks/pre-push` when no other hook manager owns it.
pub fn pre_push(top: &Path, rvow: &Path) -> Result<Outcome> {
    let script = pre_push_script(rvow)?;
    if let Ok(dir) = git::run(top, &["config", "--get", "core.hooksPath"])
        && !dir.is_empty()
    {
        return Ok(Outcome::AddLine {
            path: top.join(&dir).join("pre-push"),
            line: format!("'{}' gate --strict --rerun", rvow.display()),
        });
    }
    let common = git::run(top, &["rev-parse", "--git-common-dir"])?;
    let hooks = top.join(common).join("hooks");
    let path = hooks.join("pre-push");
    match std::fs::read_to_string(&path) {
        Ok(old) if old == script => return Ok(Outcome::Unchanged(path)),
        Ok(old) if !old.contains(PRE_PUSH_MARK) => {
            return Ok(Outcome::AddLine {
                path,
                line: format!("'{}' gate --strict --rerun", rvow.display()),
            });
        }
        _ => {}
    }
    std::fs::create_dir_all(&hooks).map_err(|e| Error::io(&hooks, e))?;
    std::fs::write(&path, script).map_err(|e| Error::io(&path, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| Error::io(&path, e))?;
    }
    Ok(Outcome::Written(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;

    #[test]
    fn managed_files_upgrade_unless_edited() {
        let tmp = TempDir::new();
        let path = tmp.0.join("ext/rvow.ts");
        assert!(matches!(
            write_managed(&path, "a\n", "//").unwrap(),
            Outcome::Written(_)
        ));
        assert!(matches!(
            write_managed(&path, "a\n", "//").unwrap(),
            Outcome::Unchanged(_)
        ));
        assert!(matches!(
            write_managed(&path, "b\n", "//").unwrap(),
            Outcome::Written(_)
        ));
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text + "// mine\n").unwrap();
        assert!(matches!(
            write_managed(&path, "c\n", "//").unwrap(),
            Outcome::KeptEdit(_)
        ));
    }

    #[test]
    fn skill_keeps_front_matter_first() {
        let tmp = TempDir::new();
        assert!(matches!(skill(&tmp.0).unwrap(), Outcome::Written(_)));
        let text = std::fs::read_to_string(tmp.0.join("skills/rvow/SKILL.md")).unwrap();
        assert!(text.starts_with("---\nname: rvow\n"), "{text}");
        assert!(matches!(skill(&tmp.0).unwrap(), Outcome::Unchanged(_)));
        std::fs::write(tmp.0.join("skills/rvow/SKILL.md"), text + "mine\n").unwrap();
        assert!(matches!(skill(&tmp.0).unwrap(), Outcome::KeptEdit(_)));
    }

    #[test]
    fn settings_merge_keeps_other_hooks_and_order() {
        let tmp = TempDir::new();
        let path = tmp.0.join("settings.json");
        let original = r#"{"model": "x", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "other"}]}]}, "zeta": 1}"#;
        std::fs::write(&path, original).unwrap();
        let bin = Path::new("/opt/rvow");
        assert!(matches!(
            claude_settings(&path, bin).unwrap(),
            Outcome::Written(_)
        ));
        assert!(matches!(
            claude_settings(&path, bin).unwrap(),
            Outcome::Unchanged(_)
        ));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["model", "hooks", "zeta"]);
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"][0]["command"], "other");
        assert_eq!(
            stop[1]["hooks"][0]["command"],
            "'/opt/rvow' hook claude stop"
        );
        assert_eq!(v["hooks"]["PreToolUse"][0]["matcher"], TOOLS);
        assert_eq!(
            std::fs::read_to_string(tmp.0.join("settings.json.rvow-backup")).unwrap(),
            original
        );
        claude_settings(&path, Path::new("/new/rvow")).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            v["hooks"]["Stop"].as_array().unwrap().len(),
            2,
            "old rvow entry replaced"
        );
    }

    #[test]
    fn mod_marketplace_fills_the_binary_and_is_stable() {
        let tmp = TempDir::new();
        let dir = tmp.0.join("claude-plugin");
        let bin = Path::new("/opt/bin/rvow");
        assert!(matches!(
            claude_mod(&dir, bin).unwrap(),
            Outcome::Written(_)
        ));
        assert!(matches!(
            claude_mod(&dir, bin).unwrap(),
            Outcome::Unchanged(_)
        ));
        let reg = std::fs::read_to_string(dir.join("plugins/rvow/hooks/register.tsx")).unwrap();
        assert!(
            reg.contains(r#"const INSTALLED_BIN = "/opt/bin/rvow""#),
            "{reg}"
        );
        assert!(dir.join("plugins/rvow/skills/rvow/SKILL.md").is_file());
        let m: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join(".claude-plugin/marketplace.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(m["plugins"][0]["source"], "./plugins/rvow");
        assert!(matches!(
            claude_mod(&dir, Path::new("/new/rvow")).unwrap(),
            Outcome::Written(_)
        ));
    }

    #[test]
    fn switching_to_the_mod_removes_settings_hooks() {
        let tmp = TempDir::new();
        let path = tmp.0.join("settings.json");
        std::fs::write(
            &path,
            r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "other"}]}]}}"#,
        )
        .unwrap();
        claude_settings(&path, Path::new("/opt/rvow")).unwrap();
        assert!(matches!(
            remove_claude_settings(&path).unwrap(),
            Outcome::Written(_)
        ));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(v["hooks"]["PreToolUse"].as_array().unwrap().len(), 0);
        assert!(matches!(
            remove_claude_settings(&path).unwrap(),
            Outcome::Unchanged(_)
        ));
    }

    #[test]
    fn pre_push_respects_other_hook_managers() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let bin = tmp.0.join("rvow-bin");
        std::fs::write(&bin, "binary").unwrap();
        assert!(matches!(
            pre_push(&repo, &bin).unwrap(),
            Outcome::Written(_)
        ));
        assert!(matches!(
            pre_push(&repo, &bin).unwrap(),
            Outcome::Unchanged(_)
        ));
        let script = std::fs::read_to_string(repo.join(".git/hooks/pre-push")).unwrap();
        assert!(script.contains(&sha256_hex(b"binary")));

        std::fs::write(repo.join(".git/hooks/pre-push"), "#!/bin/sh\necho mine\n").unwrap();
        assert!(matches!(
            pre_push(&repo, &bin).unwrap(),
            Outcome::AddLine { .. }
        ));
        git(&repo, &["config", "core.hooksPath", ".husky"]);
        assert!(matches!(
            pre_push(&repo, &bin).unwrap(),
            Outcome::AddLine { .. }
        ));
    }
}
