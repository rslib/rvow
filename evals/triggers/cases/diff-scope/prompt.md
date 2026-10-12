---
tags: [train, topic:failed-gate]
max_turns: 3
timeout_seconds: 120
allowed_tools: [Read, Glob, Grep, Skill]
---

my diff touches Cargo.toml but the task files list only names src/. gate will fail right? what do i run to fix the plan before pushing
