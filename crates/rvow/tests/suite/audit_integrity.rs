use std::path::{Path, PathBuf};
use std::process::Command;

use rvow_core::{change, gate, guard, ledger, proof, store, vow};
use serde_json::json;

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    project: store::Project,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("rvow-integrity-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(&repo)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "test@test")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "test@test")
                .output()
                .unwrap();
            assert!(out.status.success(), "{:?}", out);
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "initial"]);
        let project = store::init(&repo, &root.join("home"), store::Mode::Home).unwrap();
        Self { root, repo, project }
    }

    fn saved(&self, key: &str, text: &str) -> proof::Active {
        let dir = change::new(&self.project, "Integrity", "main", true).unwrap();
        let loaded = change::load(&dir.path, &[]).unwrap();
        let hash = vow::put_object(&self.project, text.as_bytes()).unwrap();
        let event = ledger::Event {
            kind: ledger::Kind::Prove,
            time: 1,
            parents: vec![],
            data: json!({"proved": true, "tests": {key: hash}}).as_object().unwrap().clone(),
            hash: String::new(),
            line: 1,
        };
        proof::Active { dir, loaded, events: vec![event], ledger_diags: vec![] }
    }

    fn hooks_path(&self, path: &str) {
        assert!(Command::new("git").current_dir(&self.repo).args(["config", "core.hooksPath", path]).status().unwrap().success());
    }

    fn hook_failures(&self) -> Vec<String> {
        gate::gate(&self.project, &self.root.join("home"), &self.repo, gate::Options::default(), &mut std::io::sink())
            .unwrap()
            .failures
            .into_iter()
            .filter(|f| f.contains("hook"))
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn audit_integrity_missing_default_installed_hook_is_a_failure() {
    let f = Fixture::new("deleted-default");
    let bin = f.root.join("rvow");
    std::fs::write(&bin, "#!/bin/sh\nexit 0\n").unwrap();
    executable(&bin);
    rvow_core::install::pre_push(&f.repo, &bin).unwrap();
    std::fs::remove_file(f.repo.join(".git/hooks/pre-push")).unwrap();
    assert!(!f.hook_failures().is_empty(), "deleting the installed default hook bypassed the gate");
}

#[test]
fn audit_integrity_added_shell_control_flow_cannot_disable_saved_checks() {
    let f = Fixture::new("shell-flow");
    let before = "check_one\ncheck_two\nexit 0\n";
    let c = f.saved("checks.sh", before);
    for added in ["check_early && exit 0", "check_early | cat", "check_early > /dev/null"] {
        std::fs::write(f.repo.join("checks.sh"), format!("{added}\n{before}")).unwrap();
        assert!(!guard::drift(&f.project, &f.repo, &c).is_empty(), "control-flow insertion bypassed the guard: {added}");
    }
}

const INLINE: &str =
    "#[cfg(test)]\nmod tests {\n    #[test]\n    fn check() {\n        assert!(true);\n        assert_eq!(2, 2);\n    }\n}\n";

#[test]
fn audit_integrity_inline_context_order_and_multiplicity() {
    let f = Fixture::new("inline");
    let c = f.saved("lib.rs#cfg(test)", INLINE);
    for text in [
        INLINE.replace("        assert!(true);", "        /*\n        assert!(true);\n        */"),
        INLINE.replace("        assert!(true);", "        let _ = r#\"\n        assert!(true);\n        \"#;"),
        INLINE.replace("        assert!(true);\n        assert_eq!(2, 2);", "        assert_eq!(2, 2);\n        assert!(true);"),
        INLINE.replace("    #[test]", "    #[cfg(any())]\n    #[test]"),
        INLINE.replace("    #[test]", "    #[ignore]\n    #[test]"),
        format!("#![cfg(any())]\n{INLINE}"),
        INLINE.replace("        assert!(true);", "        if false {\n        assert!(true);\n        }"),
        INLINE.replace("        assert!(true);", "        /* unterminated\n        assert!(true);"),
        INLINE.replace("        assert!(true);", "        return;\n        assert!(true);"),
    ] {
        std::fs::write(f.repo.join("lib.rs"), &text).unwrap();
        assert!(!guard::drift(&f.project, &f.repo, &c).is_empty(), "bypass: {text}");
    }
    let twice = INLINE.replace("        assert!(true);", "        assert!(true);\n        assert!(true);");
    let c = f.saved("lib.rs#cfg(test)", &twice);
    std::fs::write(f.repo.join("lib.rs"), INLINE).unwrap();
    assert!(!guard::drift(&f.project, &f.repo, &c).is_empty());
}

#[test]
fn audit_integrity_additions_and_literal_braces() {
    let source = INLINE
        .replace("        assert!(true);", "        let _ = r#\"}\"#;\n        // }\n        /* } /* } */ } */\n        assert!(true);");
    assert_eq!(guard::rust_test_modules(&source), source);
    let f = Fixture::new("additions");
    let c = f.saved("lib.rs#cfg(test)", INLINE);
    let extra = INLINE.replace("    #[test]", "    #[test]\n    fn additional() { assert!(true); }\n    #[test]");
    std::fs::write(f.repo.join("lib.rs"), extra).unwrap();
    assert!(guard::drift(&f.project, &f.repo, &c).is_empty());
}

#[test]
fn audit_integrity_missing_and_corrupt_objects() {
    let f = Fixture::new("objects");
    let c = f.saved("lib.rs#cfg(test)", INLINE);
    std::fs::write(f.repo.join("lib.rs"), INLINE).unwrap();
    let object = f.project.root.join("objects").join(vow::sha256_hex(INLINE.as_bytes()));
    std::fs::write(&object, "corrupt").unwrap();
    assert!(!guard::drift(&f.project, &f.repo, &c).is_empty());
    std::fs::remove_file(&object).unwrap();
    assert!(!guard::drift(&f.project, &f.repo, &c).is_empty());
}

fn executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn audit_integrity_hooks_path_rejects_comment_and_disabled_command() {
    let f = Fixture::new("comments");
    f.hooks_path(".manager");
    std::fs::create_dir_all(f.repo.join(".manager")).unwrap();
    let hook = f.repo.join(".manager/pre-push");
    for text in [
        "#!/bin/sh\n# rvow gate --strict --rerun\n",
        "#!/bin/sh\nexit 0\nrvow gate --strict --rerun\n",
        "#!/bin/sh\necho 'rvow gate --strict --rerun'\n",
    ] {
        std::fs::write(&hook, text).unwrap();
        executable(&hook);
        assert!(!f.hook_failures().is_empty(), "bypass: {text}");
    }
}

#[test]
fn audit_integrity_installed_hook_and_manager_line() {
    let f = Fixture::new("installed");
    assert!(f.hook_failures().is_empty(), "uninstalled repositories stay free");
    let bin = f.root.join("rvow");
    std::fs::write(&bin, "#!/bin/sh\nexit 0\n").unwrap();
    executable(&bin);
    rvow_core::install::pre_push(&f.repo, &bin).unwrap();
    assert!(f.hook_failures().is_empty());
    let hook = f.repo.join(".git/hooks/pre-push");
    let original = std::fs::read_to_string(&hook).unwrap();
    std::fs::write(&hook, original.replace("exec env", "# exec env")).unwrap();
    assert!(!f.hook_failures().is_empty());
    std::fs::write(&hook, original).unwrap();
    std::fs::write(&bin, "changed").unwrap();
    assert!(!f.hook_failures().is_empty());
    std::fs::write(&bin, "#!/bin/sh\nexit 0\n").unwrap();
    f.hooks_path(".manager");
    std::fs::create_dir_all(f.repo.join(".manager")).unwrap();
    let manager = f.repo.join(".manager/pre-push");
    std::fs::write(&manager, format!("#!/bin/sh\n'{}' gate --strict --rerun\n", bin.display())).unwrap();
    executable(&manager);
    assert!(f.hook_failures().is_empty());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&manager, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!f.hook_failures().is_empty());
    }
    std::fs::remove_file(&manager).unwrap();
    assert!(!f.hook_failures().is_empty());
}
