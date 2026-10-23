use std::path::PathBuf;

use rvow_hook::hook::script_rule;

struct Tmp {
    dir: PathBuf,
}

impl Tmp {
    fn new(name: &str) -> Tmp {
        let dir = std::env::temp_dir().join(format!("rvow-script-finder-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Tmp { dir }
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn script_finder_stdin_is_not_a_script() {
    let tmp = Tmp::new("stdin");
    std::fs::write(tmp.dir.join("notes.md"), "run rvow accept CHG-x\n").unwrap();
    assert!(script_rule("python3 - notes.md", &tmp.dir).is_none());
}

#[test]
fn script_finder_inline_code_ignores_a_data_file() {
    let tmp = Tmp::new("inline");
    std::fs::write(tmp.dir.join("notes.md"), "run rvow accept CHG-x\n").unwrap();
    // The old finder read the code string as a path, so it only fails this test when that path exists.
    std::fs::write(tmp.dir.join("console.log(1)"), "run rvow accept CHG-x\n").unwrap();
    assert!(script_rule("node -e \"console.log(1)\" notes.md", &tmp.dir).is_none());
}

#[test]
fn script_finder_real_script_is_still_denied() {
    let tmp = Tmp::new("real");
    std::fs::write(tmp.dir.join("run.py"), "rvow accept CHG-abcdef\n").unwrap();
    let denial = script_rule("python3 run.py", &tmp.dir);
    assert!(denial.is_some_and(|d| d.reason.contains("run.py")));
}
