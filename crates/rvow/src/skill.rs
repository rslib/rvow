//! The skill template, rendered from the real command tree at install time.

use clap::{Command, CommandFactory};
use rvow_core::error::{Error, Result};

use crate::Cli;

const TEMPLATE: &str = include_str!("../../../skills/rvow/SKILL.md.tmpl");

/// Why `rvow <args>` is not a command this CLI has, or `None` when it is one.
pub fn problem(line: &str) -> Option<String> {
    let line = line.trim().trim_start_matches(['!', '$']).trim();
    let mut words = line.split_whitespace();
    if words.next() != Some("rvow") {
        return None;
    }
    let mut cmd: Command = Cli::command();
    cmd.build();
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
        } else if !positional && let Some(sub) = cmd.find_subcommand(word).cloned() {
            cmd = sub;
        } else if !positional && cmd.has_subcommands() {
            return Some(format!("`{line}`: `{}` has no subcommand {word}", cmd.get_name()));
        } else {
            positional = true;
        }
    }
    None
}

fn render_text(template: &str) -> Result<String> {
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
        if let Some(why) = problem(&line) {
            return Err(Error::internal(format!("the skill template names a missing command: {why}")));
        }
        out.push_str(&format!("`{line}`"));
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The skill text that `rvow install` writes, with every command checked against the CLI.
pub fn render() -> Result<String> {
    render_text(TEMPLATE)
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
    fn a_missing_command_or_placeholder_fails_the_render() {
        assert!(render_text("{{cmd:nope}}").is_err());
        assert!(render_text("{{cmd:prove --nope}}").is_err());
        assert!(render_text("{{what:prove}}").is_err());
        assert!(render_text("{{cmd:prove").is_err());
        assert_eq!(render_text("a {{cmd:prove <task>}} b").unwrap(), "a `rvow prove <task>` b");
    }

    #[test]
    fn a_flag_that_does_not_exist_is_reported() {
        assert!(problem("rvow prove --nope").is_some());
        assert!(problem("rvow nope").is_some());
        assert!(problem("rvow prove --stale").is_none());
        assert!(problem("! rvow amend --unlock <path> --reason \"a --b c\"").is_none());
    }
}
