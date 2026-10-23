//! Every file and folder name in the rvow tree, so no crate has to reach up for
//! one.

// The project tree.
pub const PROJECT_FILE: &str = "project.toml";
pub const CONFIG_FILE: &str = "config.toml";
pub const IDS_FILE: &str = "ids.jsonl";
pub const LOCK_FILE: &str = ".lock";
/// The local index in the rvow home from a repository to its project.
pub const INDEX_FILE: &str = "project-index.json";
/// The git common dirs whose pre-push gate `rvow install` wrote, local to one machine.
pub const GATE_REPOS_FILE: &str = "gate-repos.json";
/// The folder in the rvow home that holds one warm worktree per project.
pub const WORKTREES_DIR: &str = "worktrees";
pub const REPO_DIR: &str = "rvow";
pub const RECORD_FILE: &str = "spec-record.json";
pub const YOLO_FILE: &str = "yolo.json";
pub const LEASE_FILE: &str = "leases.jsonl";
pub const STATS_FILE: &str = "hook-stats.jsonl";
/// What the harness saw the user make, kept in the project.
pub const WITNESS_FILE: &str = "witness.json";
/// Where a full stats file moves, under the local-only `hook-state/` folder.
pub const ROTATED_FILE: &str = "hook-state/hook-stats.old.jsonl";
pub const CACHE_FILE: &str = "rerun-cache.jsonl";

// A change folder.
pub const PROPOSAL: &str = "proposal.md";
pub const DESIGN: &str = "design.md";
pub const TASKS: &str = "tasks.md";
pub const DELTAS: &str = "specs";
pub const LEDGER: &str = "ledger.jsonl";

// The rvow home.
pub const MARKETPLACE_DIR: &str = "claude-plugin";
pub const KEY_FOLDER: &str = "hidden-checks";
pub const KEY_FILE: &str = ".cache-key";
