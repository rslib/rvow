#!/usr/bin/env bash
# Release notes for TAG: the version block of CHANGELOG.md without its header,
# then the compare link from the link list.
# Usage: release-notes.sh TAG [CHANGELOG]
set -euo pipefail
tag=$1
file=${2:-CHANGELOG.md}
version=${tag#v}

notes=$(awk -v v="$version" '
  index($0, "## [" v "]") == 1 { on = 1; next }
  on && /^## \[/ { exit }
  on && /^\[[^]]+\]: / { exit }
  on { print }
' "$file" | sed -e '/./,$!d')
if [ -z "$(printf '%s' "$notes" | tr -d '[:space:]')" ]; then
  echo "$file has no entries under ## [$version]" >&2
  exit 1
fi
printf '%s\n' "$notes" | sed -e :a -e '/^\n*$/{$d;N;ba' -e '}'
link=$(grep -m1 -F "[$version]: " "$file" | sed 's/^[^:]*: //' || true)
[ -n "$link" ] && printf '\n**Full Changelog**: %s\n' "$link"
exit 0
