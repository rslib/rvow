---
type: regex
pattern: 'grep -q sub CHANGELOG.md'
match: contains
target: {source: file, path: .git/rvow-eval/change/tasks.md}
weight: 2
arm: both
---
