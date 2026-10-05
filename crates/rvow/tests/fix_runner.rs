use std::time::Duration;

use rvow_core::runner::{mask, run};

fn tests_run(script: &str) -> Option<u64> {
    run(script, &std::env::temp_dir(), &[], Duration::from_secs(10), &mut std::io::sink()).unwrap().tests_run
}

#[test]
fn nextest_counts_only_summary_lines_and_sums_runs() {
    assert_eq!(tests_run("echo '     Summary [   0.5s] 12 tests run: 12 passed, 0 skipped'"), Some(12));
    assert_eq!(
        tests_run(
            "echo '     Summary [   0.5s] 12 tests run: 12 passed, 0 skipped'; echo '     Summary [   0.1s] 3 tests run: 3 passed, 1 skipped'"
        ),
        Some(15)
    );
    assert_eq!(tests_run("echo 'the job will run: it passed and skipped nothing'"), None);
    assert_eq!(tests_run("echo '            Summary [ 0.5s] 99 tests run: 99 passed'"), None, "a deeply indented line is test output");
    assert_eq!(tests_run("echo '     Summary [   0.0s] 0 tests run: 0 passed, 2 skipped'"), Some(0));
}

#[test]
fn jest_counts_the_unindented_tests_line() {
    assert_eq!(tests_run("echo 'Tests:       1 failed, 12 passed, 13 total'"), Some(13));
    assert_eq!(tests_run("echo '    Tests: 40 total printed by a test'"), None);
    assert_eq!(tests_run("echo 'Tests:       0 total'"), Some(0));
}

#[test]
fn go_counts_package_lines() {
    assert_eq!(tests_run("printf 'ok  \\texample/a\\t0.006s [no tests to run]\\n?   \\texample/b\\t[no test files]\\n'"), Some(0));
    assert_eq!(tests_run("printf 'ok  \\texample/a\\t0.006s [no tests to run]\\nok  \\texample/c\\t0.1s\\n'"), None);
    assert_eq!(tests_run("printf '    ok  \\tnot go\\t0.1s [no tests to run]\\n'"), None);
    assert_eq!(tests_run("printf 'ok  \\texample/c\\t0.1s\\n'"), None);
}

#[test]
fn mask_keeps_shas_and_paths_and_hides_secrets() {
    let m = |s: &str| mask(s, None, "");
    let sha1 = "0123456789abcdef0123456789abcdef01234567";
    let sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    assert_eq!(m(&format!("commit {sha1}")), format!("commit {sha1}"));
    assert_eq!(m(&format!("blob {sha256} ok")), format!("blob {sha256} ok"));
    let path = "crates/rvow-core/src/archive/merge_tests_long";
    assert_eq!(m(&format!("at {path}")), format!("at {path}"));
    assert_eq!(m("token ghp_ABCDEFGHIJKLMNOP1234"), "token <masked>");
    assert_eq!(m("abcdefabcdefabcdefabcdefabcdefab"), "<masked>");
    assert_eq!(m("0123456789abcdef0123456789abcdef0123456z"), "<masked>");
    assert_eq!(m("API_KEY=0123456789abcdef0123456789abcdef01234567"), "API_KEY=<masked>");
}
