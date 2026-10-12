#!/usr/bin/env bash
# No change on the branch: a repo with an rvow tree, nothing planned yet.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
if [ -f "$here/lib.sh" ]; then
  . "$here/lib.sh"
else
  . "$here/../../lib.sh"
fi
rvow_eval_git_repo
rvow_eval_init
