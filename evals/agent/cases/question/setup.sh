#!/usr/bin/env bash
# An active change with one missing task, for a question about it.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
if [ -f "$here/lib.sh" ]; then
  . "$here/lib.sh"
else
  . "$here/../../lib.sh"
fi
rvow_eval_git_repo
rvow_eval_init
rvow_eval_accept_change "Subtract" '- TSK-new1 Add sub
  covers: REQ-new1
  verify: grep -q "def sub" app.py
  files: app.py
' '## ADD REQ-new1 Subtract
The app SHALL subtract as well as add.
### Scenario: subtract
- WHEN the plan is done
- THEN app.py holds a sub function
'
