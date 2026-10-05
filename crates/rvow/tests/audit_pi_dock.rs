use std::path::Path;
use std::process::Command;

#[test]
fn audit_pi_dock_refresh_focus_escape_and_shutdown() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for script in ["typecheck:pi", "test:pi:dock"] {
        let output = Command::new("npm")
            .current_dir(&root)
            .args(["--prefix", "extensions", "run", script])
            .output()
            .expect("npm must be installed for pi dock behavior audits");
        assert!(
            output.status.success(),
            "{script}\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
