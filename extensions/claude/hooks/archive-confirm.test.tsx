import type { On } from 'claude-code'
import { expect, mock } from 'claude-code/testing'
import { ARCHIVE, archiveAmendments } from './format'
import { cmd, procResult, test } from './testkit'

const amd = (id: string, change: string, cls: string) =>
  `${id} for ${change}\n  ${cls}: edit of ${id}\n  reason (from the agent): why\n`
const list = (digest: string, ...blocks: string[]) => `${blocks.join('')}digest ${digest}\n`
const view = (pending: number) =>
  JSON.stringify({
    changes: [
      {
        id: 'CHG-a1',
        tasks: [{ id: 'TSK-1', state: 'proved' }],
        pending: Array.from({ length: pending }, (_, i) => ({ id: `AMD-${i}` })),
      },
    ],
  })

/** `lists` is what `rvow confirm` prints on each call, the last one repeating. */
function fake(on: On, answer: string, pending: number, lists: string[]) {
  const ran: string[] = []
  const asked: string[] = []
  let reads = 0
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('ui.toast', async () => ({ value: undefined }))
  on('process.run', async (_$, e) => {
    const line = [...e.argv].slice(1).join(' ')
    ran.push(line)
    if (line === 'hook claude view') return { value: procResult(0, view(pending)) }
    if (line === 'confirm --full') {
      const out = lists[Math.min(reads, lists.length - 1)] ?? ''
      reads += 1
      return { value: procResult(0, out) }
    }
    if (line.startsWith('confirm ')) return { value: procResult(0, 'accepted\n') }
    return { value: procResult(0, 'archived CHG-a1\n') }
  })
  on('tool.call', { tool: 'AskUserQuestion' }, async (_$, e) => {
    const question = e.questions[0]?.question ?? ''
    asked.push(question)
    return { result: { questions: e.questions, answers: { [question]: answer } } }
  })
  return { ran, asked }
}

const call = () => ({ tool: ARCHIVE, change: 'CHG-a1' }) as never
const decided = (ran: string[]) => ran.filter((l) => /^(confirm (?!--full)|archive)/.test(l))

const three = list(
  'd3',
  amd('AMD-1', 'CHG-a1', 'free'),
  amd('AMD-2', 'CHG-a1', 'weaken'),
  amd('AMD-3', 'CHG-a1', 'confirm'),
)

test('one dialog lists the pending amendments, weaken first, and yes accepts then archives', async ($, on) => {
  const { ran, asked } = fake(on, 'Accept and archive', 3, [three])
  const out = JSON.stringify(await $.tool.call(call()))
  expect(asked.length).toBe(1)
  const q = asked[0] ?? ''
  for (const id of ['AMD-1', 'AMD-2', 'AMD-3']) expect(q).toContain(id)
  expect(q.indexOf('AMD-2')).toBeLessThan(q.indexOf('AMD-1'))
  expect(decided(ran)).toEqual(['confirm AMD-2 AMD-1 AMD-3 --accept', 'archive CHG-a1'])
  expect(out).toContain('archived CHG-a1')
})

test('a no leaves the amendments pending and the change active', async ($, on) => {
  const { ran } = fake(on, 'Not yet', 3, [three])
  const out = JSON.stringify(await $.tool.call(call()))
  expect(decided(ran)).toEqual([])
  expect(out).toContain('stays active')
  expect(out).toContain('/rvow-archive CHG-a1')
})

test('an amendment recorded before the answer is never accepted unseen', async ($, on) => {
  const four = list(
    'd4',
    amd('AMD-1', 'CHG-a1', 'free'),
    amd('AMD-2', 'CHG-a1', 'weaken'),
    amd('AMD-3', 'CHG-a1', 'confirm'),
    amd('AMD-4', 'CHG-a1', 'free'),
  )
  const { ran, asked } = fake(on, 'Accept and archive', 3, [three, four, four])
  await $.tool.call(call())
  expect(asked.length).toBe(2)
  expect(asked[0]).not.toContain('AMD-4')
  expect(asked[1]).toContain('AMD-4')
  expect(decided(ran)).toEqual(['confirm AMD-2 AMD-1 AMD-3 AMD-4 --accept', 'archive CHG-a1'])
})

test('with no pending amendment the dialog only asks whether to archive', async ($, on) => {
  const { ran, asked } = fake(on, 'Archive', 0, [''])
  const out = JSON.stringify(await $.tool.call(call()))
  expect(asked.length).toBe(1)
  expect(asked[0]).toContain('Archive it?')
  expect(ran).not.toContain('confirm --full')
  expect(decided(ran)).toEqual(['archive CHG-a1'])
  expect(out).toContain('archived CHG-a1')
})

test('amendments of another change are not listed or accepted', async ($, on) => {
  const mixed = list('dm', amd('AMD-1', 'CHG-zz', 'weaken'), amd('AMD-2', 'CHG-a1', 'free'))
  const { ran, asked } = fake(on, 'Accept and archive', 1, [mixed])
  await $.tool.call(call())
  expect(asked[0]).not.toContain('AMD-1')
  expect(decided(ran)).toEqual(['confirm AMD-2 --accept', 'archive CHG-a1'])
})

test('archiveAmendments keeps one change and puts weaken first', () => {
  const ids = archiveAmendments(three, 'CHG-a1').map((i) => i.id)
  expect(ids).toEqual(['AMD-2', 'AMD-1', 'AMD-3'])
  expect(archiveAmendments(three, 'CHG-zz')).toEqual([])
})

test('a view that counts pending edits but an empty listing still asks before it archives', async ($, on) => {
  const { ran, asked } = fake(on, 'Not yet', 2, [''])
  const out = JSON.stringify(await $.tool.call(call()))
  expect(asked.length).toBe(1)
  expect(asked[0]).toContain('Archive it?')
  expect(decided(ran)).toEqual([])
  expect(out).toContain('stays active')
})

test('with an empty listing a yes archives', async ($, on) => {
  const { ran, asked } = fake(on, 'Archive', 2, [''])
  const out = JSON.stringify(await $.tool.call(call()))
  expect(asked.length).toBe(1)
  expect(decided(ran)).toEqual(['archive CHG-a1'])
  expect(out).toContain('archived CHG-a1')
})

test('/rvow-archive asks the same amendment dialog before it archives', async ($, on) => {
  mock.clock(on)
  on('prompt.submit', async (_$, e) => ({ text: e.text }))
  const { ran, asked } = fake(on, 'Accept and archive', 3, [three])
  await $.command.run(cmd('rvow-archive', 'CHG-a1'))
  expect(asked.length).toBe(1)
  expect(asked[0]).toContain('AMD-2')
  expect(decided(ran)).toEqual(['confirm AMD-2 AMD-1 AMD-3 --accept', 'archive CHG-a1'])
})

test('/rvow-archive leaves the change active when the user says not yet', async ($, on) => {
  mock.clock(on)
  const { ran, asked } = fake(on, 'Not yet', 3, [three])
  const out = JSON.stringify(await $.command.run(cmd('rvow-archive', 'CHG-a1')))
  expect(asked.length).toBe(1)
  expect(decided(ran)).toEqual([])
  expect(out).toContain('stays active')
})
