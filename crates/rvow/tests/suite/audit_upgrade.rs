use std::path::PathBuf;
use std::process::{Command, Stdio};

use rvow_core::install::{self, Outcome};
use rvow_core::vow::sha256_hex;

const OLD: &str = "---\nname: rvow\ndescription: original\n---\nOld body\n";
const NEW: &str = "---\nname: rvow\ndescription: original\n---\nNew body\n";

struct Fixture(PathBuf);

impl Fixture {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("rvow-audit-upgrade-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn skill(&self) -> PathBuf {
        self.0.join(".claude/skills/rvow/SKILL.md")
    }

    fn cli(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_rvow"))
            .args(args)
            .current_dir(&self.0)
            .env("HOME", &self.0)
            .env("RVOW_HOME", self.0.join("store"))
            .env("CLAUDE_CONFIG_DIR", self.0.join(".claude"))
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("XDG_DATA_HOME", self.0.join("data"))
            .env("PI_CODING_AGENT_DIR", self.0.join("pi"))
            .env_remove("RVOW_AGENT")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn audit_upgrade_refreshes_unedited_skills_and_is_idempotent() {
    let f = Fixture::new("refresh");
    let dir = f.0.join(".claude");
    assert!(matches!(install::skill(&dir, OLD).unwrap(), Outcome::Written(_)));
    assert!(matches!(install::skill(&dir, NEW).unwrap(), Outcome::Written(_)));
    assert!(std::fs::read_to_string(f.skill()).unwrap().ends_with("New body\n"));
    assert!(matches!(install::skill(&dir, NEW).unwrap(), Outcome::Unchanged(_)));
    let updated_front = NEW.replace("description: original", "description: upgraded");
    assert!(matches!(install::skill(&dir, &updated_front).unwrap(), Outcome::Written(_)));
    let text = std::fs::read_to_string(f.skill()).unwrap();
    assert!(text.starts_with("---\nname: rvow\ndescription: upgraded\n---\n"));
    assert!(install::skill_is_current(&text, &updated_front));
    assert!(matches!(install::skill(&dir, &updated_front).unwrap(), Outcome::Unchanged(_)));
}

#[test]
fn audit_upgrade_preserves_front_matter_and_body_edits() {
    for (name, from, to) in [("front", "description: original", "description: personal"), ("body", "Old body", "Personal body")] {
        let f = Fixture::new(name);
        let dir = f.0.join(".claude");
        install::skill(&dir, OLD).unwrap();
        let edited = std::fs::read_to_string(f.skill()).unwrap().replace(from, to);
        std::fs::write(f.skill(), &edited).unwrap();
        assert!(matches!(install::skill(&dir, NEW).unwrap(), Outcome::KeptEdit(_)), "{name}");
        assert_eq!(std::fs::read_to_string(f.skill()).unwrap(), edited);
    }
}

#[test]
fn audit_upgrade_legacy_markers_refresh_only_with_matching_front_matter() {
    for edited in [false, true] {
        let f = Fixture::new(if edited { "legacy-edit" } else { "legacy-clean" });
        let dir = f.0.join(".claude");
        std::fs::create_dir_all(f.skill().parent().unwrap()).unwrap();
        let front = if edited { "personal" } else { "original" };
        let old =
            format!("---\nname: rvow\ndescription: {front}\n---\n<!-- rvow-managed sha256:{} -->\nOld body\n", sha256_hex(b"Old body\n"));
        std::fs::write(f.skill(), &old).unwrap();
        let result = install::skill(&dir, NEW).unwrap();
        if edited {
            assert!(matches!(result, Outcome::KeptEdit(_)));
            assert_eq!(std::fs::read_to_string(f.skill()).unwrap(), old);
        } else {
            assert!(matches!(result, Outcome::Written(_)));
        }
    }
}

#[test]
fn audit_upgrade_warning_preserves_next_and_final_text_budget() {
    let text = format!("{}\nnext: TSK-abc123 {}\nrules: owners decide\n", "long notes\n".repeat(100), "title".repeat(500));
    for warning in [
        format!("rvow: stale skill. Please run rvow install. Path: {}", "path".repeat(2000)),
        "rvow: stale skill. Please run rvow install.\n".repeat(100),
    ] {
        let combined = rvow_core::brief::with_warning(&text, &warning);
        assert!(combined.len() <= 1500, "{}", combined.len());
        assert!(combined.lines().count() <= 25);
        assert!(combined.contains("run rvow install"));
        assert!(combined.contains("next: TSK-abc123"));
        assert!(combined.contains("rules:"));
    }
}

#[test]
fn audit_upgrade_session_start_warns_for_stale_and_edited_skills() {
    let f = Fixture::new("warning");
    let out = f.cli(&["install", "--claude", "settings"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    let current = std::fs::read_to_string(f.skill()).unwrap();
    let out = f.cli(&["hook", "claude", "session-start"]);
    assert!(out.status.success());
    assert!(!String::from_utf8_lossy(&out.stdout).contains("run rvow install"));
    for text in [current.replace("name: rvow", "name: personal"), format!("{current}\nPersonal body\n")] {
        std::fs::write(f.skill(), text).unwrap();
        let out = f.cli(&["hook", "claude", "session-start"]);
        assert!(out.status.success());
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let context = value["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
        assert!(context.contains("run rvow install"), "{context}");
        assert_eq!(value["hookSpecificOutput"]["hookEventName"], "SessionStart");
    }
    std::fs::remove_file(f.skill()).unwrap();
    install::skill(&f.0.join(".claude"), OLD).unwrap();
    let out = f.cli(&["hook", "claude", "session-start"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("run rvow install"));
    assert!(f.cli(&["install", "--claude", "settings"]).status.success());
    let out = f.cli(&["hook", "claude", "session-start"]);
    assert!(out.status.success());
    assert!(!String::from_utf8_lossy(&out.stdout).contains("run rvow install"));
}

fn files_under(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[test]
fn audit_upgrade_a_second_install_refreshes_every_managed_file_of_an_older_one() {
    let f = Fixture::new("whole");
    std::fs::create_dir_all(f.0.join("pi")).unwrap();
    assert!(f.cli(&["install"]).status.success());
    let (claude, mod_dir, pi) = (f.0.join(".claude"), f.0.join("data/rvow/claude-plugin"), f.0.join("pi"));
    let plugin = mod_dir.join("plugins/rvow");
    let skill_text = std::fs::read_to_string(plugin.join("skills/rvow/SKILL.md")).unwrap();
    let stale = || install::stale_managed_files(&claude, &mod_dir, Some(&pi), &skill_text);
    assert_eq!(stale(), Vec::<PathBuf>::new(), "a fresh install is current");

    // The files an older binary wrote: the mod's files hold other text, the others carry a valid marker for it.
    let mut managed = Vec::new();
    files_under(&plugin, &mut managed);
    managed.retain(|p| !p.ends_with("plugin.json"));
    for path in &managed {
        std::fs::write(path, "old\n").unwrap();
    }
    let mut pi_files = Vec::new();
    files_under(&pi.join("extensions/rvow"), &mut pi_files);
    assert!(!pi_files.is_empty(), "the pi extension is installed");
    for path in &pi_files {
        std::fs::write(path, format!("// rvow-managed sha256:{}\nold\n", sha256_hex(b"old\n"))).unwrap();
    }
    assert_eq!(stale().len(), managed.len() + pi_files.len(), "{:?}", stale());

    assert!(f.cli(&["install"]).status.success());
    assert_eq!(stale(), Vec::<PathBuf>::new(), "the second install refreshed every file");
    assert!(std::fs::read_to_string(plugin.join("hooks/register.tsx")).unwrap().contains("INSTALLED_BIN"));
}
