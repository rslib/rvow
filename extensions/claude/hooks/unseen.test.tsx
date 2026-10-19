import type { On } from 'claude-code'
import { expect } from 'claude-code/testing'
import { ACCEPT } from './format'
import { cmd, procResult, test } from './testkit'

const EDIT = { tool: 'Edit', file_path: '/tmp/x', old_string: 'a', new_string: 'b' } as const

type UserMade = {
  since: number
  changes: string[]
  yolo: { tier: string; at: number; until: number }[]
  runs: { at: number; until: number }[]
}

/** A fake rvow that answers every call with exit 0; returns the `user_made` of each pre-tool-use call. */
function fakeRvow(on: On) {
  const seen: UserMade[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('ui.toast', async () => ({ value: undefined }))
  on('process.run', async (_$, e) => {
    if (e.argv[1] === 'hook' && e.argv[3] === 'pre-tool-use' && e.init?.stdin)
      seen.push(JSON.parse(e.init.stdin).user_made)
    return { value: procResult(0, '') }
  })
  on('tool.call', { tool: 'Edit' }, async () => ({ result: 'ok' }))
  return seen
}

test('each hook call carries what the user made, from the session start', async ($, on) => {
  const seen = fakeRvow(on)
  await $.tool.call(EDIT)
  const after = Date.now()
  expect(seen[0]?.since).toBeGreaterThan(0)
  expect(seen[0]?.since).toBeLessThanOrEqual(after)
  expect(seen[0]?.changes).toEqual([])
  expect(seen[0]?.yolo).toEqual([])
})

test('an accept the user types is recorded with its id and its run', async ($, on) => {
  const seen = fakeRvow(on)
  await $.command.run(cmd('rvow-accept', 'CHG-abc123'))
  await $.tool.call(EDIT)
  const last = seen.at(-1)
  expect(last?.changes).toEqual(['CHG-abc123'])
  expect(last?.runs).toEqual([])
})

test('only a confirm that accepts opens a span; a listing or a reject does not', async ($, on) => {
  const seen = fakeRvow(on)
  await $.command.run(cmd('rvow-confirm', 'reject AMD-abc123'))
  await $.tool.call(EDIT)
  expect(seen.at(-1)?.runs).toEqual([])
  await $.command.run(cmd('rvow-confirm', 'accept AMD-abc123'))
  await $.tool.call(EDIT)
  expect(seen.at(-1)?.runs.length).toBe(1)
})

test('an accept in the dialog is recorded', async ($, on) => {
  const seen = fakeRvow(on)
  on('tool.call', { tool: 'AskUserQuestion' }, async (_$, e) => {
    const questions = (e as { questions: { question: string }[] }).questions
    return { result: { questions, answers: { [questions[0]?.question ?? '']: 'Accept' } } }
  })
  await $.tool.call({ tool: ACCEPT, change: 'CHG-abc123' } as never)
  await $.tool.call(EDIT)
  expect(seen.at(-1)?.changes).toEqual(['CHG-abc123'])
})

test('a yolo tier the user types is recorded with its end', async ($, on) => {
  const seen = fakeRvow(on)
  const before = Date.now()
  await $.command.run(cmd('rvow-yolo', 'auto 30m'))
  await $.tool.call(EDIT)
  const [y] = seen.at(-1)?.yolo ?? []
  expect(y?.tier).toBe('auto')
  expect(y?.at).toBeGreaterThanOrEqual(before)
  expect((y?.until ?? 0) - (y?.at ?? 0)).toBe(30 * 60_000)
})

test('a command the agent sends is not the user, and records nothing', async ($, on) => {
  const seen = fakeRvow(on)
  on('tool.call', { tool: 'Bash' }, async () => ({ result: 'ok' }))
  await $.command.run(cmd('rvow-accept', 'CHG-abc123', { kind: 'model' } as never))
  await $.tool.call({ tool: 'Bash', command: 'rvow accept CHG-abc123 --yes' })
  await $.tool.call(EDIT)
  expect(seen.at(-1)?.changes).toEqual([])
  expect(seen.at(-1)?.runs).toEqual([])
})
