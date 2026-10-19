# Hooks for any harness

rvow judges each tool call of an agent through `rvow hook`.
The Claude mod and the pi extension call it for you.
This page is for a harness that has neither: it gives the formats `rvow hook` reads and prints, and how to wire one.

## Pick a format

| Command | For | Reads and prints |
|---------|-----|------------------|
| `rvow hook claude <event>` | Claude Code and the pi extension | Claude Code's hook JSON |
| `rvow hook codex <event>` | Codex | Claude Code's hook JSON; an ask is a deny, since Codex cannot ask |
| `rvow hook json <event>` | any other harness | the neutral format below |

The events are `pre-tool-use`, `post-tool-use`, `session-start` and `stop`.
`rvow hook --help` lists them all.
Every call sends one JSON object on stdin, gets at most one JSON object on stdout and exits 0.

## The neutral format

### pre-tool-use

Send this before the tool runs:

```json
{
  "cwd": "/path/to/repo",
  "tool": "Bash",
  "input": {"command": "rvow accept CHG-x"},
  "session_id": "s1",
  "agent_id": "a1",
  "tool_use_id": "t1",
  "user_made": {"since": 1760000000000, "changes": [], "yolo": [], "runs": []}
}
```

- `tool` and `input` are the tool's name and arguments.
  rvow knows the tool names of Claude Code, pi, OpenCode, Codex and Gemini CLI; any other name is judged by its words, such as `write` or `read`.
- `session_id` lets rvow count denials in a row and the files the session changed.
- `agent_id` names a subagent, so a lease applies to it.
- `tool_use_id` pairs this call with its `post-tool-use`.
- `user_made` is what the user did in this session (see [What the user made](how-it-works.md#what-the-user-made)).
  Leave it out when the harness cannot tell the user's actions from the agent's.

rvow prints the decision:

```json
{"decision": "deny", "reason": "rvow: rvow accept belongs to the user; ..."}
```

`"decision"` is `allow`, `deny` or `ask`.
A deny or an ask has a `"reason"` for the agent; an ask is a question for the user, and a harness that cannot ask treats it as a deny.

### post-tool-use, session-start and stop

Each event reads `cwd` too.

- `post-tool-use` takes `session_id`, `agent_id` and `tool_use_id`, so rvow records the files the call changed, and prints `{}`.
- `session-start` takes `session_id` and `source` (`startup`, `resume`, `clear`, `compact` or `fork`), and prints `{"context": "..."}` with the brief for the agent, or `{}` when the session already has the same brief.
- `stop` takes `session_id`, `agent_id` and `stop_hook_active`, and prints `{"decision": "block", "reason": "..."}` when the session must go on, else `{}`; the same reason is not given twice in a row.

## Wire a harness

1. Run `rvow hook json pre-tool-use` before every tool call, and pass the `"reason"` of a deny to the agent as the tool's error.
2. Run `rvow hook json post-tool-use` after every call that ran, also a failed one.
3. Add the `"context"` of `session-start` to the agent's first prompt.
4. Run `rvow hook json stop` when the agent wants to end its turn, and send the `"reason"` of a block back as the next prompt.
5. When the hook cannot run, deny the tools that write and let the reading tools run, as the Claude mod does (see [When the hook fails](how-it-works.md#when-the-hook-fails)).

For Codex, point its `PreToolUse`, `PostToolUse`, `SessionStart` and `Stop` hooks in `~/.codex/hooks.json` at `rvow hook codex <event>`.
Codex reads the answer as Claude Code does.
Codex runs a hook only after you trust it with `/hooks`, and the OpenAI docs call its hooks a guardrail, not a full boundary.
