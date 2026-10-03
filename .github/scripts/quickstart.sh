#!/usr/bin/env bash
# Runs the README quickstart in a throwaway repository and home. Needs `rvow` on PATH.
set -euo pipefail
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
export HOME=$tmp/home RVOW_HOME=$tmp/rvow-home
unset XDG_CONFIG_HOME XDG_DATA_HOME
mkdir -p "$HOME" "$tmp/repo"
cd "$tmp/repo"
git init -q
git -c user.name=ci -c user.email=ci@example.invalid -c commit.gpgsign=false commit -q --allow-empty -m init
rvow init
rvow new "Print a greeting"
out=$(rvow ls --human)
echo "$out"
echo "$out" | grep -Eq '^CHG-[0-9a-z]+ +backlog +[^ ]+ Print a greeting$'
rvow validate --human
