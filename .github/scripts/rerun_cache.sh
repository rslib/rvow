#!/usr/bin/env bash
# Runs the gate rerun three times in a throwaway repository and home and checks that the
# second run is answered from the cache and a changed file runs the task again. Needs `rvow` on PATH.
set -euo pipefail
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
export HOME=$tmp/home RVOW_HOME=$tmp/rvow-home
unset XDG_CONFIG_HOME XDG_DATA_HOME
mkdir -p "$HOME" "$tmp/repo/src"
cd "$tmp/repo"
ci_git() { git -c user.name=ci -c user.email=ci@example.invalid -c commit.gpgsign=false "$@"; }
field() { grep -E "^$1: " | head -n 1 | sed -E "s/^$1: *//"; }
git init -q -b main
echo a > src/a.txt
git add src
ci_git commit -q -m init
rvow init > /dev/null
made=$(rvow new --spec "Write ok" --toon)
id=$(echo "$made" | field created)
dir=$(echo "$made" | field path)
cat > "$dir/specs/out.md" << 'EOF'
## ADD REQ-new1 Write ok

rvow SHALL write ok.

### Scenario: s
- WHEN it runs
- THEN out.txt holds ok
EOF
cat > "$dir/tasks.md" << EOF
- TSK-new1 Write ok
  covers: REQ-new1
  verify: grep -q ok out.txt && echo run >> "$tmp/count" && echo 'test result: ok. 1 passed; 0 failed;'
  files: out.txt, src/**
EOF
rvow accept "$id" --yes > /dev/null
active=$(rvow path "$id" --toon | field path)
task=$(sed -nE 's/^- (TSK-[0-9a-z]+) .*/\1/p' "$active/tasks.md")
echo ok > out.txt
git add out.txt
ci_git commit -q -m out
rvow prove "$task" > /dev/null
rm -f "$tmp/count"
runs() { if [ -f "$tmp/count" ]; then wc -l < "$tmp/count" | tr -d ' '; else echo 0; fi; }

first=$(rvow gate --rerun --toon)
echo "$first" | grep -q 'passed: true'
[ "$(runs)" = 1 ]

second=$(rvow gate --rerun --toon)
echo "$second"
echo "$second" | grep -q 'passed: true'
echo "$second" | grep -Fq 'cached[1]'
[ "$(runs)" = 1 ]

forced=$(rvow gate --rerun --no-cache --toon)
echo "$forced" | grep -q 'passed: true'
! echo "$forced" | grep -Fq 'cached'
[ "$(runs)" = 2 ]

echo b > src/a.txt
ci_git commit -q -am change
rvow prove "$task" > /dev/null
rm -f "$tmp/count"
third=$(rvow gate --rerun --toon)
echo "$third" | grep -q 'passed: true'
! echo "$third" | grep -Fq 'cached'
[ "$(runs)" = 1 ]
echo "rerun cache: ok"
