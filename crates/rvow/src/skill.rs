//! The skill template, rendered at install time and checked against the command tree in tests.

use clap::Command;
#[cfg(test)]
use clap::CommandFactory;
use rvow_core::error::{Error, Result};

#[cfg(test)]
use crate::Cli;

const TEMPLATE: &str = include_str!("../../../skills/rvow/SKILL.md.tmpl");

/// Why `rvow <args>` is not a command this CLI has, or `None` when it is one.
#[cfg(test)]
pub(crate) fn problem(line: &str) -> Option<String> {
    problem_in(&built(), line)
}

/// The whole command tree, built once for every line a caller checks.
#[cfg(test)]
fn built() -> Command {
    let mut cmd = Cli::command();
    cmd.build();
    cmd
}

fn problem_in(root: &Command, line: &str) -> Option<String> {
    let line = line.trim().trim_start_matches(['!', '$']).trim();
    let mut words = line.split_whitespace();
    if words.next() != Some("rvow") {
        return None;
    }
    let mut cmd = root;
    let mut in_quote = false;
    let mut positional = false;
    for word in words {
        if in_quote {
            in_quote = !word.ends_with('"');
            continue;
        }
        if word.starts_with('"') {
            in_quote = !(word.len() > 1 && word.ends_with('"'));
            positional = true;
        } else if let Some(flag) = word.strip_prefix("--") {
            let flag = flag.split('=').next().unwrap_or_default();
            let known = cmd.get_arguments().any(|a| a.get_long() == Some(flag)) || ["toon", "human", "help"].contains(&flag);
            if !known {
                return Some(format!("`{line}`: `{}` has no --{flag}", cmd.get_name()));
            }
        } else if word.starts_with('-') || word.starts_with('<') || word.starts_with('[') {
            positional = true;
        } else if !positional && let Some(sub) = cmd.find_subcommand(word) {
            cmd = sub;
        } else if !positional && cmd.has_subcommands() {
            return Some(format!("`{line}`: `{}` has no subcommand {word}", cmd.get_name()));
        } else {
            positional = true;
        }
    }
    None
}

/// Fills the placeholders of `template`, checking each command against `root` when it is given.
fn render_text(template: &str, root: Option<&Command>) -> Result<String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find("}}").ok_or_else(|| Error::internal("the skill template has an unclosed {{"))?;
        let Some(args) = after[..end].strip_prefix("cmd:") else {
            return Err(Error::internal(format!("the skill template has an unknown placeholder {{{{{}}}}}", &after[..end])));
        };
        let line = format!("rvow {args}");
        if let Some(why) = root.and_then(|root| problem_in(root, &line)) {
            return Err(Error::internal(format!("the skill template names a missing command: {why}")));
        }
        out.push_str(&format!("`{line}`"));
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The skill text that `rvow install` writes, with every command placeholder filled in.
pub fn render() -> Result<String> {
    render_text(TEMPLATE, None)
}

/// The skill text with every command checked against the CLI; the check runs in tests only.
#[cfg(test)]
pub(crate) fn checked_render() -> Result<String> {
    checked_text(TEMPLATE)
}

