use rvow_core::config;
use rvow_core::store::{Meta, Mode, Project};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn doc(path: &str) -> String {
    std::fs::read_to_string(root().join(path)).unwrap()
}

fn toml_example(path: &str) -> String {
    doc(path).split_once("```toml\n").unwrap().1.split_once("```").unwrap().0.to_string()
}

struct Fixture {
    dir: PathBuf,
    home: PathBuf,
    project: Project,
}

impl Fixture {
    fn new(mode: Mode) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("rvow-audit-docs-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let home = dir.join("home");
        let project = Project { root: dir.join("project"), meta: Meta { roots: vec![], remote: None, mode } };
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&project.root).unwrap();
        Self { dir, home, project }
    }

    fn load(&self, local: &str, global: &str) -> rvow_core::error::Result<config::Config> {
        std::fs::write(self.project.root.join("config.toml"), local).unwrap();
        std::fs::write(self.home.join("config.toml"), global).unwrap();
        config::load(&self.project, &self.home)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        rvow_core::git::testrepo::remove_tree(&self.dir);
    }
}

#[test]
fn audit_docs_settings_example_and_global_only_keys() {
    let example = toml_example("docs/settings.md");
    let f = Fixture::new(Mode::Home);
    let cfg = f.load(&example, "").unwrap();
    assert_eq!(cfg.interview_cap, 3);
    assert!(!cfg.require_signature);
    assert_eq!(cfg.timeout_secs, 600);
    for key in ["machine", "signing_key", "agent"] {
        let setting = format!("{key} = \"example\"\n");
        let err = f.load(&setting, "").unwrap_err();
        assert_eq!(err.code, "config");
        assert!(err.message.contains("global config only"));
        let global = f.load("", &setting).unwrap();
        assert!(!global.canonical().unwrap().contains("example"));
    }
    for cap in [1, 3] {
        assert_eq!(f.load(&format!("interview_cap = {cap}\n"), "").unwrap().interview_cap, cap);
    }
    for cap in [0, 4] {
        assert_eq!(f.load(&format!("interview_cap = {cap}\n"), "").unwrap_err().code, "config");
    }
}

#[test]
fn audit_docs_context_example_merge_and_byte_limit() {
    let example = toml_example("docs/authoring-context.md");
    let f = Fixture::new(Mode::Home);
    let cfg = f.load(&example, "context = \"old\"\n[rules]\nproposal = \"old rule\"\n").unwrap();
    assert!(cfg.context.contains("library owns validation"));
    assert_eq!(cfg.rules.proposal.as_deref(), Some("Name the user-visible change."));
    let inherited = f.load("[rules]\nproposal = \"\"\n", &example).unwrap();
    assert_eq!(inherited.context, cfg.context);
    assert_eq!(inherited.rules.proposal.as_deref(), Some(""));
    assert_eq!(inherited.rules.spec, cfg.rules.spec);
    for prefix in ["context = ", "[rules]\nproposal = "] {
        let at_limit = "x".repeat(config::AUTHORING_BYTES);
        assert!(f.load(&format!("{prefix}\"{at_limit}\"\n"), "").is_ok());
        let too_long = format!("{at_limit}x");
        assert_eq!(f.load(&format!("{prefix}\"{too_long}\"\n"), "").unwrap_err().code, "config");
    }
    assert!(doc("docs/authoring-context.md").contains(&format!("{} UTF-8 bytes", config::AUTHORING_BYTES)));
    let repo = Fixture::new(Mode::Repo);
    let cfg = repo.load("", &example).unwrap();
    assert!(cfg.context.is_empty());
    assert_eq!(cfg.rules, config::Rules::default());
}

#[test]
fn audit_docs_readme_and_doc_links_resolve() {
    let mut paths = vec![root().join("README.md")];
    paths.extend(
        std::fs::read_dir(root().join("docs")).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "md")),
    );
    let mut checked = 0;
    for path in paths {
        let text = std::fs::read_to_string(&path).unwrap();
        for link in text.split("](").skip(1) {
            let target = link.split(')').next().unwrap().split('#').next().unwrap();
            if target.is_empty() || target.contains("://") || target.starts_with("mailto:") {
                continue;
            }
            assert!(path.parent().unwrap().join(target).exists(), "{} links to missing {target}", path.display());
            checked += 1;
        }
    }
    assert!(checked >= 6, "README must route to the docs rather than duplicate the manual");
}

#[test]
fn audit_docs_guide_examples_still_parse_as_one_change() {
    let guides = [rvow_core::guide::PROPOSAL, rvow_core::guide::TASKS, rvow_core::guide::SPEC];
    let examples: Vec<_> =
        guides.iter().map(|guide| guide.split_once("<!-- example -->\n").unwrap().1.split_once("<!-- /example -->").unwrap().0).collect();
    let (tasks, diags) = rvow_core::tasks::parse_tasks(examples[1]);
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(tasks.len(), 1);
    assert!(!tasks[0].files.is_empty());
    assert!(tasks[0].verify_command().is_ok());
    assert!(examples[0].contains("branch:"));
    assert!(examples[2].contains(&tasks[0].covers[0].to_string()));
}

