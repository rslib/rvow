//! `rvow install`: the skill, the Claude Code hooks, the pi extension and the
//! pre-push gate.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::git;
use crate::vow::sha256_hex;

pub const PI_EXTENSION: &str = include_str!("../../../extensions/pi/rvow/index.ts");
const PI_MODULES: [(&str, &str); 8] = [
    ("formats.ts", include_str!("../../../extensions/pi/rvow/formats.ts")),
    ("prompts.ts", include_str!("../../../extensions/pi/rvow/prompts.ts")),
    ("types.ts", include_str!("../../../extensions/pi/rvow/types.ts")),
    ("tools.ts", include_str!("../../../extensions/pi/rvow/tools.ts")),
    ("typed-tools.ts", include_str!("../../../extensions/pi/rvow/typed-tools.ts")),
    ("refresh.ts", include_str!("../../../extensions/pi/rvow/refresh.ts")),
    ("judge.ts", include_str!("../../../extensions/pi/rvow/judge.ts")),
    ("engine.ts", include_str!("../../../extensions/pi/rvow/engine.ts")),
];
const MOD_PLUGIN_JSON: &str = include_str!("../../../extensions/claude/.claude-plugin/plugin.json");
const MOD_HOOKS_JSON: &str = include_str!("../../../extensions/claude/hooks/hooks.json");
const MOD_REGISTER: &str = include_str!("../../../extensions/claude/hooks/register.tsx");
/// Modules `register.tsx` imports; the engine cannot follow `$` or an atom across an import, so these hold pure code.
const MOD_MODULES: [(&str, &str); 3] = [
    ("format.ts", include_str!("../../../extensions/claude/hooks/format.ts")),
    ("accept.tsx", include_str!("../../../extensions/claude/hooks/accept.tsx")),
    ("engine.ts", include_str!("../../../extensions/claude/hooks/engine.ts")),
];
const MOD_TYPES: &str = include_str!("../../../extensions/claude/types/index.d.ts");

pub use rvow_base::layout::MARKETPLACE_DIR;

const MARK: &str = "rvow-managed sha256:";
const PRE_PUSH_MARK: &str = "# rvow pre-push gate";
pub(crate) const PRE_PUSH_RECORD: &str = "rvow-pre-push-installed";
const PRE_PUSH_PREV_SUFFIX: &str = ".rvow-prev";
/// Hands the ref lines git gave the hook to `rvow gate --push` on its stdin.
pub(crate) const PRE_PUSH_REFS_REDIRECT: &str = " < \"$refs\"";
const TOOLS: &str = "Bash|Edit|Write|MultiEdit|NotebookEdit|Read|Grep|Glob|mcp__.*";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Written(PathBuf),
    Unchanged(PathBuf),
    /// The user edited the file, so rvow leaves it alone.
    KeptEdit(PathBuf),
    /// rvow moved the user's hook to `prev` and wrote `path`, which runs it first.
    Chained {
        path: PathBuf,
        prev: PathBuf,
    },
    /// Another hook manager owns the hook; the user adds this line.
    AddLine {
        path: PathBuf,
        line: String,
    },
}

/// The pi agent folder: `PI_CODING_AGENT_DIR` when set (pi honours it), else
/// `<home>/.pi/agent`.
pub fn pi_dir(home: Option<&Path>) -> Option<PathBuf> {
    std::env::var_os("PI_CODING_AGENT_DIR").filter(|d| !d.is_empty()).map(PathBuf::from).or_else(|| home.map(|h| h.join(".pi/agent")))
}

/// The folder of the Claude Code mod marketplace: `<XDG_DATA_HOME>/rvow/claude-plugin`,
/// else `<HOME>/.local/share/rvow/claude-plugin`. It is never under `RVOW_HOME`,
/// which can name a synced plan store.
pub fn mod_dir() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    env("XDG_DATA_HOME").or_else(|| env("HOME").map(|h| h.join(".local/share"))).map(|d| d.join("rvow").join(MARKETPLACE_DIR))
}

