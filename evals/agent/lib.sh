#!/usr/bin/env bash
# Shared setup helpers for the agent eval cases under evals/agent/cases/.
# Sourced by each case's setup.sh, never run directly. Expects RVOW_BIN (the
# rvow binary; defaults to rvow on PATH) and RVOW_HOME in the environment;
# evals/agent/run.sh bakes absolute values into the copies it hands to the
# eval harness, and the offline test sets them from CARGO_BIN_EXE_rvow.
# The setup runs as the owner, so `rvow accept --yes` is allowed here.
: "${RVOW_BIN:=rvow}"

# A fresh git repo with a small committed Python file. Runs in the cwd.
rvow_eval_git_repo() {
  git init -q -b main
  printf 'def add(a, b):\n    return a + b\n' > app.py
  printf '# Demo\n' > README.md
  git -c user.name=t -c user.email=t@t add app.py README.md
  git -c user.name=t -c user.email=t@t commit -q -m "feat: add app"
}

# An in-repo rvow tree for the current repository.
rvow_eval_init() {
  "$RVOW_BIN" init --in-repo >/dev/null
}

# Create a backlog change without accepting it. $1 is the title, $2 the
# tasks.md body, $3 the spec body (one REQ). Prints "<id> <dir>".
rvow_eval_new_change() {
  local title=$1 tasks=$2 spec=$3
  local out id dir
  out=$("$RVOW_BIN" new --spec "$title")
  id=$(printf '%s\n' "$out" | sed -n 's/^created: //p')
  dir=$(printf '%s\n' "$out" | sed -n 's/^path: //p')
  printf '%s' "$tasks" > "$dir/tasks.md"
  printf '%s' "$spec" > "$dir/specs/case.md"
  printf '%s %s\n' "$id" "$dir"
}

# Create a backlog change and accept it. Same arguments as
# rvow_eval_new_change; prints "<id> <dir>" (the backlog dir).
rvow_eval_accept_change() {
  local id dir
  read -r id dir <<< "$(rvow_eval_new_change "$1" "$2" "$3")"
  "$RVOW_BIN" accept "$id" --yes >/dev/null
  printf '%s %s\n' "$id" "$dir"
}

# Link a change dir where a grader can read it without showing in the work
# tree. $1 is the change dir relative to the repo root, $2 the link name.
# Glob and Grep skip .git/, so the agent never trips over the link, while a
# grader with target {source: file} reads straight through it.
rvow_eval_hide_link() {
  mkdir -p .git/rvow-eval
  ln -s "../../$1" ".git/rvow-eval/$2"
}
