import type { On } from 'claude-code'
import { expect } from 'claude-code/testing'
import type { RvowPlan } from '../types'
import { ARCHIVE, acceptable, archiveBlockers, blocker, findChange } from './format'
import { procResult, spawnHook, test } from './testkit'

/** `reply` maps a command line (without the binary) to an exit code and its output. */
function fake(on: On, answer: string, reply: (line: string) => [number, string]) {
  const ran: string[] = []
  const asked: string[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('ui.toast', async () => ({ value: undefined }))
  on('process.run', async (_$, e) => {
    const line = [...e.argv].slice(1).join(' ')
    ran.push(line)
    const [code, out] = reply(line)
    return { value: procResult(code, out) }
  })
  on('process.spawn', async function* (_$, e) {
    const line = [...e.argv].slice(1).join(' ')
    ran.push(line)
    return yield* spawnHook(reply)(_$, e)
  })
  on('tool.call', { tool: 'AskUserQuestion' }, async (_$, e) => {
    const question = e.questions[0]?.question ?? ''
    asked.push(question)
    return { result: { questions: e.questions, answers: { [question]: answer } } }
  })
  return { ran, asked }
}

const call = (tool: string, input: Record<string, unknown>) => ({ tool, ...input }) as never

const change = (states: string[]) => ({
  id: 'CHG-a1',
  tasks: states.map((state, i) => ({ id: `TSK-${i}`, state })),
  pending: [],
})

const plan = (over: Partial<RvowPlan>): RvowPlan => ({
  id: 'CHG-p1',
  title: 'A plan',
  tasks: 1,
  questions: 0,
  issues: 0,
  ready: true,
  branch: 'feat/x',
  here: false,
  path: '/p',
  ...over,
})

test('rvow_archive archives a change that lives on another branch', async ($, on) => {
  const { ran, asked } = fake(on, 'Archive', (line) =>
    line === 'hook claude view'
      ? [0, JSON.stringify({ changes: [], elsewhere: [change(['proved', 'stale'])] })]
      : [0, 'archived CHG-a1\n'],
  )
  const out = JSON.stringify(await $.tool.call(call(ARCHIVE, { change: 'CHG-a1' })))
  expect(asked[0]).toContain('Every task of CHG-a1 is proved')
  expect(ran.filter((l) => l.startsWith('archive'))).toEqual(['archive CHG-a1'])
  expect(out).toContain('archived CHG-a1')
})

test('rvow_archive still names a change that no worktree holds', async ($, on) => {
  const { ran, asked } = fake(on, 'Archive', () => [0, JSON.stringify({ changes: [] })])
  const out = JSON.stringify(await $.tool.call(call(ARCHIVE, { change: 'CHG-a1' })))
  expect(out).toContain('not an active change')
  expect(out).toContain('any worktree')
  expect(asked).toEqual([])
  expect(ran.filter((l) => l.startsWith('archive'))).toEqual([])
})

test('findChange looks on this branch first, then in the other worktrees', () => {
  const here = change(['proved'])
  const there = { ...change(['stale']), id: 'CHG-b2' }
  const view = { changes: [here], elsewhere: [there] }
  expect(findChange(view, 'CHG-a1')).toBe(here)
  expect(findChange(view, 'CHG-b2')).toBe(there)
  expect(findChange({ changes: [here] }, 'CHG-b2')).toBeUndefined()
  expect(archiveBlockers(findChange(view, 'CHG-b2'), 'CHG-b2')).toBeNull()
})

test('a plan whose branch has a worktree can be accepted from any branch', () => {
  expect(acceptable(plan({ here: true }))).toBe(true)
  expect(acceptable(plan({ worktree: '/w' }))).toBe(true)
  expect(acceptable(plan({ worktree: null }))).toBe(false)
  expect(acceptable(plan({}))).toBe(false)
  expect(acceptable(plan({ worktree: '/w', ready: false, questions: 1 }))).toBe(false)
  expect(blocker(plan({}))).toBe('take it to this branch first')
  expect(blocker(plan({ worktree: '/w', questions: 1 }))).toBe('answer the open questions first')
})
