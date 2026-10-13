//! The integration tests of the rvow crate, built as one binary. Each module is one former test file; the files live in suite/.

#[path = "suite/accept_speed.rs"]
mod accept_speed;
#[path = "suite/agent_mode.rs"]
mod agent_mode;
#[path = "suite/amend_baseline.rs"]
mod amend_baseline;
#[path = "suite/amend_fixes.rs"]
mod amend_fixes;
#[path = "suite/attest.rs"]
mod attest;
#[path = "suite/audit_build.rs"]
mod audit_build;
#[path = "suite/audit_ci.rs"]
mod audit_ci;
#[path = "suite/audit_context.rs"]
mod audit_context;
#[path = "suite/audit_docs.rs"]
mod audit_docs;
#[path = "suite/audit_evals.rs"]
mod audit_evals;
#[path = "suite/audit_gate.rs"]
mod audit_gate;
#[path = "suite/audit_hardening.rs"]
mod audit_hardening;
#[path = "suite/audit_import.rs"]
mod audit_import;
#[path = "suite/audit_integration.rs"]
mod audit_integration;
#[path = "suite/audit_integrity.rs"]
mod audit_integrity;
#[path = "suite/audit_modules.rs"]
mod audit_modules;
#[path = "suite/audit_pi_dock.rs"]
mod audit_pi_dock;
#[path = "suite/audit_pi_tools.rs"]
mod audit_pi_tools;
#[path = "suite/audit_proof.rs"]
mod audit_proof;
#[path = "suite/audit_settings.rs"]
mod audit_settings;
#[path = "suite/audit_signing.rs"]
mod audit_signing;
#[path = "suite/audit_text.rs"]
mod audit_text;
#[path = "suite/audit_triggers.rs"]
mod audit_triggers;
#[path = "suite/audit_upgrade.rs"]
mod audit_upgrade;
#[path = "suite/audit_view.rs"]
mod audit_view;
#[path = "suite/base_ref.rs"]
mod base_ref;
#[path = "suite/cli.rs"]
mod cli;
#[path = "suite/committed_tests.rs"]
mod committed_tests;
#[path = "suite/confirm_friction.rs"]
mod confirm_friction;
#[path = "suite/diff_mutants.rs"]
mod diff_mutants;
#[path = "suite/done_scope.rs"]
mod done_scope;
#[path = "suite/drift_fixes.rs"]
mod drift_fixes;
#[path = "suite/fix_enforcement.rs"]
mod fix_enforcement;
#[path = "suite/fix_export.rs"]
mod fix_export;
#[path = "suite/fix_gate.rs"]
mod fix_gate;
#[path = "suite/fix_runner.rs"]
mod fix_runner;
#[path = "suite/fix_strings.rs"]
mod fix_strings;
#[path = "suite/fix_text.rs"]
mod fix_text;
#[path = "suite/flaky_tree.rs"]
mod flaky_tree;
#[path = "suite/git_bin.rs"]
mod git_bin;
#[path = "suite/hook_stats.rs"]
mod hook_stats;
#[path = "suite/judge.rs"]
mod judge;
#[path = "suite/literal_paths.rs"]
mod literal_paths;
#[path = "suite/protected_growth.rs"]
mod protected_growth;
#[path = "suite/quiet_hooks.rs"]
mod quiet_hooks;
#[path = "suite/reach.rs"]
mod reach;
#[path = "suite/rerun_cache.rs"]
mod rerun_cache;
#[path = "suite/rerun_depends.rs"]
mod rerun_depends;
#[path = "suite/rerun_scope.rs"]
mod rerun_scope;
#[path = "suite/script_finder.rs"]
mod script_finder;
#[path = "suite/sealed_proofs.rs"]
mod sealed_proofs;
#[path = "suite/stop_scope.rs"]
mod stop_scope;
#[path = "suite/syntax_canonical.rs"]
mod syntax_canonical;
#[path = "suite/syntax_cli.rs"]
mod syntax_cli;
#[path = "suite/syntax_drift.rs"]
mod syntax_drift;
#[path = "suite/syntax_grammar.rs"]
mod syntax_grammar;
#[path = "suite/tamper_holes.rs"]
mod tamper_holes;
#[path = "suite/yolo.rs"]
mod yolo;
