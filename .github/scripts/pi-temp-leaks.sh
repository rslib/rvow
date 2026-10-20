#!/bin/sh
# Runs the pi tests with an empty temp folder and fails when a test leaves a
# file or folder there.
set -eu

tmp=$(mktemp -d "${RUNNER_TEMP:-/tmp}/rvow-leaks.XXXXXX")
tmp=$(cd "$tmp" && pwd -P)
trap 'rm -rf "$tmp"' EXIT

TMPDIR="$tmp/" npm --prefix extensions run test:pi
sh "$(dirname "$0")/temp-left.sh" "$tmp"