#[test]
fn docs_name_the_archive_confirm() {
    let how = doc("docs/how-it-works.md");
    assert!(
        how.contains("confirms every pending amendment of the change once, in the archive dialog"),
        "how-it-works lacks the archive confirm"
    );
    assert!(how.contains("`rvow amend --unlock`") && how.contains("its own confirm"), "how-it-works lacks the unlock exception");
    let log = doc("CHANGELOG.md");
    let unreleased = log.split("## [").nth(1).unwrap_or_default();
    assert!(unreleased.contains("archive dialog"), "the Unreleased changelog lacks the archive confirm");
}

#[test]
fn docs_name_the_brief_once_rule() {
    let how = doc("docs/how-it-works.md");
    assert!(how.contains("sends the same brief once per session"), "how-it-works lacks the brief once rule");
    assert!(how.contains("`compact`") && how.contains("`clear`"), "how-it-works lacks the compact and clear rule");
    let log = doc("CHANGELOG.md");
    let unreleased = log.split("## [").nth(1).unwrap_or_default();
    assert!(unreleased.contains("brief once per session"), "the Unreleased changelog lacks the brief rule");
}

#[test]
fn docs_name_the_quiet_brief() {
    let how = doc("docs/how-it-works.md");
    assert!(how.contains("sends nothing when only the counts changed"), "how-it-works lacks the count rule");
    assert!(how.contains("only the brief from the `next:` line"), "how-it-works lacks the next task rule");
    assert!(how.contains("names its leased task as `next:`"), "how-it-works lacks the leased brief");
    assert!(how.contains("one block that lists every open item"), "how-it-works lacks the one stop block");
    let log = doc("CHANGELOG.md");
    let unreleased = log.split("## [").nth(1).unwrap_or_default();
    assert!(unreleased.contains("only the count of tasks changed"), "the Unreleased changelog lacks the quiet brief");
    assert!(unreleased.contains("one stop block"), "the Unreleased changelog lacks the one stop block");
}

#[test]
fn docs_name_the_snapshot_lock() {
    let how = doc("docs/how-it-works.md");
    assert!(how.contains("takes the snapshot lock of its worktree, not the project lock"), "how-it-works lacks the snapshot lock");
    assert!(how.contains("names its files hashes only those files"), "how-it-works lacks the named-file snapshot");
    let settings = doc("docs/settings.md");
    assert!(settings.contains("`xcrun --find git`"), "settings lacks the real git");
    let log = doc("CHANGELOG.md");
    let unreleased = log.split("## [").nth(1).unwrap_or_default();
    assert!(unreleased.contains("snapshot lock of the worktree"), "the Unreleased changelog lacks the snapshot lock");
}

#[test]
fn docs_name_the_worktree_lease() {
    let how = doc("docs/how-it-works.md");
    assert!(how.contains("`rvow delegate <task> --worktree`"), "how-it-works lacks the worktree flag");
    assert!(how.contains("binds the session to the lease by its session id"), "how-it-works lacks the worktree bind");
    assert!(how.contains("A worktree lease has no time limit"), "how-it-works lacks the worktree lease life");
    let log = doc("CHANGELOG.md");
    let unreleased = log.split("## [").nth(1).unwrap_or_default();
    assert!(unreleased.contains("--worktree [path]"), "the Unreleased changelog lacks the worktree flag");
}

#[test]
fn docs_name_the_land() {
    let how = doc("docs/how-it-works.md");
    assert!(how.contains("`rvow land <change>`"), "how-it-works lacks rvow land");
    assert!(how.contains("`--merge` makes one merge commit per task"), "how-it-works lacks the merge mode");
    assert!(how.contains("When the gate fails, it moves nothing"), "how-it-works lacks the gate rule");
    let log = doc("CHANGELOG.md");
    let unreleased = log.split("## [").nth(1).unwrap_or_default();
    assert!(unreleased.contains("`rvow land <change or task>`"), "the Unreleased changelog lacks rvow land");
}

#[test]
fn docs_name_the_automatic_baseline() {
    let how = doc("docs/how-it-works.md");
    assert!(how.contains("## The baseline: the code at accept"), "how-it-works lacks the automatic baseline");
    assert!(!how.contains("baseline-first") && !how.contains("baseline pending"), "how-it-works still names the removed rule");
    let settings = doc("docs/settings.md");
    assert!(!settings.contains("warm worktree"), "settings still names the warm worktree");
    let log = doc("CHANGELOG.md");
    let unreleased = log.split("## [").nth(1).unwrap_or_default();
    assert!(
        unreleased.contains("Take the baseline of a task from the code at accept"),
        "the Unreleased changelog lacks the baseline entry"
    );
    assert!(!unreleased.contains("`baseline-first`"), "the Unreleased changelog still names the removed rule");
}

#[test]
fn audit_docs_names_the_alloc_budgets() {
    let agents = doc("AGENTS.md");
    assert!(agents.contains("`crates/rvow/tests/suite/alloc.rs` holds an allocation budget per hook"), "AGENTS.md lacks the alloc budgets");
    assert!(agents.contains("never raise one to make a test pass"), "AGENTS.md lacks the budget rule");
    assert!(agents.contains("load specs with `change::living` (no lint)"), "AGENTS.md lacks the hot-path rule");
}
