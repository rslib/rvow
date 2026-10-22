import type { AgentSpawnInput, On } from 'claude-code'
import { expect } from 'claude-code/testing'
import { procResult, test } from './testkit'

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

test('a top-level prompt that starts with a lease line binds the session to it', async ($, on) => {
  const calls = fakeRvow(on)
  on('classic.UserPromptSubmit', () => ({}))
  await $.classic.UserPromptSubmit({
    session_id: 's1',
    transcript_path: '/t/s1.jsonl',
    cwd: '/tmp',
    prompt: 'lease: TSK-abc123\nYou hold one task.',
  })
  const bind = calls.find((c) => c.event === 'lease-bind')
  expect(bind?.input).toMatchObject({
    agent_id: 's1',
    prompt: 'lease: TSK-abc123\nYou hold one task.',
  })
  // The session is bound before the brief is asked for, so the brief names the leased task.
  const events = calls.map((c) => c.event)
  expect(events.indexOf('lease-bind')).toBeLessThan(events.indexOf('session-start'))
})

test('a top-level prompt without a lease line binds no lease', async ($, on) => {
  const calls = fakeRvow(on)
  on('classic.UserPromptSubmit', () => ({}))
  await $.classic.UserPromptSubmit({
    session_id: 's1',
    transcript_path: '/t/s1.jsonl',
    cwd: '/tmp',
    prompt: 'Read README.md and summarize it.',
  })
  expect(calls.filter((c) => c.event === 'lease-bind')).toHaveLength(0)
})

test('a top-level prompt whose lease line is not first binds no lease', async ($, on) => {
  const calls = fakeRvow(on)
  on('classic.UserPromptSubmit', () => ({}))
  await $.classic.UserPromptSubmit({
    session_id: 's1',
    transcript_path: '/t/s1.jsonl',
    cwd: '/tmp',
    prompt: 'please read this first\nlease: TSK-abc123\nYou hold one task.',
  })
  expect(calls.filter((c) => c.event === 'lease-bind')).toHaveLength(0)
})

test('a top-level lease that does not bind shows the reason to the user', async ($, on) => {
  const toasts: string[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('ui.toast', async (_$, e) => {
    toasts.push(e.text)
    return { value: undefined }
  })
  on('process.run', async (_$, e) => {
    const out =
      e.argv[3] === 'lease-bind'
        ? JSON.stringify({
            bound: false,
            task: 'TSK-abc123',
            reason: 'no open lease for TSK-abc123',
          })
        : ''
    return { value: procResult(0, out) }
  })
  on('classic.UserPromptSubmit', () => ({}))
  await $.classic.UserPromptSubmit({
    session_id: 's1',
    transcript_path: '/t/s1.jsonl',
    cwd: '/tmp',
    prompt: 'lease: TSK-abc123\nYou hold one task.',
  })
  expect(toasts.join('\n')).toContain('no open lease for TSK-abc123')
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

test('a lease that does not bind is shown to the user and returned to the agent', async ($, on) => {
  const toasts: string[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('ui.toast', async (_$, e) => {
    toasts.push(e.text)
    return { value: undefined }
  })
  on('process.run', async (_$, e) => {
    const out =
      e.argv[3] === 'lease-bind' ? JSON.stringify({ bound: false, task: 'TSK-abc123' }) : ''
    return { value: procResult(0, out) }
  })
  on('agent.spawn', async () => ({ model: 'm', agentId: 'a1' }))
  on('tool.call', { tool: 'Agent' }, async () => ({ result: 'done' }))
  await $.agent.spawn(spawn('lease: TSK-abc123\nYou hold one task.'))
  expect(toasts.join('\n')).toContain('rvow delegate TSK-abc123')
  const r = await $.tool.call({
    tool: 'Agent',
    tool_use_id: 't1',
    prompt: 'p',
    description: 'd',
  } as never)
  expect((r.context ?? []).join('\n')).toContain('rvow delegate TSK-abc123')
})

test('a subagent bind that fails without a reason says to start the subagent again', async ($, on) => {
  const toasts: string[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('ui.toast', async (_$, e) => {
    toasts.push(e.text)
    return { value: undefined }
  })
  on('process.run', async (_$, e) => {
    const out =
      e.argv[3] === 'lease-bind' ? JSON.stringify({ bound: false, task: 'TSK-abc123' }) : ''
    return { value: procResult(0, out) }
  })
  on('agent.spawn', async () => ({ model: 'm', agentId: 'a1' }))
  await $.agent.spawn(spawn('lease: TSK-abc123\nYou hold one task.'))
  expect(toasts.join('\n')).toContain('start the subagent again')
  expect(toasts.join('\n')).not.toContain('send the prompt again')
})

test('a top-level bind that fails without a reason says to send the prompt again', async ($, on) => {
  const toasts: string[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('ui.toast', async (_$, e) => {
    toasts.push(e.text)
    return { value: undefined }
  })
  on('process.run', async (_$, e) => {
    const out =
      e.argv[3] === 'lease-bind' ? JSON.stringify({ bound: false, task: 'TSK-abc123' }) : ''
    return { value: procResult(0, out) }
  })
  on('classic.UserPromptSubmit', () => ({}))
  await $.classic.UserPromptSubmit({
    session_id: 's1',
    transcript_path: '/t/s1.jsonl',
    cwd: '/tmp',
    prompt: 'lease: TSK-abc123\nYou hold one task.',
  })
  expect(toasts.join('\n')).toContain('send the prompt again')
  expect(toasts.join('\n')).not.toContain('start the subagent again')
})

test('a lease that binds shows nothing', async ($, on) => {
  const toasts: string[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('ui.toast', async (_$, e) => {
    toasts.push(e.text)
    return { value: undefined }
  })
  on('process.run', async (_$, e) => {
    const out =
      e.argv[3] === 'lease-bind' ? JSON.stringify({ bound: true, task: 'TSK-abc123' }) : ''
    return { value: procResult(0, out) }
  })
  on('agent.spawn', async () => ({ model: 'm', agentId: 'a1' }))
  await $.agent.spawn(spawn('lease: TSK-abc123\nYou hold one task.'))
  expect(toasts).toHaveLength(0)
})
