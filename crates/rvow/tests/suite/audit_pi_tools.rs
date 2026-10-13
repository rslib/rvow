use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

struct Installed(PathBuf);

impl Drop for Installed {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn npm(root: &Path, script: &str, installed: &Path) {
    let output = Command::new("npm")
        .current_dir(root)
        .args(["--prefix", "extensions", "run", script])
        .env("RVOW_TEST_INSTALLED", installed)
        .output()
        .expect("npm must be installed for pi behavior audits");
    assert!(output.status.success(), "{script}\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

#[test]
#[ignore = "runs the pi npm scripts; CI runs it with --include-ignored"]
fn audit_pi_tools_installed_modules_and_typed_execution() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let tmp = Installed(root.join(format!("extensions/pi/audit-install-{}-{nonce}", std::process::id())));
    rvow_core::install::pi_extension(&tmp.0).unwrap();
    let index = tmp.0.join("extensions/rvow/index.ts");
    assert!(index.is_file());
    for module in std::fs::read_dir(root.join("extensions/pi/rvow")).unwrap() {
        let module = module.unwrap();
        if module.path().extension().is_some_and(|ext| ext == "ts") {
            let body = std::fs::read_to_string(module.path()).unwrap();
            let installed = std::fs::read_to_string(tmp.0.join("extensions/rvow").join(module.file_name())).unwrap();
            assert_eq!(installed.split_once('\n').unwrap().1, body, "every pi source module must be installed");
        }
    }
    npm(&root, "typecheck:pi", &index);
    npm(&root, "test:pi:tools", &index);

    let legacy = tmp.0.join("extensions/rvow.ts");
    let body = "export default function() {}\n";
    let hash = rvow_core::vow::sha256_hex(body.as_bytes());
    std::fs::write(&legacy, format!("// rvow-managed sha256:{hash}\n{body}")).unwrap();
    rvow_core::install::pi_extension(&tmp.0).unwrap();
    assert!(!legacy.exists(), "unedited flat adapter must be removed");
    let edited = format!("// rvow-managed sha256:{hash}\n{body}// user edit\n");
    std::fs::write(&legacy, &edited).unwrap();
    std::fs::remove_dir_all(tmp.0.join("extensions/rvow")).unwrap();
    assert_eq!(rvow_core::install::pi_extension(&tmp.0).unwrap(), rvow_core::install::Outcome::KeptEdit(legacy.clone()));
    assert_eq!(std::fs::read_to_string(&legacy).unwrap(), edited);
    assert!(!index.exists(), "edited legacy adapter must not gain a duplicate loader");
}
