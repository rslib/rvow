//! Shell forms that once got past the Bash rule: `-c` text in fish and ash,
//! `rgit` in place of `git`, `git -c` config tricks and awk programs.

use rvow_core::hook::{Rule, bash_rule};

fn rule(cmd: &str) -> Option<Rule> {
    bash_rule(cmd).map(|d| d.rule)
}

#[test]
fn shell_fish_and_ash_inline_text_is_judged() {
    assert_eq!(rule("fish -c 'rvow accept CHG-x --yes'"), Some(Rule::OwnerCommand));
    assert_eq!(rule("fish --command='rvow confirm --accept --all'"), Some(Rule::OwnerCommand));
    assert_eq!(rule("ash -c 'git push --no-verify'"), Some(Rule::GitHooks));
    assert_eq!(rule("/usr/local/bin/fish -c 'git commit -n -m x'"), Some(Rule::GitHooks));
    assert_eq!(rule("fish -c 'ls'"), None);
}

#[test]
fn shell_rgit_is_treated_as_git() {
    assert_eq!(rule("rgit push --no-verify"), Some(Rule::GitHooks));
    assert_eq!(rule("rgit commit -n -m x"), Some(Rule::GitHooks));
    assert_eq!(rule("rgit -c core.hooksPath=/dev/null commit -m x"), Some(Rule::GitHooks));
    assert_eq!(rule("rgit status --short"), None);
    assert_eq!(rule("rgit commit -m 'mention -n and --no-verify'"), None);
}

#[test]
fn shell_git_config_aliases_that_run_a_shell_or_skip_hooks_are_denied() {
    assert_eq!(rule("git -c alias.x='!rvow accept CHG-x --yes' x"), Some(Rule::GitHooks));
    assert_eq!(rule("git -c 'alias.x=!sh -c true' x"), Some(Rule::GitHooks));
    assert_eq!(rule("git -c alias.ci='commit -n' ci -m x"), Some(Rule::GitHooks));
    assert_eq!(rule("git -c alias.p='push --no-verify' p"), Some(Rule::GitHooks));
    assert_eq!(rule("rgit -c alias.x='!true' x"), Some(Rule::GitHooks));
    assert_eq!(rule("git -c alias.st=status st"), None);
}

#[test]
fn shell_awk_programs_are_judged_as_script_text() {
    assert_eq!(rule("awk 'BEGIN{system(\"rvow accept CHG-x --yes\")}'"), Some(Rule::OwnerCommand));
    assert_eq!(rule("gawk 'BEGIN{system(\"rvow confirm --accept --all\")}'"), Some(Rule::OwnerCommand));
    assert_eq!(rule("mawk 'BEGIN{system(\"rvow accept CHG-x\")}'"), Some(Rule::OwnerCommand));
    assert_eq!(rule("awk '{print $1}' notes.txt"), None);
}
