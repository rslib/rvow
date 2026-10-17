import type { On } from 'claude-code'
import { expect } from 'claude-code/testing'
import { procResult, test } from './testkit'

type Call = { event: string; input: Record<string, unknown> }

function fakeRvow(on: On, post = '') {
  const calls: Call[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('process.run', async (_$, e) => {
    const event = e.argv[3] ?? ''
    calls.push({ event, input: e.init?.stdin ? JSON.parse(e.init.stdin) : {} })
    return { value: procResult(0, event === 'post-tool-use' ? post : '') }
  })
  return calls
}

test('a checked call reports the same call id before and after it runs', async ($, on) => {
  const calls = fakeRvow(on)
  on('classic.UserPromptSubmit', () => ({}))
  on('tool.call', { tool: 'Bash' }, async () => ({ result: 'ok' }))
  await $.classic.UserPromptSubmit({
    session_id: 's1',
    transcript_path: '/t/s1.jsonl',
    cwd: '/tmp',
    prompt: 'hi',
  })
  await $.tool.call({ tool: 'Bash', command: 'echo hi', tool_use_id: 't1use' } as never)
  const pre = calls.find((c) => c.event === 'pre-tool-use')
  expect(pre?.input).toMatchObject({ session_id: 's1', tool_use_id: 't1use' })
  const post = calls.find((c) => c.event === 'post-tool-use')
  expect(post?.input).toMatchObject({ session_id: 's1', tool_use_id: 't1use' })
  expect(calls.filter((c) => c.event === 'post-tool-use')).toHaveLength(1)
})

test('a failed checked call still reports post-tool-use', async ($, on) => {
  const calls = fakeRvow(on)
  on('tool.call', { tool: 'Bash' }, async () => {
    throw new Error('boom')
  })
  await $.tool
    .call({ tool: 'Bash', command: 'false', tool_use_id: 't2use' } as never)
    .catch(() => {})
  const post = calls.find((c) => c.event === 'post-tool-use')
  expect(post?.input).toMatchObject({ tool_use_id: 't2use' })
})

test('a subagent call carries the agent id to post-tool-use too', async ($, on) => {
  const calls = fakeRvow(on)
  on('tool.call', { tool: 'Edit' }, async () => ({ result: 'ok' }))
  await $.tool.call({
    tool: 'Edit',
    file_path: '/tmp/a.txt',
    old_string: 'a',
    new_string: 'b',
    agentId: 'a1',
    tool_use_id: 't3use',
  } as never)
  const post = calls.find((c) => c.event === 'post-tool-use')
  expect(post?.input).toMatchObject({ agent_id: 'a1', tool_use_id: 't3use' })
})

const NOTICE = 'rvow: TSK-x is baseline pending. Run `rvow prove TSK-x` now.'
const allow = JSON.stringify({
  hookSpecificOutput: {
    hookEventName: 'PostToolUse',
    additionalContext: NOTICE,
  },
})

test('the notice of an allowed call reaches the model after the tool result', async ($, on) => {
  fakeRvow(on, allow)
  on('tool.call', { tool: 'Edit' }, async () => ({ result: 'ok', context: ['earlier'] }))
  const out = await $.tool.call({
    tool: 'Edit',
    file_path: '/tmp/a.txt',
    old_string: 'a',
    new_string: 'b',
    tool_use_id: 't4use',
  } as never)
  expect(out).toMatchObject({ result: 'ok', context: ['earlier', NOTICE] })
})

test('a call without a notice carries no context', async ($, on) => {
  fakeRvow(on, '')
  on('tool.call', { tool: 'Edit' }, async () => ({ result: 'ok' }))
  const out = await $.tool.call({
    tool: 'Edit',
    file_path: '/tmp/a.txt',
    old_string: 'a',
    new_string: 'b',
    tool_use_id: 't5use',
  } as never)
  expect(out).not.toHaveProperty('context')
})
