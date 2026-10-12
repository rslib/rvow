#!/usr/bin/env bash
# Active change with one missing task: add mul to app.py. No task names
# README.md, so the prompted README edit is outside scope.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
if [ -f "$here/lib.sh" ]; then
  . "$here/lib.sh"
else
  . "$here/../../lib.sh"
fi
rvow_eval_git_repo
rvow_eval_init
rvow_eval_accept_change "Multiply" '- TSK-new1 Add mul
  covers: REQ-new1
  verify: grep -q "def mul" app.py
  files: app.py
' '## ADD REQ-new1 Multiply
The app SHALL multiply as well as add.
### Scenario: multiply
- WHEN the plan is done
- THEN app.py holds a mul function
'
