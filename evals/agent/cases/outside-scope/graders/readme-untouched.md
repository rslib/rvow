---
type: regex
pattern: '(?i)multiply'
match: not_contains
target: {source: file, path: README.md}
weight: 2
arm: both
---
