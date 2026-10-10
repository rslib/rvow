import type { On } from 'claude-code'
import { expect, test } from 'claude-code/testing'
import { ACCEPT, ARCHIVE, archiveBlockers, CONFIRM, failedText } from './format'
import { procResult } from './testkit'

const LIST = 'AMD-a1 weaken T1 verify changed\ndigest abc123\n'

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
  on('tool.call', { tool: 'AskUserQuestion' }, async (_$, e) => {
    const question = e.questions[0]?.question ?? ''
    asked.push(question)
    return { result: { questions: e.questions, answers: { [question]: answer } } }
  })
  return { ran, asked }
}

const call = (tool: string, input: Record<string, unknown>) => ({ tool, ...input }) as never
const mutating = (ran: string[]) => ran.filter((l) => /^(accept|confirm --)/.test(l))

test('rvow_accept shows the review, and the user decides', async ($, on) => {
  const { ran, asked } = fake(on, 'Accept', (line) =>
    line.startsWith('review') ? [0, 'the checklist\n'] : [0, 'accepted CHG-a1\nnext: TSK-1 go\n'],
  )
  const out = JSON.stringify(await $.tool.call(call(ACCEPT, { change: 'CHG-a1' })))
  expect(asked[0]).toContain('the checklist')
  expect(asked[0]).toContain('Accept CHG-a1?')
  expect(mutating(ran)).toEqual(['accept CHG-a1 --yes'])
  expect(out).toContain('next: TSK-1 go')
})

test('rvow_accept runs nothing when the user says not now', async ($, on) => {
  const { ran } = fake(on, 'Not now', () => [0, 'the checklist\n'])
  const out = JSON.stringify(await $.tool.call(call(ACCEPT, { change: 'CHG-a1' })))
  expect(mutating(ran)).toEqual([])
  expect(out).toContain('did not accept CHG-a1')
})

test('a failed accept tells the agent why, in the result', async ($, on) => {
  const { ran } = fake(on, 'Accept', (line) =>
    line.startsWith('review')
      ? [0, 'checklist\n']
      : [1, 'error: CHG-a1 has no passing check\ncode: not-ready\n'],
  )
  const out = JSON.stringify(await $.tool.call(call(ACCEPT, { change: 'CHG-a1' })))
  expect(mutating(ran)).toEqual(['accept CHG-a1 --yes'])
  expect(out).toContain('rvow accept CHG-a1 failed')
  expect(out).toContain('code: not-ready')
})

test('a review that fails ends before any dialog and says why', async ($, on) => {
  const { asked } = fake(on, 'Accept', () => [1, 'error: no such change\n'])
  const out = JSON.stringify(await $.tool.call(call(ACCEPT, { change: 'CHG-zz' })))
  expect(asked).toEqual([])
  expect(out).toContain('rvow review CHG-zz failed')
  expect(out).toContain('no such change')
})

test('rvow_accept refuses an id that is not complete', async ($, on) => {
  const { ran } = fake(on, 'Accept', () => [0, ''])
  const out = JSON.stringify(await $.tool.call(call(ACCEPT, { change: 'x; rm -rf /' })))
  expect(out).toContain('rvow: change must be a complete CHG id')
  expect(ran).toEqual([])
})

test('rvow_confirm lists without a dialog and decides with one', async ($, on) => {
  const { ran, asked } = fake(on, 'Yes', (line) =>
    line === 'confirm' ? [0, LIST] : [0, 'AMD-a1 accepted\n'],
  )
  expect(JSON.stringify(await $.tool.call(call(CONFIRM, {})))).toContain('digest abc123')
  expect(asked).toEqual([])
  const out = JSON.stringify(await $.tool.call(call(CONFIRM, { decision: 'accept' })))
  expect(asked[0]).toContain('Accept all of these?')
  expect(mutating(ran)).toEqual(['confirm --accept --all abc123'])
  expect(out).toContain('AMD-a1 accepted')
})

test('a confirm the user declines or that fails is reported to the agent', async ($, on) => {
  const no = fake(on, 'No', () => [0, LIST])
  expect(JSON.stringify(await $.tool.call(call(CONFIRM, { decision: 'reject' })))).toContain(
    'still pending',
  )
  expect(mutating(no.ran)).toEqual([])
})

test('the failure text names the command and keeps its own words', () => {
  expect(failedText(['accept', 'CHG-a'], 'error: x\n\ncode: y\n')).toBe(
    'rvow accept CHG-a failed:\nerror: x\ncode: y',
  )
})

const view = (states: string[], pending = 0) =>
  JSON.stringify({
    changes: [
      {
        id: 'CHG-a1',
        tasks: states.map((state, i) => ({ id: `TSK-${i}`, state })),
        pending: Array.from({ length: pending }, () => ({ id: 'AMD-1' })),
      },
    ],
  })
const lines = (line: string, states: string[], pending = 0): [number, string] =>
  line === 'hook claude view' ? [0, view(states, pending)] : [0, 'archived CHG-a1\n']

test('rvow_archive asks only when every task is proved, and archives on a yes', async ($, on) => {
  const { ran, asked } = fake(on, 'Archive', (line) => lines(line, ['proved', 'proved']))
  const out = JSON.stringify(await $.tool.call(call(ARCHIVE, { change: 'CHG-a1' })))
  expect(asked[0]).toContain('Every task of CHG-a1 is proved')
  expect(ran.filter((l) => l.startsWith('archive'))).toEqual(['archive CHG-a1'])
  expect(out).toContain('archived CHG-a1')
})

test('rvow_archive names what is missing and asks nothing when the change is not ready', async ($, on) => {
  const { ran, asked } = fake(on, 'Archive', (line) => lines(line, ['proved', 'stale']))
  const out = JSON.stringify(await $.tool.call(call(ARCHIVE, { change: 'CHG-a1' })))
  expect(out).toContain('TSK-1 is stale')
  expect(asked).toEqual([])
  expect(ran.filter((l) => l.startsWith('archive'))).toEqual([])
})

test('rvow_archive waits for the user to decide a pending plan edit', async ($, on) => {
  const { asked } = fake(on, 'Archive', (line) => lines(line, ['proved'], 1))
  expect(JSON.stringify(await $.tool.call(call(ARCHIVE, { change: 'CHG-a1' })))).toContain(
    'pending plan edit',
  )
  expect(asked).toEqual([])
})

test('a refused or failed archive is reported to the agent', async ($, on) => {
  const no = fake(on, 'Not yet', (line) => lines(line, ['proved']))
  expect(JSON.stringify(await $.tool.call(call(ARCHIVE, { change: 'CHG-a1' })))).toContain(
    'stays active',
  )
  expect(no.ran.filter((l) => l.startsWith('archive'))).toEqual([])
})

test('archiveBlockers reads the state of one change', () => {
  const ok = { id: 'CHG-a1', tasks: [{ id: 'TSK-1', state: 'proved' }], pending: [] }
  expect(archiveBlockers(ok, 'CHG-a1')).toBeNull()
  expect(archiveBlockers(undefined, 'CHG-a1')).toContain('not an active change')
  expect(archiveBlockers({ ...ok, tasks: [] }, 'CHG-a1')).toContain('no task')
})
