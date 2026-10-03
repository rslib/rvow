#!/usr/bin/env bash
# Runs `rvow install` in a throwaway home and repository and checks what it writes. Needs `rvow` on PATH.
set -euo pipefail
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
export HOME=$tmp/home RVOW_HOME=$tmp/rvow-home CLAUDE_CONFIG_DIR=$tmp/home/.claude
unset XDG_CONFIG_HOME XDG_DATA_HOME
mkdir -p "$HOME" "$tmp/repo"
cd "$tmp/repo"
git init -q
git -c user.name=ci -c user.email=ci@example.invalid -c commit.gpgsign=false commit -q --allow-empty -m init
rvow init --human

rvow install --claude settings --human
skill=$CLAUDE_CONFIG_DIR/skills/rvow/SKILL.md
test -f "$skill"
if grep -q '{{' "$skill"; then
  echo "the installed skill has an unrendered placeholder"
  exit 1
fi
grep -q '`rvow prove --stale`' "$skill"
grep -q rvow "$CLAUDE_CONFIG_DIR/settings.json"
test -x "$(git rev-parse --git-path hooks/pre-push)"
rvow install --claude settings --human | grep -q 'up to date'

rvow install --claude mod --human
test -f "$RVOW_HOME/claude-plugin/.claude-plugin/marketplace.json"
test -f "$RVOW_HOME/claude-plugin/plugins/rvow/skills/rvow/SKILL.md"
