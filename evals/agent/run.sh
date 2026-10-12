#!/usr/bin/env bash
# Does an agent with the rvow Claude mod (skill, hooks) follow the accepted
# plan? Installs the mod with the built binary into a throwaway home, runs
# each case under evals/agent/cases/ with the mod and with no mod on the
# model given (default haiku), and writes the report to evals/agent/results/.
#
#   evals/agent/run.sh [model] [extra claude plugin eval flags]
#
# Uses $RVOW (default target/debug/rvow); builds nothing. Each run is a real
# model call: a few cents per case on haiku with --runs 1, about $5 at most
# for a full run through --max-cost-usd. Nothing is written to your home,
# your rvow store or your Claude folder: HOME, RVOW_HOME and the Claude
# config dir all point under a mktemp dir (removed on exit) for the install,
# and the eval target is that throwaway plugin by path.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
rvow=${RVOW:-$root/target/debug/rvow}
rvow=$(cd "$(dirname "$rvow")" && pwd)/$(basename "$rvow")
model=${1:-haiku}
shift || true

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
rvow_home=$tmp/home/.local/share/rvow
# Throwaway home for the install: the mod lands at
# $RVOW_HOME/claude-plugin/plugins/rvow, and the Claude registration stays
# under the temp dir too, so the user's store and Claude folder are untouched.
# The cd keeps the pre-push gate out of the real repository.
(cd "$tmp" && HOME=$tmp/home XDG_DATA_HOME=$tmp/home/.local/share XDG_CONFIG_HOME=$tmp/home/.config CLAUDE_CONFIG_DIR=$tmp/claude RVOW_HOME=$rvow_home "$rvow" install >/dev/null)
plugin=$rvow_home/claude-plugin/plugins/rvow
cp -R "$here/cases" "$plugin/evals"
bindir=$(dirname "$rvow")
sq() { local s=${1//\'/\'\\\'\'}; printf "'%s'" "$s"; }
for case in "$plugin"/evals/*/; do
  cp "$here/lib.sh" "$case/lib.sh"
  # The scaffold env may be cleaned, so bake the absolute paths into the
  # copied setup script as well as exporting them for the agent's shell.
  {
    head -n 1 "$case/setup.sh"
    printf 'RVOW_BIN=%s\nRVOW_HOME=%s\nPATH=%s:"$PATH"\nexport RVOW_BIN RVOW_HOME PATH\n' "$(sq "$rvow")" "$(sq "$rvow_home")" "$(sq "$bindir")"
    tail -n +2 "$case/setup.sh"
  } > "$case/setup.sh.new"
  mv "$case/setup.sh.new" "$case/setup.sh"
done

out=$here/results/$model-$(date -u +%Y-%m-%dT%H-%M-%SZ)
mkdir -p "$out"
# /usr/bin/git on macOS is an xcrun shim whose cache write the eval sandbox
# denies, and the bare `git` name stays pinned to that shim inside the agent
# session no matter what PATH says, so name a real binary outright. RVOW_GIT
# carries it to the hooks, which run outside the sandbox with this
# environment; EVAL_RVOW_GIT carries it to the agent session, whose sandbox
# passes only EVAL_* variables through (rvow reads both).
real_git=${RVOW_GIT:-}
if [ -z "$real_git" ]; then
  for g in /opt/homebrew/bin/git /usr/local/bin/git; do
    if [ -x "$g" ]; then real_git=$g; break; fi
  done
fi
if [ -z "$real_git" ]; then
  while IFS= read -r g; do
    if [ "$g" != /usr/bin/git ] || [ "$(uname)" != Darwin ]; then
      if [ -x "$g" ]; then real_git=$g; break; fi
    fi
  done < <(command -v -a git 2>/dev/null)
fi
if [ -n "$real_git" ] && "$real_git" --version >/dev/null 2>&1; then
  export PATH="$bindir:$PATH" RVOW_HOME="$rvow_home" RVOW_BIN="$rvow" RVOW_GIT="$real_git" EVAL_RVOW_GIT="$real_git"
else
  echo "run.sh: no working git outside the shim; eval git calls may fail" >&2
  export PATH="$bindir:$PATH" RVOW_HOME="$rvow_home" RVOW_BIN="$rvow"
fi
# Only the plugin under test: no memory, bundled skills or CLAUDE.md files.
(
  cd "$tmp"
  env CLAUDE_CODE_DISABLE_AUTO_MEMORY=1 CLAUDE_CODE_DISABLE_ORG_MEMORY=1 \
    CLAUDE_CODE_DISABLE_BUNDLED_SKILLS=1 CLAUDE_CODE_DISABLE_CLAUDE_MDS=1 \
    CLAUDE_CODE_DISABLE_WORKFLOWS=1 \
    claude plugin eval "$plugin" --model "$model" --no-publish \
    --trust-plugin --threshold 0 --runs 3 --scaffold -j 3 \
    --allow-tools Bash Write Edit 'mcp__rvow__*' --max-cost-usd 5 \
    --json "$out/result.json" "$@"
)
cp -R "$plugin/evals/results/." "$out/" 2>/dev/null || true
echo "results: $out"
