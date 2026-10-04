//! The fixed guide text of `rvow guide <artifact>`, at most 400 words each.

pub const PROPOSAL: &str = include_str!("../../../guides/proposal.md");
pub const SPEC: &str = include_str!("../../../guides/spec.md");
pub const TASKS: &str = include_str!("../../../guides/tasks.md");
pub const DESIGN: &str = include_str!("../../../guides/design.md");

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
    fn every_guide_fits_the_word_budget() {
        for (name, text) in [("proposal", PROPOSAL), ("spec", SPEC), ("tasks", TASKS), ("design", DESIGN)] {
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
