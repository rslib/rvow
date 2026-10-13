---
tags: [train, topic:quiet-bug]
max_turns: 3
timeout_seconds: 120
allowed_tools: [Read, Glob, Grep, Skill]
---

getting this traceback when i run the script, any idea whats wrong

```
Traceback (most recent call last):
  File "app.py", line 12, in <module>
    data = json.load(f)
FileNotFoundError: [Errno 2] No such file or directory: 'config.json'
```

my app.py opens config.json with a relative path. the file is in the same folder as the script but i run it from the repo root
