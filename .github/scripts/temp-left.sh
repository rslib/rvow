#!/bin/sh
# Fails when a test run left anything in the temp folder $1. Node's shared
# compile cache is not a leak: Node keeps and reuses it across runs.
set -eu

left=$(ls -A "$1" | grep -vx 'node-compile-cache' || true)
if [ -n "$left" ]; then
  echo "error: the tests left $(echo "$left" | wc -l | tr -d ' ') entries in the temp folder:" >&2
  echo "$left" | head -20 >&2
  exit 1
fi
echo "ok: the tests left no temp files"
