import type { AgentSpawnInput, On } from 'claude-code'
import { expect, test } from 'claude-code/testing'
import { procResult } from './testkit'

type Call = { event: string; input: Record<string, unknown> }

const spawn = (prompt: string): AgentSpawnInput => ({
  tool_use_id: 't1',
  prompt,
  description: 'a task',
  subagentType: 'general-purpose',
  provider: { plugin: 'engine', tier: 'core' },
  parentModel: 'm',
  background: false,
  fork: false,
})

function fakeRvow(on: On) {
  const calls: Call[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('process.run', async (_$, e) => {
    const event = e.argv[3] ?? ''
    calls.push({ event, input: e.init?.stdin ? JSON.parse(e.init.stdin) : {} })
    return { value: procResult(0, '') }
  })
  return calls
}

test('a subagent whose prompt starts with a lease line is bound to it', async ($, on) => {
  const calls = fakeRvow(on)
  on('agent.spawn', async () => ({ model: 'm', agentId: 'a1' }))
  await $.agent.spawn(spawn('lease: TSK-abc123\nYou hold one task.'))
  const bind = calls.find((c) => c.event === 'lease-bind')
  expect(bind?.input).toMatchObject({
    agent_id: 'a1',
    prompt: 'lease: TSK-abc123\nYou hold one task.',
  })
})

test('a prompt without a lease line starts no rvow process', async ($, on) => {
  const calls = fakeRvow(on)
  on('agent.spawn', async () => ({ model: 'm', agentId: 'a2' }))
  await $.agent.spawn(spawn('Read README.md and summarize it.'))
  expect(calls.filter((c) => c.event === 'lease-bind')).toHaveLength(0)
})

test('a tool call from a subagent carries its agent id to the hook', async ($, on) => {
  const calls = fakeRvow(on)
  on('tool.call', { tool: 'Edit' }, async () => ({ result: 'ok' }))
  await $.tool.call({
    tool: 'Edit',
    file_path: '/tmp/a.txt',
    old_string: 'a',
    new_string: 'b',
    agentId: 'a1',
  } as never)
  const pre = calls.find((c) => c.event === 'pre-tool-use')
  expect(pre?.input).toMatchObject({ tool_name: 'Edit', agent_id: 'a1' })
})

test('a tool call carries the session id of the last prompt to the hook', async ($, on) => {
  const calls = fakeRvow(on)
  on('classic.UserPromptSubmit', () => ({}))
  on('tool.call', { tool: 'Edit' }, async () => ({ result: 'ok' }))
  await $.classic.UserPromptSubmit({
    session_id: 's9',
    transcript_path: '/t/s9.jsonl',
    cwd: '/tmp',
    prompt: 'hi',
  })
  await $.tool.call({
    tool: 'Edit',
    file_path: '/tmp/a.txt',
    old_string: 'a',
    new_string: 'b',
  } as never)
  const pre = calls.find((c) => c.event === 'pre-tool-use')
  expect(pre?.input).toMatchObject({ session_id: 's9' })
})

test('a subagent that stops ends its lease', async ($, on) => {
  const calls = fakeRvow(on)
  on('classic.SubagentStop', async () => ({}))
  await $.classic.SubagentStop({
    stop_hook_active: false,
    agent_id: 'a1',
    agent_transcript_path: '/tmp/t',
    agent_type: 'general-purpose',
  })
  expect(calls.find((c) => c.event === 'lease-end')?.input).toMatchObject({ agent_id: 'a1' })
})
