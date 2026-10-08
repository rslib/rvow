use std::path::Path;
use std::process::Command;

use rvow_core::diag::Rule;
use rvow_core::{hook, lint, spec};

#[test]
fn audit_modules_archive_runs_merge_and_lifecycle_regressions() {
    let output = Command::new(env!("CARGO"))
        .args(["test", "--locked", "-p", "rvow-core", "--lib", "archive::tests::"])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
    for test in [
        "archive_merges_every_op_in_place",
        "a_second_change_on_a_requirement_conflicts_until_rebased",
        "a_merge_that_would_not_validate_writes_nothing",
    ] {
        assert!(stdout.contains(&format!("archive::tests::{test} ... ok")), "{stdout}");
    }
}

#[test]
fn audit_modules_prose_keeps_diagnostics_and_scenarios() {
    let source = "# Core\n## REQ-k3f9m2 Record\nWHEN a run ends, rvow SHALL record `rvow prove`.\n### Scenario: records\n- GIVEN a run\n- WHEN it ends\n- THEN the record exists\n";
    let (parsed, diagnostics) = spec::parse_spec(source);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert!(lint::lint_spec(&parsed).is_empty());

    let source = source
        .replace("rvow SHALL record `rvow prove`.", "it must quickly call `src/record.rs` if necessary.")
        .replace("### Scenario: records", "### Scenario: records and prints");
    let (parsed, diagnostics) = spec::parse_spec(&source);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let diagnostics = lint::lint_spec(&parsed);
    let rules: Vec<_> = diagnostics.iter().map(|d| d.rule).collect();
    assert_eq!(
        rules,
        [Rule::Keyword, Rule::Keyword, Rule::Ears, Rule::Vague, Rule::EscapeClause, Rule::Pronoun, Rule::DesignInSpec, Rule::ScenarioTitle]
    );
    assert_eq!(diagnostics[0].line, 3);
    assert_eq!(diagnostics[0].message, "\"must\" carries no meaning unless it is written in capitals");
    assert_eq!(diagnostics.last().unwrap().line, 4);
}

#[test]
fn audit_modules_shell_keeps_quotes_heredocs_and_denials() {
    for command in
        ["cat <<'EOF'\nrvow accept CHG-x\nEOF", "echo 'rvow confirm AMD-x'", "rvow prove TSK-x", "git commit -m 'accept confirm'"]
    {
        assert_eq!(hook::bash_denial(command), None, "{command}");
    }
    for command in [
        "echo \"$(/usr/local/bin/rvow accept CHG-x)\"",
        "cat <<EOF\nquoted text\nEOF\nrvow confirm AMD-x",
        "cargo run -- confirm AMD-x",
        "git commit -n -m x",
        "rvow answer CHG-x:proposal.md:3 yes --via=dialog",
        "echo x >> ledger.jsonl",
    ] {
        assert!(hook::bash_denial(command).is_some(), "{command}");
    }
    assert_eq!(
        hook::bash_denial("git push --no-verify").as_deref(),
        Some("--no-verify skips the hooks that check the plan; run git commit and git push without it")
    );
}
