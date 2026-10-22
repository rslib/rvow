#!/usr/bin/env bash
# Times the five hook kinds of CHG-g2z0y2's baseline on a scratch copy of this
# repository and its living specs, in a scratch home, so the user's store and
# hook stats are never touched. With --check it fails when a hook's median is
# slower than its trunk time.
#
#   scripts/hook-bench.sh            # print the medians
#   scripts/hook-bench.sh --check    # also fail on a hook slower than trunk
set -euo pipefail

check=0
[[ "${1:-}" == "--check" ]] && check=1

command -v hyperfine >/dev/null || { echo "hook-bench: install hyperfine (brew install hyperfine)" >&2; exit 1; }
command -v jq >/dev/null || { echo "hook-bench: install jq" >&2; exit 1; }

repo=$(git rev-parse --show-toplevel)
cargo build --release --locked -q -p rvow --manifest-path "$repo/Cargo.toml"
target=$(cargo metadata --format-version 1 --no-deps --manifest-path "$repo/Cargo.toml" | jq -r .target_directory)
rv="$target/release/rvow"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/os" "$work/home" "$work/config"
"$rv" export openspec --to "$work/os" --human >/dev/null
# The baseline store held the living specs and no open change.
rm -rf "$work/os/openspec/changes"
git clone -q "$repo" "$work/repo"
# A branch that no plan names, as in the baseline, so no hook takes a plan's path.
git -C "$work/repo" switch -q -c hook-bench

export RVOW_HOME="$work/rh" HOME="$work/home" XDG_CONFIG_HOME="$work/config"
cd "$work/repo"
"$rv" init --human >/dev/null
"$rv" import openspec --from "$work/os/openspec" --human >/dev/null

top=$(pwd -P)
mkdir -p "$work/in"
printf '%s' "{\"cwd\":\"$top\",\"hook_event_name\":\"PreToolUse\",\"tool_name\":\"Edit\",\"tool_input\":{\"file_path\":\"$top/crates/rvow-core/src/id.rs\",\"old_string\":\"a\",\"new_string\":\"b\"},\"session_id\":\"s1\",\"tool_use_id\":\"e1\"}" >"$work/in/pre_edit.json"
printf '%s' "{\"cwd\":\"$top\",\"hook_event_name\":\"PreToolUse\",\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"cargo test -p rvow-core\"},\"session_id\":\"s1\",\"tool_use_id\":\"b1\"}" >"$work/in/pre_bash.json"
printf '%s' "{\"cwd\":\"$top\",\"hook_event_name\":\"PostToolUse\",\"tool_name\":\"Edit\",\"tool_input\":{\"file_path\":\"$top/crates/rvow-core/src/id.rs\"},\"session_id\":\"s1\",\"tool_use_id\":\"e1\"}" >"$work/in/post_edit.json"
printf '%s' "{\"cwd\":\"$top\",\"hook_event_name\":\"Stop\",\"session_id\":\"s1\"}" >"$work/in/stop.json"
printf '%s' "{\"cwd\":\"$top\",\"hook_event_name\":\"SessionStart\",\"source\":\"startup\",\"session_id\":\"s1\"}" >"$work/in/start.json"

# name, event, input, trunk median in ms (the design's baseline on d05d6ee)
cases=(
  "pre_edit pre-tool-use 31"
  "pre_bash pre-tool-use 23"
  "post_edit post-tool-use 12"
  "stop stop 40"
  "start session-start 22"
)
slow=0
for c in "${cases[@]}"; do
  read -r name event trunk <<<"$c"
  # The hook reads its input on stdin, so each run goes through a shell, as the
  # baseline's runs did.
  hyperfine --shell=default -w 3 -r 20 --export-json "$work/$name.json" --style none \
    "$rv hook claude $event < $work/in/$name.json > /dev/null" >/dev/null
  # hyperfine 1 writes `median`; hyperfine 2 writes `summary.time_wall_clock.median`.
  median=$(jq '(.results[0].median // .results[0].summary.time_wall_clock.median) * 1000 | floor' "$work/$name.json")
  printf '%-10s median %4s ms  trunk %3s ms\n' "$name" "$median" "$trunk"
  if (( median > trunk )); then slow=1; fi
done
if (( check && slow )); then
  echo "hook-bench: a hook is slower than its trunk time" >&2
  exit 1
fi
