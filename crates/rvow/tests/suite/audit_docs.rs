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
        let _ = std::fs::remove_dir_all(&self.dir);
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
fn docs_name_the_baseline_first_rule() {
    let how = doc("docs/how-it-works.md");
    assert!(how.contains("`baseline-first`") && how.contains("denies it once"), "how-it-works lacks the baseline-first rule");
    assert!(how.contains("of every active change on the branch"), "how-it-works lacks the branch-wide proved scope");
    assert!(!how.contains("gives the agent one line that names"), "how-it-works still names the removed notice");
    let log = doc("CHANGELOG.md");
    let unreleased = log.split("## [").nth(1).unwrap_or_default();
    assert!(unreleased.contains("`baseline-first`"), "the Unreleased changelog lacks the baseline-first rule");
}

#[test]
fn docs_name_the_warm_target() {
    let settings = doc("docs/settings.md");
    assert!(settings.contains("its own cargo build folder"), "settings lacks the warm worktree's own build folder");
    assert!(!settings.contains("share one build folder between worktrees"), "settings still says to share one build folder");
    let log = doc("CHANGELOG.md");
    let unreleased = log.split("## [").nth(1).unwrap_or_default();
    assert!(unreleased.contains("its own cargo build folder"), "the Unreleased changelog lacks the warm build folder");
    assert!(!unreleased.contains("<shared dir>"), "the Unreleased changelog still names a shared build folder");
}
