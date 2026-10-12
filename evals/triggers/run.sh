#!/usr/bin/env bash
# Do prompts load the rvow skill? Renders the skill with the built binary
# into a throwaway home, builds a plugin that holds that skill and no
# hooks, and runs the trigger cases in evals/triggers/cases/ on the model
# given (default haiku) with no baseline arm. Results stay local.
#
#   evals/triggers/run.sh [model] [extra claude plugin eval flags]
#   evals/triggers/run.sh --build-only <dir>
#
# Uses $RVOW (default target/debug/rvow); builds nothing. Each run is a real
# model call: about $3 at most for a full run through --max-cost-usd.
# Nothing is written to your home, your rvow store or your Claude folder:
# HOME, RVOW_HOME and the Claude config dir all point under a mktemp dir
# (removed on exit) for the install. --build-only builds the skill-only
# plugin into <dir> and exits before calling claude; the offline test uses it.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
rvow=${RVOW:-$root/target/debug/rvow}
rvow=$(cd "$(dirname "$rvow")" && pwd)/$(basename "$rvow")

build_only=""
if [ "${1:-}" = "--build-only" ]; then
  build_only=${2:?usage: run.sh --build-only <dir>}
  shift 2
fi

build_plugin() {
  dest=$1
  mkdir -p "$dest/.claude-plugin" "$dest/skills/rvow" "$dest/evals"
  printf '{"name": "rvow", "version": "0.0.0", "description": "rvow skill triggers"}\n' > "$dest/.claude-plugin/plugin.json"
  cp "$plugin/skills/rvow/SKILL.md" "$dest/skills/rvow/SKILL.md"
  if [ -d "$here/cases" ]; then
    cp -R "$here/cases/." "$dest/evals/"
  fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
rvow_home=$tmp/home/.local/share/rvow
# Throwaway home for the install: the mod lands at
# $RVOW_HOME/claude-plugin/plugins/rvow, and the Claude registration stays
# under the temp dir too, so the user's store and Claude folder are untouched.
# The cd keeps the pre-push gate out of the real repository.
(cd "$tmp" && HOME=$tmp/home XDG_DATA_HOME=$tmp/home/.local/share XDG_CONFIG_HOME=$tmp/home/.config CLAUDE_CONFIG_DIR=$tmp/claude RVOW_HOME=$rvow_home "$rvow" install >/dev/null)
plugin=$rvow_home/claude-plugin/plugins/rvow

if [ -n "$build_only" ]; then
  build_plugin "$build_only"
  exit 0
fi

model=${1:-haiku}
shift || true
dest=$tmp/trigger-plugin
build_plugin "$dest"

out=$here/results/$model-$(date -u +%Y-%m-%dT%H-%M-%SZ)
mkdir -p "$out"
# Only the plugin under test: no memory, bundled skills or CLAUDE.md files.
(
  cd "$tmp"
  env CLAUDE_CODE_DISABLE_AUTO_MEMORY=1 CLAUDE_CODE_DISABLE_ORG_MEMORY=1 \
    CLAUDE_CODE_DISABLE_BUNDLED_SKILLS=1 CLAUDE_CODE_DISABLE_CLAUDE_MDS=1 \
    CLAUDE_CODE_DISABLE_WORKFLOWS=1 \
    claude plugin eval "$dest" --model "$model" --ablation none --no-publish \
    --trust-plugin --threshold 0 --runs 3 -j 3 --max-cost-usd 3 \
    --json "$out/result.json" "$@"
)
cp -R "$dest/evals/results/." "$out/" 2>/dev/null || true
echo "results: $out"