/// Removes the rvow marketplace an older install wrote at `old`, when `old` is
/// not `current`. A folder that does not hold the rvow marketplace stays.
pub fn remove_old_mod(old: &Path, current: &Path) -> Result<Option<PathBuf>> {
    if old == current {
        return Ok(None);
    }
    let ours = std::fs::read_to_string(old.join(".claude-plugin/marketplace.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .is_some_and(|m| m.get("name").and_then(Value::as_str) == Some("rvow"));
    if !ours {
        return Ok(None);
    }
    std::fs::remove_dir_all(old).map_err(|e| Error::io(old, e))?;
    Ok(Some(old.to_path_buf()))
}

/// `text` as one shell word in single quotes; a single quote inside becomes `'\''`.
pub(crate) fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The first single-quoted shell word of `text` and the rest after it, the
/// inverse of `shell_quote`.
pub(crate) fn shell_unquote_prefix(text: &str) -> Option<(String, &str)> {
    let mut rest = text.strip_prefix('\'')?;
    let mut word = String::new();
    loop {
        let (chunk, after) = rest.split_once('\'')?;
        word.push_str(chunk);
        match after.strip_prefix("\\''") {
            Some(more) => {
                word.push('\'');
                rest = more;
            }
            None => return Some((word, after)),
        }
    }
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
            first.split_once(MARK).is_some_and(|(_, h)| h.trim_end_matches(" -->").trim() == sha256_hex(rest.as_bytes()))
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
pub fn skill(claude_dir: &Path, text: &str) -> Result<Outcome> {
    let path = claude_dir.join("skills/rvow/SKILL.md");
    let full = marked_skill(text);
    if let Ok(old) = std::fs::read_to_string(&path) {
        if old == full {
            return Ok(Outcome::Unchanged(path));
        }
        let (old_front, old_body) = split_front_matter(&old);
        let unedited = old_body.split_once('\n').is_some_and(|(first, rest)| {
            if let Some(hash) = first.strip_prefix("<!-- rvow-managed skill-sha256:").and_then(|s| s.strip_suffix(" -->")) {
                hash == sha256_hex(format!("{old_front}{rest}").as_bytes())
            } else {
                // Legacy markers did not cover front matter, so only matching front matter is safe to refresh.
                old_front == split_front_matter(text).0
                    && first.strip_prefix("<!-- rvow-managed sha256:").and_then(|s| s.strip_suffix(" -->"))
                        == Some(sha256_hex(rest.as_bytes()).as_str())
            }
        });
        if !unedited {
            return Ok(Outcome::KeptEdit(path));
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    std::fs::write(&path, full).map_err(|e| Error::io(&path, e))?;
    Ok(Outcome::Written(path))
}

fn marked_skill(text: &str) -> String {
    let (front, body) = split_front_matter(text);
    format!("{front}<!-- rvow-managed skill-sha256:{} -->\n{body}", sha256_hex(text.as_bytes()))
}

/// The skill for pi at `<pi_dir>/skills/rvow/SKILL.md`: the same text and
/// marker as the Claude skill, so a user's edit is kept the same way.
pub fn pi_skill(pi_dir: &Path, text: &str) -> Result<Outcome> {
    skill(pi_dir, text)
}

/// Compares an installed skill with the current rendered text, including legacy body-only markers.
pub fn skill_is_current(installed: &str, text: &str) -> bool {
    if installed == marked_skill(text) {
        return true;
    }
    let (front, body) = split_front_matter(text);
    installed == format!("{front}<!-- {MARK}{} -->\n{body}", sha256_hex(body.as_bytes()))
}

fn json_string(text: &str) -> String {
    Value::String(text.to_string()).to_string()
}

fn pretty(value: &Value) -> Result<String> {
    serde_json::to_string_pretty(value).map(|text| text + "\n").map_err(|e| Error::internal(format!("JSON does not serialize: {e}")))
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
/// binary path filled in and the rendered skill `skill` inside. rvow owns this folder, so every
/// file is rewritten to match.
pub fn claude_mod(dir: &Path, rvow: &Path, skill: &str) -> Result<Outcome> {
    let register = MOD_REGISTER.replace("'__RVOW_BIN__'", &json_string(&rvow.display().to_string()));
    let modules: String = MOD_MODULES.iter().map(|(_, src)| *src).collect();
    let version = format!(
        "{}+{}",
        env!("CARGO_PKG_VERSION"),
        &sha256_hex(format!("{register}{modules}{skill}{MOD_HOOKS_JSON}{MOD_TYPES}").as_bytes())[..12]
    );
    let mut plugin: Value =
        serde_json::from_str(MOD_PLUGIN_JSON).map_err(|e| Error::internal(format!("the bundled plugin.json is not JSON: {e}")))?;
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
    let mut files = vec![
        (".claude-plugin/marketplace.json".to_string(), pretty(&marketplace)?),
        ("plugins/rvow/.claude-plugin/plugin.json".to_string(), pretty(&plugin)?),
        ("plugins/rvow/hooks/hooks.json".to_string(), MOD_HOOKS_JSON.to_string()),
        ("plugins/rvow/hooks/register.tsx".to_string(), register),
        ("plugins/rvow/types/index.d.ts".to_string(), MOD_TYPES.to_string()),
        ("plugins/rvow/skills/rvow/SKILL.md".to_string(), skill.to_string()),
    ];
    files.extend(MOD_MODULES.iter().map(|(name, src)| (format!("plugins/rvow/hooks/{name}"), (*src).to_string())));
    let mut changed = false;
    for (rel, body) in files {
        let path = dir.join(rel);
        if std::fs::read_to_string(&path).is_ok_and(|old| old == body) {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        std::fs::write(&path, body).map_err(|e| Error::io(&path, e))?;
        changed = true;
    }
    Ok(if changed { Outcome::Written(dir.to_path_buf()) } else { Outcome::Unchanged(dir.to_path_buf()) })
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
    let text = pretty(&settings)?;
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

/// Runs `claude` with `args` and returns its stdout, or an error that names
/// the command, so a failed step tells the user what to run by hand.
fn run_claude(claude: &Path, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new(claude)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| Error::new("claude", format!("cannot run claude: {e}"), "install Claude Code or put claude on PATH"))?;
    if !out.status.success() {
        return Err(Error::new(
            "claude",
            format!("claude {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()),
            format!("run it by hand: claude {}", args.join(" ")),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Registers the marketplace in `dir` with Claude Code and installs or updates
/// the rvow plugin from it. Each step is skipped or turned into an update when
/// it already holds, so a repeated install is safe. Returns what it did.
pub fn register_claude_mod(claude: &Path, dir: &Path) -> Result<Vec<String>> {
    let mut done = Vec::new();
    let dir_s = dir.display().to_string();
    let markets = run_claude(claude, &["plugin", "marketplace", "list", "--json"])?;
    let registered: Option<String> = serde_json::from_str::<Vec<Value>>(&markets)
        .unwrap_or_default()
        .into_iter()
        .find(|m| m.get("name").and_then(Value::as_str) == Some("rvow"))
        .map(|m| m.get("path").or_else(|| m.get("installLocation")).and_then(Value::as_str).unwrap_or("").to_string());
    match registered {
        Some(path) if Path::new(&path) == dir => {
            run_claude(claude, &["plugin", "marketplace", "update", "rvow"])?;
            done.push("updated the rvow marketplace".into());
        }
        Some(path) => {
            // The rvow home moved (RVOW_HOME changed), so the old registration
            // would keep serving the old copy.
            run_claude(claude, &["plugin", "marketplace", "remove", "rvow"])?;
            run_claude(claude, &["plugin", "marketplace", "add", &dir_s])?;
            done.push(format!("moved the rvow marketplace from {path} to {dir_s}"));
        }
        None => {
            run_claude(claude, &["plugin", "marketplace", "add", &dir_s])?;
            done.push(format!("added the rvow marketplace from {dir_s}"));
        }
    }
    let plugins = run_claude(claude, &["plugin", "list", "--json"])?;
    let listed: Vec<Value> = serde_json::from_str(&plugins).unwrap_or_default();
    let ours = listed.iter().find(|v| v.get("id").and_then(Value::as_str) == Some("rvow@rvow"));
    if let Some(ours) = ours {
        run_claude(claude, &["plugin", "update", "rvow@rvow"])?;
        done.push("updated the rvow plugin; run /reload-plugins in Claude Code, or restart it, to load it".into());
        if ours.get("enabled").and_then(Value::as_bool) == Some(false) {
            run_claude(claude, &["plugin", "enable", "rvow@rvow"])?;
            done.push("enabled the rvow plugin, which was disabled; run /reload-plugins in Claude Code".into());
        }
    } else {
        run_claude(claude, &["plugin", "install", "rvow@rvow"])?;
        done.push("installed the rvow plugin; run /reload-plugins in Claude Code, or restart it, to load it".into());
    }
    Ok(done)
}

pub fn pi_extension(pi_dir: &Path) -> Result<Outcome> {
    let legacy = pi_dir.join("extensions/rvow.ts");
    match std::fs::read_to_string(&legacy) {
        Ok(old) => {
            let unedited = old.split_once('\n').is_some_and(|(first, rest)| {
                first.strip_prefix("// rvow-managed sha256:").is_some_and(|hash| hash == sha256_hex(rest.as_bytes()))
            });
            if !unedited {
                return Ok(Outcome::KeptEdit(legacy));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::io(&legacy, e)),
    }
    let dir = pi_dir.join("extensions/rvow");
    let files: Vec<_> = std::iter::once(("index.ts", PI_EXTENSION)).chain(PI_MODULES).collect();
    for (name, _) in &files {
        let path = dir.join(name);
        match std::fs::read_to_string(&path) {
            Ok(old) => {
                let unedited = old.split_once('\n').is_some_and(|(first, rest)| {
                    first.strip_prefix("// rvow-managed sha256:").is_some_and(|hash| hash == sha256_hex(rest.as_bytes()))
                });
                if !unedited {
                    return Ok(Outcome::KeptEdit(path));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io(&path, e)),
        }
    }
    let mut changed = false;
    for (name, body) in files {
        match write_managed(&dir.join(name), body, "//")? {
            Outcome::KeptEdit(path) => return Ok(Outcome::KeptEdit(path)),
            Outcome::Written(_) => changed = true,
            _ => {}
        }
    }
    match std::fs::remove_file(&legacy) {
        Ok(()) => changed = true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::io(&legacy, e)),
    }
    Ok(if changed { Outcome::Written(dir) } else { Outcome::Unchanged(dir) })
}

/// A settings hook command that runs `rvow hook claude <event>` or the neutral `rvow hook <event>`.
/// The installed rvow files that no longer match what this binary would write:
/// the skill, the mod's files and the pi extension files. An edited file
/// counts too, since `rvow install` keeps edits and the user decides. A file
/// that is not installed is not listed.
pub fn stale_managed_files(claude_dir: &Path, mod_dir: &Path, pi_dir: Option<&Path>, skill_text: &str) -> Vec<PathBuf> {
    let mut stale = Vec::new();
    // The two marked forms of the skill, built once for its three copies.
    let marked = marked_skill(skill_text);
    let (front, body) = split_front_matter(skill_text);
    let legacy = format!("{front}<!-- {MARK}{} -->\n{body}", sha256_hex(body.as_bytes()));
    let skill_current = |t: &str| t == skill_text || t == marked || t == legacy;
    let mut check = |path: PathBuf, current: &dyn Fn(&str) -> bool| {
        if std::fs::read_to_string(&path).is_ok_and(|installed| !current(&installed)) {
            stale.push(path);
        }
    };
    check(claude_dir.join("skills/rvow/SKILL.md"), &skill_current);
    let plugin = mod_dir.join("plugins/rvow");
    check(plugin.join("skills/rvow/SKILL.md"), &skill_current);
    check(plugin.join("hooks/hooks.json"), &|t| t == MOD_HOOKS_JSON);
    check(plugin.join("types/index.d.ts"), &|t| t == MOD_TYPES);
    for (name, src) in MOD_MODULES {
        check(plugin.join("hooks").join(name), &|t| t == src);
    }
    check(plugin.join("hooks/register.tsx"), &|t| {
        // The installed copy holds the binary path where the template holds the placeholder.
        let literal = t.lines().find_map(|l| l.split_once("const INSTALLED_BIN = ").map(|(_, v)| v.trim().to_string()));
        literal.is_some_and(|bin| {
            t.len() + "'__RVOW_BIN__'".len() == MOD_REGISTER.len() + bin.len() && t == MOD_REGISTER.replace("'__RVOW_BIN__'", &bin)
        })
    });
    if let Some(pi) = pi_dir {
        check(pi.join("skills/rvow/SKILL.md"), &skill_current);
        let dir = pi.join("extensions/rvow");
        for (name, body) in std::iter::once(("index.ts", PI_EXTENSION)).chain(PI_MODULES) {
            // The header is `// <MARK><64 hex digits>\n`, so only a file of that length is hashed.
            check(dir.join(name), &|t| {
                t.len() == 3 + MARK.len() + 64 + 1 + body.len()
                    && t.ends_with(body)
                    && t == format!("// {MARK}{}\n{body}", sha256_hex(body.as_bytes()))
            });
        }
    }
    stale
}

fn is_rvow_command(command: &str) -> bool {
    command.contains("hook claude")
        || (command.contains("rvow")
            && ["pre-tool-use", "post-tool-use", "session-start", "stop", "status", "view"]
                .iter()
                .any(|e| command.contains(&format!(" hook {e}"))))
}

fn is_rvow_entry(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hs| hs.iter().any(|h| h.get("command").and_then(Value::as_str).is_some_and(is_rvow_command)))
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
        Some(t) if !t.trim().is_empty() => serde_json::from_str(t)
            .map_err(|e| Error::new("settings", format!("{}: {e}", path.display()), "fix the JSON, then run rvow install again"))?,
        _ => json!({}),
    };
    let bin = shell_quote(&rvow.display().to_string());
    let entry = |matcher: Option<&str>, event: &str| {
        let hook = json!({"type": "command", "command": format!("{bin} hook claude {event}")});
        match matcher {
            Some(m) => json!({"matcher": m, "hooks": [hook]}),
            None => json!({"hooks": [hook]}),
        }
    };
    let wanted = [
        ("PreToolUse", entry(Some(TOOLS), "pre-tool-use")),
        ("PostToolUse", entry(Some(TOOLS), "post-tool-use")),
        ("PostToolUseFailure", entry(Some(TOOLS), "post-tool-use")),
        ("SessionStart", entry(None, "session-start")),
        ("Stop", entry(None, "stop")),
    ];
    let root = settings.as_object_mut().ok_or_else(|| {
        Error::new(
            "settings",
            format!("{}: the top level is not a JSON object", path.display()),
            format!("replace the contents of {} with {{}}, then run `rvow install` again", path.display()),
        )
    })?;
    let hooks = root.entry("hooks").or_insert_with(|| json!({})).as_object_mut().ok_or_else(|| {
        Error::new(
            "settings",
            format!("{}: \"hooks\" is not an object", path.display()),
            format!("delete the \"hooks\" key in {}, then run `rvow install` again", path.display()),
        )
    })?;
    for (event, e) in wanted {
        let list = hooks.entry(event).or_insert_with(|| json!([]));
        let arr = list.as_array_mut().ok_or_else(|| {
            Error::new(
                "settings",
                format!("{}: hooks.{event} is not a list", path.display()),
                format!("delete the \"{event}\" key under \"hooks\" in {}, then run `rvow install` again", path.display()),
            )
        })?;
        arr.retain(|x| !is_rvow_entry(x));
        arr.push(e);
    }
    let text = pretty(&settings)?;
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

/// The pre-push hook script. It saves the ref lines git gave it, hands them to
/// the hook that `rvow install` moved to `pre-push.rvow-prev` and then to
/// `rvow gate --push`, and exits with the gate's status. It refuses to run an
/// rvow binary that changed since install, so a binary the agent rebuilt cannot
/// pass the gate. The temp file goes away on every exit path, so a run that
/// fails or is killed leaves nothing behind.
pub fn pre_push_script(rvow: &Path) -> Result<String> {
    let bytes = std::fs::read(rvow).map_err(|e| Error::io(rvow, e))?;
    let bin = shell_quote(&rvow.display().to_string());
    Ok(format!(
        "#!/bin/sh\n{PRE_PUSH_MARK}, written by rvow install\n# One group: the shell reads the whole hook before the old hook can change it.\n{{\nrvow={bin}\nwant={}\nprev=\"$0{PRE_PUSH_PREV_SUFFIX}\"\nrefs=$(mktemp) || exit 1\ntrap 'rm -f \"$refs\"' EXIT\ncat > \"$refs\" || exit 1\nif [ -x \"$prev\" ]; then\n  \"$prev\" \"$@\" < \"$refs\" || exit $?\nfi\nhave=$(shasum -a 256 \"$rvow\" | cut -d' ' -f1)\nif [ \"$have\" != \"$want\" ]; then\n  echo \"rvow: $rvow changed since rvow install; run rvow install again\" >&2\n  exit 1\nfi\nenv -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE \"$rvow\" gate --push{PRE_PUSH_REFS_REDIRECT}\nstatus=$?\nrm -f \"$refs\"\ntrap - EXIT\nexit $status\n}}\n",
        sha256_hex(&bytes)
    ))
}

/// The line a user adds as the first command of a pre-push hook that another
/// hook manager owns. A failed gate exits before the rest of the hook runs.
pub fn pre_push_line(rvow: &Path) -> String {
    format!("{} gate --push || exit 1", shell_quote(&rvow.display().to_string()))
}

/// Whether the rvow gate is still at `common`: the record file `rvow install`
/// writes, or a hook that runs the gate.
pub fn has_gate(common: &Path) -> bool {
    let hooks = common.join("hooks");
    hooks.join(PRE_PUSH_RECORD).is_file()
        || std::fs::read(hooks.join("pre-push")).is_ok_and(|script| String::from_utf8_lossy(&script).contains(PRE_PUSH_MARK))
}

/// The work tree of a git common dir: the folder that holds `.git`. A bare
/// repository's common dir, or one named otherwise, belongs to no single work tree.
pub fn work_tree(common: &Path) -> Option<&Path> {
    if common.file_name().is_some_and(|name| name == ".git") { common.parent() } else { None }
}

/// Writes `.git/hooks/pre-push` when no other hook manager owns it.
pub fn pre_push(top: &Path, rvow: &Path) -> Result<Outcome> {
    let script = pre_push_script(rvow)?;
    let common = git::run(top, &["rev-parse", "--git-common-dir"])?;
    let hooks = top.join(common).join("hooks");
    std::fs::create_dir_all(&hooks).map_err(|e| Error::io(&hooks, e))?;
    let record = hooks.join(PRE_PUSH_RECORD);
    let recorded = format!("{PRE_PUSH_MARK}\nsha256:{}\n", sha256_hex(&std::fs::read(rvow).map_err(|e| Error::io(rvow, e))?));
    if !std::fs::read_to_string(&record).is_ok_and(|text| text == recorded) {
        std::fs::write(&record, recorded).map_err(|e| Error::io(&record, e))?;
    }
    if let Ok(dir) = git::run(top, &["config", "--get", "core.hooksPath"])
        && !dir.is_empty()
    {
        return Ok(Outcome::AddLine { path: top.join(&dir).join("pre-push"), line: pre_push_line(rvow) });
    }
    let path = hooks.join("pre-push");
    let mut chained = None;
    if std::fs::symlink_metadata(&path).is_ok() {
        // A hook that is not text, or a dangling link, is still the user's hook.
        let old = std::fs::read(&path).unwrap_or_default();
        if old == script.as_bytes() {
            return Ok(Outcome::Unchanged(path));
        }
        if !String::from_utf8_lossy(&old).contains(PRE_PUSH_MARK) {
            let prev = hooks.join(format!("pre-push{PRE_PUSH_PREV_SUFFIX}"));
            std::fs::rename(&path, &prev).map_err(|e| Error::io(&path, e))?;
            chained = Some(prev);
        }
    }
    std::fs::write(&path, script).map_err(|e| Error::io(&path, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).map_err(|e| Error::io(&path, e))?;
    }
    Ok(match chained {
        Some(prev) => Outcome::Chained { path, prev },
        None => Outcome::Written(path),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testrepo::*;

    const TEST_SKILL: &str = "---\nname: rvow\ndescription: test\n---\n# rvow\n";

    #[test]
    fn install_fresh_home_leaves_nothing_stale() {
        let tmp = TempDir::new();
        let (claude, mod_dir, pi) = (tmp.0.join(".claude"), tmp.0.join("data/claude-plugin"), tmp.0.join("pi"));
        std::fs::create_dir_all(&pi).unwrap();
        skill(&claude, TEST_SKILL).unwrap();
        claude_mod(&mod_dir, &tmp.0.join("bin/rvow"), TEST_SKILL).unwrap();
        pi_skill(&pi, TEST_SKILL).unwrap();
        pi_extension(&pi).unwrap();
        assert_eq!(stale_managed_files(&claude, &mod_dir, Some(&pi), TEST_SKILL), Vec::<PathBuf>::new());
        let other = format!("{TEST_SKILL}more\n");
        assert!(
            !stale_managed_files(&claude, &mod_dir, Some(&pi), &other).is_empty(),
            "a binary with another skill sees the files as stale"
        );
    }

    #[test]
    fn managed_files_upgrade_unless_edited() {
        let tmp = TempDir::new();
        let path = tmp.0.join("ext/rvow.ts");
        assert!(matches!(write_managed(&path, "a\n", "//").unwrap(), Outcome::Written(_)));
        assert!(matches!(write_managed(&path, "a\n", "//").unwrap(), Outcome::Unchanged(_)));
        assert!(matches!(write_managed(&path, "b\n", "//").unwrap(), Outcome::Written(_)));
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text + "// mine\n").unwrap();
        assert!(matches!(write_managed(&path, "c\n", "//").unwrap(), Outcome::KeptEdit(_)));
    }

    #[test]
    fn skill_keeps_front_matter_first() {
        let tmp = TempDir::new();
        assert!(matches!(skill(&tmp.0, TEST_SKILL).unwrap(), Outcome::Written(_)));
        let text = std::fs::read_to_string(tmp.0.join("skills/rvow/SKILL.md")).unwrap();
        assert!(text.starts_with("---\nname: rvow\n"), "{text}");
        assert!(matches!(skill(&tmp.0, TEST_SKILL).unwrap(), Outcome::Unchanged(_)));
        std::fs::write(tmp.0.join("skills/rvow/SKILL.md"), text + "mine\n").unwrap();
        assert!(matches!(skill(&tmp.0, TEST_SKILL).unwrap(), Outcome::KeptEdit(_)));
    }

    #[test]
    fn settings_merge_keeps_other_hooks_and_order() {
        let tmp = TempDir::new();
        let path = tmp.0.join("settings.json");
        let original = r#"{"model": "x", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "other"}]}]}, "zeta": 1}"#;
        std::fs::write(&path, original).unwrap();
        let bin = Path::new("/opt/rvow");
        assert!(matches!(claude_settings(&path, bin).unwrap(), Outcome::Written(_)));
        assert!(matches!(claude_settings(&path, bin).unwrap(), Outcome::Unchanged(_)));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["model", "hooks", "zeta"]);
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"][0]["command"], "other");
        assert_eq!(stop[1]["hooks"][0]["command"], "'/opt/rvow' hook claude stop");
        assert_eq!(v["hooks"]["PreToolUse"][0]["matcher"], TOOLS);
        assert_eq!(std::fs::read_to_string(tmp.0.join("settings.json.rvow-backup")).unwrap(), original);
        claude_settings(&path, Path::new("/new/rvow")).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 2, "old rvow entry replaced");
    }

    #[test]
    fn settings_post_tool_use_hooks_are_written_and_recognized() {
        let tmp = TempDir::new();
        let path = tmp.0.join("settings.json");
        let bin = Path::new("/opt/rvow");
        assert!(matches!(claude_settings(&path, bin).unwrap(), Outcome::Written(_)));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for event in ["PostToolUse", "PostToolUseFailure"] {
            let arr = v["hooks"][event].as_array().expect(event);
            assert_eq!(arr.len(), 1, "{event}");
            assert_eq!(arr[0]["matcher"], TOOLS, "{event}");
            assert_eq!(arr[0]["hooks"][0]["command"], "'/opt/rvow' hook claude post-tool-use");
        }
        assert!(is_rvow_command("'/opt/rvow' hook claude post-tool-use"));
        assert!(is_rvow_command("'/opt/rvow' hook post-tool-use"));
        // A repeated install stays stable, and the mod switch removes them.
        assert!(matches!(claude_settings(&path, bin).unwrap(), Outcome::Unchanged(_)));
        assert!(matches!(remove_claude_settings(&path).unwrap(), Outcome::Written(_)));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["hooks"]["PostToolUse"].as_array().unwrap().len(), 0);
        assert_eq!(v["hooks"]["PostToolUseFailure"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn mod_marketplace_fills_the_binary_and_is_stable() {
        let tmp = TempDir::new();
        let dir = tmp.0.join("claude-plugin");
        let bin = Path::new("/opt/bin/rvow");
        assert!(matches!(claude_mod(&dir, bin, TEST_SKILL).unwrap(), Outcome::Written(_)));
        assert!(matches!(claude_mod(&dir, bin, TEST_SKILL).unwrap(), Outcome::Unchanged(_)));
        let reg = std::fs::read_to_string(dir.join("plugins/rvow/hooks/register.tsx")).unwrap();
        assert!(reg.contains(r#"const INSTALLED_BIN = "/opt/bin/rvow""#), "{reg}");
        assert!(dir.join("plugins/rvow/skills/rvow/SKILL.md").is_file());
        for (name, src) in MOD_MODULES {
            let installed = std::fs::read_to_string(dir.join("plugins/rvow/hooks").join(name));
            assert_eq!(installed.unwrap(), src, "{name} lands in the hooks dir");
        }
        let m: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(".claude-plugin/marketplace.json")).unwrap()).unwrap();
        assert_eq!(m["plugins"][0]["source"], "./plugins/rvow");
        assert!(matches!(claude_mod(&dir, Path::new("/new/rvow"), TEST_SKILL).unwrap(), Outcome::Written(_)));
    }

    #[test]
    fn switching_to_the_mod_removes_settings_hooks() {
        let tmp = TempDir::new();
        let path = tmp.0.join("settings.json");
        std::fs::write(&path, r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "other"}]}]}}"#).unwrap();
        claude_settings(&path, Path::new("/opt/rvow")).unwrap();
        assert!(matches!(remove_claude_settings(&path).unwrap(), Outcome::Written(_)));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(v["hooks"]["PreToolUse"].as_array().unwrap().len(), 0);
        assert!(matches!(remove_claude_settings(&path).unwrap(), Outcome::Unchanged(_)));
    }

    #[test]
    fn registers_then_updates_through_the_claude_cli() {
        let tmp = TempDir::new();
        let log = tmp.0.join("log");
        let state = tmp.0.join("installed");
        let fake = tmp.0.join("claude");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\necho \"$*\" >> {log}\ncase \"$*\" in\n  'plugin marketplace list --json') if [ -f {state} ]; then echo \"[{{\\\"name\\\":\\\"rvow\\\",\\\"path\\\":\\\"$(cat {state})\\\"}}]\"; else echo '[]'; fi ;;\n  'plugin list --json') if [ -f {state} ]; then echo '[{{\"id\":\"rvow@rvow\"}}]'; else echo '[]'; fi ;;\n  'plugin marketplace add '*) echo \"${{4}}\" > {state}.path ;;\n  'plugin install rvow@rvow') cat {state}.path > {state} ;;\nesac\n",
                log = log.display(),
                state = state.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let dir = tmp.0.join("market");
        let first = register_claude_mod(&fake, &dir).unwrap();
        assert!(first[0].starts_with("added") && first[1].starts_with("installed"), "{first:?}");
        let second = register_claude_mod(&fake, &dir).unwrap();
        assert!(second[0].starts_with("updated") && second[1].starts_with("updated"), "{second:?}");
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(calls.contains(&format!("plugin marketplace add {}", dir.display())), "{calls}");
        assert!(calls.contains("plugin update rvow@rvow"), "{calls}");

        std::fs::write(
            &fake,
            std::fs::read_to_string(&fake).unwrap().replace("{\"id\":\"rvow@rvow\"}", "{\"id\":\"rvow@rvow\",\"enabled\":false}"),
        )
        .unwrap();
        let off = register_claude_mod(&fake, &dir).unwrap();
        assert!(off[2].starts_with("enabled the rvow plugin"), "{off:?}");
        assert!(std::fs::read_to_string(&log).unwrap().contains("plugin enable rvow@rvow"));

        let moved = tmp.0.join("new-home-market");
        let third = register_claude_mod(&fake, &moved).unwrap();
        assert!(third[0].starts_with("moved the rvow marketplace"), "{third:?}");
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(calls.contains("plugin marketplace remove rvow"), "{calls}");
        assert!(calls.contains(&format!("plugin marketplace add {}", moved.display())), "{calls}");

        let failing = tmp.0.join("bad");
        std::fs::write(&failing, "#!/bin/sh\necho boom >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&failing, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let err = register_claude_mod(&failing, &dir).unwrap_err();
        assert!(err.fix.starts_with("run it by hand: claude plugin"), "{err:?}");
    }

    #[test]
    fn pre_push_respects_other_hook_managers() {
        let tmp = TempDir::new();
        let repo = repo_with_commit(&tmp.0, "r");
        let bin = tmp.0.join("rvow-bin");
        std::fs::write(&bin, "binary").unwrap();
        assert!(matches!(pre_push(&repo, &bin).unwrap(), Outcome::Written(_)));
        assert!(matches!(pre_push(&repo, &bin).unwrap(), Outcome::Unchanged(_)));
        let script = std::fs::read_to_string(repo.join(".git/hooks/pre-push")).unwrap();
        assert!(script.contains(&sha256_hex(b"binary")));

        std::fs::write(repo.join(".git/hooks/pre-push"), "#!/bin/sh\necho mine\n").unwrap();
        assert!(matches!(pre_push(&repo, &bin).unwrap(), Outcome::Chained { .. }));
        git(&repo, &["config", "core.hooksPath", ".husky"]);
        assert!(matches!(pre_push(&repo, &bin).unwrap(), Outcome::AddLine { .. }));
    }

    // The pre-push hook hands git's ref lines to the gate.

    /// Two ref lines, as git writes them to a pre-push hook.
    const REFS: &str = "refs/heads/main 1111111111111111111111111111111111111111 refs/heads/main 2222222222222222222222222222222222222222\nrefs/tags/v1 3333333333333333333333333333333333333333 refs/tags/v1 4444444444444444444444444444444444444444\n";
    /// The arguments git gives a pre-push hook.
    const HOOK_ARGS: [&str; 2] = ["origin", "git@example.com:r.git"];

    fn executable(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// What a hook that records its work appends: its name, its arguments, then
    /// the ref lines it got on stdin.
    fn recorder(name: &str, log: &Path, tail: &str) -> String {
        let log = shell_quote(&log.display().to_string());
        format!("#!/bin/sh\nprintf '{name} %s\\n' \"$*\" >> {log}\ncat >> {log}\n{tail}")
    }

    /// Runs `hook` with `refs` on its stdin and a temp folder of its own.
    fn run_hook(hook: &Path, refs: &str, tmp: &Path) -> std::process::Output {
        use std::io::Write;
        let mut child = std::process::Command::new("sh")
            .arg(hook)
            .args(HOOK_ARGS)
            .env("TMPDIR", tmp)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(refs.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    }

    /// A hook script rvow wrote in `dir`, and the log the gate it runs writes.
    fn hook_dir(tmp: &TempDir, gate: &str) -> (PathBuf, PathBuf) {
        let dir = tmp.0.join("hooks");
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("rvow");
        let log = tmp.0.join("hook.log");
        std::fs::write(&bin, gate).unwrap();
        executable(&bin);
        let hook = dir.join("pre-push");
        std::fs::write(&hook, pre_push_script(&bin).unwrap()).unwrap();
        executable(&hook);
        (hook, log)
    }

    #[test]
    fn pre_push_hook_passes_the_refs_to_the_gate() {
        let tmp = TempDir::new();
        let (hook, log) = hook_dir(&tmp, &recorder("gate", &tmp.0.join("hook.log"), "exit 0\n"));
        let scratch = tmp.0.join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        let ran = run_hook(&hook, REFS, &scratch);
        assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
        assert_eq!(std::fs::read_to_string(&log).unwrap(), format!("gate gate --push\n{REFS}"), "the gate got the ref lines");
        assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0, "the hook left its refs file behind");
    }

    #[test]
    fn pre_push_hook_runs_the_older_hook_first_with_the_same_refs() {
        let tmp = TempDir::new();
        let log = tmp.0.join("hook.log");
        let (hook, log) = hook_dir(&tmp, &recorder("gate", &log, "exit 0\n"));
        let prev = hook.with_file_name("pre-push.rvow-prev");
        std::fs::write(&prev, recorder("prev", &log, "")).unwrap();
        executable(&prev);
        let scratch = tmp.0.join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        let ran = run_hook(&hook, REFS, &scratch);
        assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
        let want = format!("prev {}\n{REFS}gate gate --push\n{REFS}", HOOK_ARGS.join(" "));
        assert_eq!(std::fs::read_to_string(&log).unwrap(), want, "the older hook ran first, with the same refs");
        assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0, "the hook left its refs file behind");
    }

    #[test]
    fn pre_push_hook_exits_with_the_gate_status() {
        let tmp = TempDir::new();
        let log = tmp.0.join("hook.log");
        let (hook, log) = hook_dir(&tmp, &recorder("gate", &log, "exit 1\n"));
        let scratch = tmp.0.join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        let ran = run_hook(&hook, REFS, &scratch);
        assert_eq!(ran.status.code(), Some(1), "a failed gate must fail the hook");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), format!("gate gate --push\n{REFS}"));
        assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0, "the hook left its refs file behind");
    }
}
