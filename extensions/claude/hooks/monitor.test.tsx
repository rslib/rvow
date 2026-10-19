import { expect } from 'claude-code/testing'
import { procResult, test } from './testkit'

const MONITOR = {
  tool: 'Monitor',
  command: 'rvow accept CHG-x --yes',
  description: 'd',
  timeout_ms: 1000,
} as const

test('a Monitor call goes to the hook and its deny stops the call', async ($, on) => {
  const sent: Record<string, unknown>[] = []
  let ran = 0
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('process.run', async (_$, e) => {
    const input = e.init?.stdin ? JSON.parse(e.init.stdin) : {}
    if (e.argv[3] !== 'pre-tool-use') return { value: procResult(0, '') }
    sent.push(input)
    const deny = {
      hookSpecificOutput: { permissionDecision: 'deny', permissionDecisionReason: 'rvow: no' },
    }
    return { value: procResult(0, JSON.stringify(deny)) }
  })
  on('tool.call', { tool: 'Monitor' }, async () => {
    ran++
    return { result: 'ok' }
  })
  const r = await $.tool.call(MONITOR)
  expect(sent[0]).toMatchObject({
    tool_name: 'Monitor',
    tool_input: { command: 'rvow accept CHG-x --yes' },
  })
  expect(r.deny).toBe('rvow: no')
  expect(ran).toBe(0)
})
