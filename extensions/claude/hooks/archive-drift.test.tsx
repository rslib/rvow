import type { On } from 'claude-code'
import { expect } from 'claude-code/testing'
import { ARCHIVE, driftReason, unrecordedDrift } from './format'
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

const WEAKEN = { class: 'weaken', what: 'test a.test.ts changed t1' }

const view = (drift: object[], pending: object[]) =>
  JSON.stringify({
    changes: [{ id: 'CHG-a1', tasks: [{ id: 'TSK-1', state: 'proved' }], drift, pending }],
  })

const LISTING =
  'AMD-n1 for CHG-a1\n  weaken: test a.test.ts changed t1\n  reason (from the agent): recorded\ndigest d1\n'

test('rvow_archive records drift itself, then asks once and archives', async ($, on) => {
  const { ran, asked } = fake(on, 'Accept and archive', (line) => {
    if (line === 'hook claude view') return [0, view([WEAKEN], [])]
    if (line.startsWith('amend')) return [0, 'amendments[1]:\n  - id: AMD-n1\n']
    if (line === 'confirm --full') return [0, LISTING]
    return [0, 'ok\n']
  })
  const out = JSON.stringify(await $.tool.call(call(ARCHIVE, { change: 'CHG-a1' })))
  const amend = ran.find((l) => l.startsWith('amend'))
  expect(amend).toContain('--change CHG-a1')
  expect(amend).toContain('--reason')
  expect(amend).toContain('test a.test.ts changed t1')
  expect(asked.length).toBe(1)
  expect(asked[0]).toContain('AMD-n1')
  expect(
    ran.filter((l) => /^(amend|confirm AMD|archive)/.test(l)).map((l) => l.split(' ')[0]),
  ).toEqual(['amend', 'confirm', 'archive'])
  expect(out).not.toContain('failed')
})

test('rvow_archive does not record drift that a pending plan edit already holds', async ($, on) => {
  const { ran } = fake(on, 'Accept and archive', (line) => {
    if (line === 'hook claude view') return [0, view([WEAKEN], [{ id: 'AMD-n1' }])]
    if (line === 'confirm --full') return [0, LISTING]
    return [0, 'ok\n']
  })
  await $.tool.call(call(ARCHIVE, { change: 'CHG-a1' }))
  expect(ran.filter((l) => l.startsWith('amend'))).toEqual([])
})

test('rvow_archive records no amendment when nothing drifted', async ($, on) => {
  const { ran } = fake(on, 'Archive', (line) =>
    line === 'hook claude view' ? [0, view([], [])] : [0, 'archived CHG-a1\n'],
  )
  await $.tool.call(call(ARCHIVE, { change: 'CHG-a1' }))
  expect(ran.filter((l) => l.startsWith('amend'))).toEqual([])
  expect(ran).toContain('archive CHG-a1')
})

test('rvow_archive stops with the failure when the amend fails', async ($, on) => {
  const { ran, asked } = fake(on, 'Archive', (line) => {
    if (line === 'hook claude view') return [0, view([WEAKEN], [])]
    if (line.startsWith('amend')) return [1, 'error: no plan edit\n']
    return [0, 'ok\n']
  })
  const out = JSON.stringify(await $.tool.call(call(ARCHIVE, { change: 'CHG-a1' })))
  expect(out).toContain('rvow amend')
  expect(out).toContain('failed')
  expect(asked).toEqual([])
  expect(ran.filter((l) => l.startsWith('archive'))).toEqual([])
})

test('unrecordedDrift keeps blocking items only, and none while an edit is pending', () => {
  const free = { class: 'free', what: 'a comment' }
  const base = { pending: [] as unknown[] }
  expect(unrecordedDrift(undefined)).toEqual([])
  expect(unrecordedDrift({ ...base, drift: [free] })).toEqual([])
  expect(unrecordedDrift({ ...base, drift: [WEAKEN, free] })).toEqual([WEAKEN])
  expect(unrecordedDrift({ pending: [{}], drift: [WEAKEN] })).toEqual([])
  expect(unrecordedDrift({ ...base })).toEqual([])
  expect(driftReason([WEAKEN])).toContain('test a.test.ts changed t1')
  expect(driftReason([WEAKEN]).length).toBeLessThan(600)
})
