//! The integration tests of the rvow crate, built as one binary. Each module is one former test file; the files live in suite/.

#[path = "suite/accept_speed.rs"]
mod accept_speed;
#[path = "suite/agent_mode.rs"]
mod agent_mode;
#[path = "suite/alloc.rs"]
mod alloc;
#[path = "suite/any_tool_change.rs"]
mod any_tool_change;
#[path = "suite/archive_reprove.rs"]
mod archive_reprove;
#[path = "suite/attest.rs"]
mod attest;
#[path = "suite/audit_build.rs"]
mod audit_build;
#[path = "suite/audit_ci.rs"]
mod audit_ci;
#[path = "suite/audit_claude_ci.rs"]
mod audit_claude_ci;
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
#[path = "suite/audit_hook_ci.rs"]
mod audit_hook_ci;
#[path = "suite/audit_integration.rs"]
mod audit_integration;
#[path = "suite/audit_modules.rs"]
mod audit_modules;
#[path = "suite/audit_nextest.rs"]
mod audit_nextest;
#[path = "suite/audit_paths.rs"]
mod audit_paths;
#[path = "suite/audit_pi_dock.rs"]
mod audit_pi_dock;
#[path = "suite/audit_pi_tools.rs"]
mod audit_pi_tools;
#[path = "suite/audit_proof.rs"]
mod audit_proof;
#[path = "suite/audit_readable.rs"]
mod audit_readable;
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
#[path = "suite/baseline_at_prove.rs"]
mod baseline_at_prove;
#[path = "suite/brief_once.rs"]
mod brief_once;
#[path = "suite/cli.rs"]
mod cli;
#[path = "suite/committed_tests.rs"]
mod committed_tests;
#[path = "suite/confirm_friction.rs"]
mod confirm_friction;
#[path = "suite/delegate_worktree.rs"]
mod delegate_worktree;
#[path = "suite/diff_mutants.rs"]
mod diff_mutants;
#[path = "suite/docs_reference.rs"]
mod docs_reference;
#[path = "suite/done_scope.rs"]
mod done_scope;
#[path = "suite/editor_snapshot.rs"]
mod editor_snapshot;
#[path = "suite/first_edit_notice.rs"]
mod first_edit_notice;
#[path = "suite/fix_enforcement.rs"]
mod fix_enforcement;
#[path = "suite/fix_export.rs"]
mod fix_export;
#[path = "suite/fix_gate.rs"]
mod fix_gate;
#[path = "suite/fix_strings.rs"]
mod fix_strings;
#[path = "suite/fix_text.rs"]
mod fix_text;
#[path = "suite/gate_failure.rs"]
mod gate_failure;
#[path = "suite/gate_lock.rs"]
mod gate_lock;
#[path = "suite/gate_speed.rs"]
mod gate_speed;
#[path = "suite/gate_tools.rs"]
mod gate_tools;
#[path = "suite/gate_unseen.rs"]
mod gate_unseen;
#[path = "suite/gate_writes.rs"]
mod gate_writes;
#[path = "suite/git_bin.rs"]
mod git_bin;
#[path = "suite/hook_calls.rs"]
mod hook_calls;
#[path = "suite/hook_git_count.rs"]
mod hook_git_count;
#[path = "suite/hook_json.rs"]
mod hook_json;
#[path = "suite/hook_stats.rs"]
mod hook_stats;
#[path = "suite/install_all.rs"]
mod install_all;
#[path = "suite/judge.rs"]
mod judge;
#[path = "suite/land.rs"]
mod land;
#[path = "suite/land_conflict.rs"]
mod land_conflict;
#[path = "suite/land_merge.rs"]
mod land_merge;
#[path = "suite/lease_bind_worktree.rs"]
mod lease_bind_worktree;
#[path = "suite/lease_brief.rs"]
mod lease_brief;
#[path = "suite/lease_worktree.rs"]
mod lease_worktree;
#[path = "suite/lease_worktree_rules.rs"]
mod lease_worktree_rules;
#[path = "suite/literal_paths.rs"]
mod literal_paths;
#[path = "suite/plan_access.rs"]
mod plan_access;
#[path = "suite/prove_stale.rs"]
mod prove_stale;
#[path = "suite/provenance.rs"]
mod provenance;
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
#[path = "suite/revalidate.rs"]
mod revalidate;
#[path = "suite/sealed_proofs.rs"]
mod sealed_proofs;
#[path = "suite/stacked_gate.rs"]
mod stacked_gate;
#[path = "suite/stop_scope.rs"]
mod stop_scope;
#[path = "suite/syntax_cli.rs"]
mod syntax_cli;
#[path = "suite/tamper_holes.rs"]
mod tamper_holes;
#[path = "suite/temp.rs"]
mod temp;
#[path = "suite/verify_runners.rs"]
mod verify_runners;
#[path = "suite/worktrees.rs"]
mod worktrees;
#[path = "suite/yolo.rs"]
mod yolo;

/// Tests in this binary share the process env, so every one that sets an
/// env var holds this lock: alloc sets HOME and RVOW_HOME.
pub(crate) mod env_lock {
    pub use rvow_core::git::testrepo::env_lock as lock;
}