#[cfg(test)]
fn checked_text(template: &str) -> Result<String> {
    render_text(template, Some(&built()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_skill_renders_with_real_commands() {
        let text = render().unwrap();
        assert!(!text.contains("{{"), "{text}");
        assert!(text.contains("`rvow prove --stale`"), "{text}");
        assert!(text.starts_with("---\nname: rvow\n"), "{text}");
    }

    #[test]
    fn the_skill_says_archive_reproves_stale_tasks() {
        let text = render().unwrap();
        assert!(text.contains("`rvow archive` re-proves stale tasks"), "{text}");
        assert!(text.contains("proved or stale, call the `rvow_archive` tool"), "{text}");
    }

    #[test]
    fn skill_description_names_the_trigger_words_and_fits_the_budget() {
        let text = render().unwrap();
        let description = text.lines().find(|l| l.starts_with("description: ")).unwrap().trim_start_matches("description: ");
        for word in ["plan", "propose", "spec", "accept", "prove"] {
            assert!(description.contains(word), "{word} missing from {description}");
        }
        assert!(description.len() < 600, "{} chars", description.len());
    }

    #[test]
    fn the_last_rule_names_the_exit_before_the_reason() {
        let text = render().unwrap();
        let section = text.split("## Stop and report").nth(1).unwrap();
        let block = section.trim_start();
        let exit = block.find("rvow block").unwrap();
        let reason = block.find("until the user confirms it").unwrap();
        assert!(exit < reason, "the exit must come first: {block}");
        assert!(!block.starts_with("Never"), "state the rule in the positive form: {block}");
    }

    #[test]
    fn the_always_on_text_fits_the_budget() {
        let skill = render().unwrap();
        assert!(skill.lines().count() < 40, "skill has {} lines", skill.lines().count());
        assert!(skill.len() < 4000, "skill has {} chars", skill.len());
        let description = skill.lines().find(|l| l.starts_with("description: ")).unwrap();
        assert!(description.len() < 600, "description has {} chars", description.len());
        // The brief cap itself is asserted by `fits` in rvow-gate/src/brief.rs;
        // here the worst-case constants are checked against the plan (W10).
        assert!(skill.contains("rvow block"), "the skill lost the legal exit");
    }

    #[test]
    fn the_vocabulary_has_one_sense_each() {
        let skill = render().unwrap();
        assert!(skill.contains("the vow is the accepted plan with its hashes"), "{skill}");
        assert!(!skill.contains("(a vow)"), "{skill}");
        let readme = include_str!("../../../README.md");
        assert!(readme.contains("The vow is the accepted plan with its hashes"), "{readme}");
    }

    #[test]
    fn mods_show_no_baseline_count() {
        // Accept runs no verify command, so neither mod counts baseline checks
        // nor sizes the accept wait from the task timeouts.
        for (name, text) in [
            ("claude", include_str!("../../../extensions/claude/hooks/register.tsx")),
            ("claude accept", include_str!("../../../extensions/claude/hooks/accept.tsx")),
            ("pi", include_str!("../../../extensions/pi/rvow/index.ts")),
            ("pi tools", include_str!("../../../extensions/pi/rvow/tools.ts")),
            ("pi typed tools", include_str!("../../../extensions/pi/rvow/typed-tools.ts")),
        ] {
            assert!(!text.contains("baseline checks"), "{name} still counts baseline checks");
            assert!(!text.contains("acceptTimeoutMs"), "{name} still sums the task timeouts");
        }
    }

    #[test]
    fn the_skill_says_to_write_the_test_first() {
        let text = render().unwrap();
        assert!(text.contains("Write the test first, then do the work and run `rvow prove <task>`"), "{text}");
        assert!(!text.contains("to record that the check fails"), "{text}");
    }

    #[test]
    fn the_skill_names_the_plan_parts_and_design() {
        let text = render().unwrap();
        assert!(text.contains("a `spec:<name>` per capability and an optional `design`"), "{text}");
        assert!(text.contains("rvow guide design"), "{text}");
        assert!(text.contains("rvow new"), "{text}");
        assert!(text.contains("rvow new --spec"), "{text}");
    }

    #[test]
    fn the_asking_sentence_is_shared_by_skill_and_tools() {
        let sentence = "Ask the open questions of the plan with the `rvow_ask` tool, and ask in chat only when the tool is missing";
        let skill = render().unwrap();
        assert!(skill.contains(sentence), "skill: {}", skill);
        let claude = include_str!("../../../extensions/claude/hooks/register.tsx");
        let pi = include_str!("../../../extensions/pi/rvow/index.ts");
        for (name, text) in [("claude", claude), ("pi", pi)] {
            assert!(text.contains("Ask in chat only when it is missing."), "{name} tool description lacks the rule");
            assert!(text.contains("one round of at most 3"), "{name} tool description lacks the round size");
        }
    }

    #[test]
    fn the_runtime_render_matches_the_checked_render() {
        assert_eq!(render().unwrap(), checked_render().unwrap());
    }

    #[test]
    fn a_missing_command_or_placeholder_fails_the_render() {
        assert!(checked_text("{{cmd:nope}}").is_err());
        assert!(checked_text("{{cmd:prove --nope}}").is_err());
        assert!(render_text("{{what:prove}}", None).is_err());
        assert!(render_text("{{cmd:prove", None).is_err());
        assert_eq!(render_text("a {{cmd:prove <task>}} b", None).unwrap(), "a `rvow prove <task>` b");
    }

    #[test]
    fn a_flag_that_does_not_exist_is_reported() {
        assert!(problem("rvow prove --nope").is_some());
        assert!(problem("rvow nope").is_some());
        assert!(problem("rvow prove --stale").is_none());
        assert!(problem("! rvow amend --unlock <path> --reason \"a --b c\"").is_none());
    }
}
