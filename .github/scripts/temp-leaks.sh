#!/bin/sh
# Runs the test suite with an empty temp folder and fails when a test leaves
# a file or folder there. Left-over folders pile up in the system temp folder
# and slow every later file write in it. Extra arguments go to nextest.
set -eu

tmp=$(mktemp -d "${RUNNER_TEMP:-/tmp}/rvow-leaks.XXXXXX")
tmp=$(cd "$tmp" && pwd -P)
trap 'rm -rf "$tmp"' EXIT

TMPDIR="$tmp/" cargo nextest run --locked "$@"

sh "$(dirname "$0")/temp-left.sh" "$tmp"
