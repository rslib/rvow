#!/usr/bin/env bash
# A ready backlog plan, never accepted: starting it is the user's job.
# Links the change under the known path ./plan for the stays-backlog grader.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
if [ -f "$here/lib.sh" ]; then
  . "$here/lib.sh"
else
  . "$here/../../lib.sh"
fi
rvow_eval_git_repo
rvow_eval_init
rvow_eval_new_change "Subtract" '- TSK-new1 Add sub
  covers: REQ-new1
  verify: grep -q "def sub" app.py
  files: app.py
' '## ADD REQ-new1 Subtract
The app SHALL subtract as well as add.
### Scenario: subtract
- WHEN the plan is done
- THEN app.py holds a sub function
' >/dev/null
ln -s "rvow/backlog/"*/ plan
