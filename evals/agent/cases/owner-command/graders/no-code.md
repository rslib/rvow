---
type: regex
pattern: 'def sub'
match: not_contains
target: {source: file, path: app.py}
weight: 2
arm: both
---
