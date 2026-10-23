//! The fixed guide text of `rvow guide <artifact>`, at most 400 words each.

pub const PROPOSAL: &str = include_str!("../../../guides/proposal.md");
pub const SPEC: &str = include_str!("../../../guides/spec.md");
pub const TASKS: &str = include_str!("../../../guides/tasks.md");
pub const DESIGN: &str = include_str!("../../../guides/design.md");
pub const JUDGE: &str = include_str!("../../../guides/judge.md");
pub const RUNNERS: &str = include_str!("../../../guides/runners.md");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change;
    use crate::git::testrepo::TempDir;

    const MAX_WORDS: usize = 400;

    fn example(guide: &str) -> &str {
        let (_, rest) = guide.split_once("<!-- example -->\n").unwrap();
        rest.split_once("<!-- /example -->").unwrap().0
    }

    #[test]
    fn task_guide_names_each_sizing_warning_and_the_gate_task() {
        use crate::diag::Rule;
        for rule in [Rule::LongTask, Rule::HotFile, Rule::DependsChain, Rule::BroadVerify] {
            assert!(TASKS.contains(&format!("(`{}`)", rule.code())), "{}", rule.code());
        }
        assert!(TASKS.contains("last gate task"));
    }

    #[test]
    fn task_guide_skill_sends_the_agent_to_the_sizing_rules() {
        let skill = include_str!("../../../skills/rvow/SKILL.md.tmpl");
        assert!(skill.contains("{{cmd:guide tasks}}"));
        assert!(skill.contains("gate task"));
    }

    #[test]
    fn runners_guide_fits_the_budget_and_is_ascii() {
        let n = RUNNERS.split_whitespace().count();
        assert!(n <= MAX_WORDS, "runners guide has {n} words");
        assert!(RUNNERS.is_ascii());
    }

    #[test]
    fn runners_guide_example_parses_and_fills_build_and_artifacts() {
        #[derive(serde::Deserialize)]
        struct Table {
            runners: std::collections::BTreeMap<String, crate::template::Runner>,
        }
        let table: Table = toml::from_str(example(RUNNERS)).unwrap();
        let task = crate::tasks::parse_tasks("- TSK-x7k2q4 T\n  verify: @cpp target=pool_test test=PoolShutdown\n  files: a\n").0.remove(0);
        let plan = crate::template::plan(&table.runners, &task).unwrap();
        assert_eq!(plan.build.as_deref(), Some("cmake --build build --target pool_test"));
        assert_eq!(plan.artifacts, ["build/tests/pool_test".to_string()]);
    }

    #[test]
    fn runners_guide_says_when_it_does_not_fit_and_names_the_sharing_keys() {
        assert!(RUNNERS.contains("hashed") && RUNNERS.contains("cargo"), "the guide lacks the Rust no-fit note");
        for key in ["`always`", "`extends`", "glob"] {
            assert!(RUNNERS.contains(key), "the guide lacks {key}");
        }
    }

    #[test]
    fn runners_guide_is_named_by_the_tasks_guide_and_the_skill() {
        assert!(TASKS.contains("rvow guide runners"));
        let skill = include_str!("../../../skills/rvow/SKILL.md.tmpl");
        assert!(skill.contains("{{cmd:guide runners}}"));
    }

    /// The guides and the lint quote the same numbers.
    #[test]
    fn guide_numbers_match_the_lint() {
        let cap = format!("at most {} in the change", crate::lint::MAX_CLARIFICATIONS);
        assert!(PROPOSAL.contains(&cap), "proposal guide lacks `{cap}`");
        let round = format!("at most {} questions per round", crate::interview::ROUND);
        assert!(PROPOSAL.contains(&round), "proposal guide lacks `{round}`");
        assert!(TASKS.contains("rvow takes the baseline from the code at accept"), "tasks guide lacks the baseline rule");
        assert!(PROPOSAL.contains("- `branch:` names the git branch"), "proposal guide does not explain branch:");
        assert!(SPEC.contains("Name the visible behavior, not files or code"), "spec guide keeps a Do-not bullet");
        assert!(DESIGN.contains("At most 100 lines"), "design guide cap moved from 100");
        assert!(!DESIGN.contains("300"), "design guide still says 300");
    }

    #[test]
    fn every_guide_fits_the_word_budget() {
        for (name, text) in [("proposal", PROPOSAL), ("spec", SPEC), ("tasks", TASKS), ("design", DESIGN), ("judge", JUDGE)] {
            let n = text.split_whitespace().count();
            assert!(n <= MAX_WORDS, "{name} guide has {n} words");
            assert!(text.is_ascii(), "{name} guide is not ASCII");
        }
    }

    #[test]
    fn the_examples_make_one_change_that_validates() {
        let tmp = TempDir::new();
        std::fs::create_dir_all(tmp.0.join("specs")).unwrap();
        for (file, guide) in [("proposal.md", PROPOSAL), ("tasks.md", TASKS), ("design.md", DESIGN), ("specs/ledger.md", SPEC)] {
            std::fs::write(tmp.0.join(file), example(guide)).unwrap();
        }
        let loaded = change::load(&tmp.0, &[]).unwrap();
        assert!(loaded.findings.is_empty(), "{:?}", loaded.findings);
        assert_eq!(loaded.tasks.len(), 1);
        assert_eq!(loaded.deltas.len(), 1);
        assert!(example(PROPOSAL).split_whitespace().count() <= 150);
        assert!(example(DESIGN).lines().count() <= change::MAX_DESIGN_LINES);
    }
}
