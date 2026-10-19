import type { On } from 'claude-code'
import { expect } from 'claude-code/testing'
import { declinedAcceptReason, repeatAcceptResult, shortReview } from './accept'
import { ACCEPT } from './format'
import { procResult, test } from './testkit'

/** `reply` maps a command line (without the binary) to an exit code and its output. */
function fake(on: On, answer: string, reply: (line: string) => [number, string]) {
  const ran: string[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('ui.toast', async () => ({ value: undefined }))
  on('process.run', async (_$, e) => {
    const line = [...(e as { argv: string[] }).argv].slice(1).join(' ')
    ran.push(line)
    const [code, out] = reply(line)
    return { value: procResult(code, out) }
  })
  const asked: string[] = []
  on('tool.call', { tool: 'AskUserQuestion' }, async (_$, e) => {
    const question = (e as { questions: { question: string }[] }).questions[0]?.question ?? ''
    asked.push(question)
    return {
      result: {
        questions: (e as { questions: unknown[] }).questions,
        answers: { [question]: answer },
      },
    }
  })
  return { ran, asked }
}

const call = (tool: string, input: Record<string, unknown>) => ({ tool, ...input }) as never

test('rvow_accept runs accept once as a plain run', async ($, on) => {
  const { ran } = fake(on, 'Accept', (line) =>
    line.startsWith('review') ? [0, 'the checklist\n'] : [0, 'accepted CHG-a1\nnext: TSK-1 go\n'],
  )
  const out = JSON.stringify(await $.tool.call(call(ACCEPT, { change: 'CHG-a1' })))
  expect(ran.filter((l) => l === 'accept CHG-a1 --yes').length).toBe(1)
  expect(out).toContain('next: TSK-1 go')
})

test('a long accept output reaches the agent cut to 40 lines with the next line kept', async ($, on) => {
  const body = Array.from({ length: 80 }, (_, i) => `detail line ${i + 1}`).join('\n')
  fake(on, 'Accept', (line) =>
    line.startsWith('review') ? [0, 'the checklist\n'] : [0, `${body}\nnext: TSK-1 go\n`],
  )
  const out = JSON.stringify(await $.tool.call(call(ACCEPT, { change: 'CHG-a1' })))
  expect(out).toContain('earlier lines cut')
  expect(out).not.toContain('detail line 1\\n')
  expect(out).toContain('next: TSK-1 go')
})

test('a failed accept still tells the agent why', async ($, on) => {
  fake(on, 'Accept', (line) => {
    return line.startsWith('review')
      ? [0, 'the checklist\n']
      : [1, 'error: CHG-a1 has no passing check\ncode: not-ready\n']
  })
  const out = JSON.stringify(await $.tool.call(call(ACCEPT, { change: 'CHG-a1' })))
  expect(out).toContain('rvow accept CHG-a1 failed')
  expect(out).toContain('code: not-ready')
})

const lines = (n: number) =>
  `${Array.from({ length: n }, (_, i) => `checklist line ${i + 1}`).join('\n')}\n`

test('shortReview shows a short review in full, with no cut line', () => {
  const review = lines(6)
  expect(shortReview(review, 'CHG-a1')).toBe(review)
})

test('shortReview cuts a long review to 10 lines and names rvow review', () => {
  const out = shortReview(lines(40), 'CHG-a1')
  const shown = out.split('\n')
  expect(shown.slice(0, 10)).toEqual(lines(40).split('\n').slice(0, 10))
  expect(shown.length).toBe(11)
  expect(shown[10]).toContain('30 more lines')
  expect(shown[10]).toContain('rvow review CHG-a1')
})

test('a failed accept is not asked again while the plan is unchanged', async ($, on) => {
  const { asked, ran } = fake(on, 'Accept', (line) => {
    return line.startsWith('review')
      ? [0, lines(40)]
      : [1, 'error: CHG-a1 has no passing check\ncode: not-ready\n']
  })
  const first = JSON.stringify(await $.tool.call(call(ACCEPT, { change: 'CHG-a1' })))
  expect(first).toContain('code: not-ready')
  const second = JSON.stringify(await $.tool.call(call(ACCEPT, { change: 'CHG-a1' })))
  // One dialog for both calls, and accept ran once: the repeat returns the old reason.
  expect(asked.length).toBe(1)
  expect(ran.filter((l) => l === 'accept CHG-a1 --yes').length).toBe(1)
  expect(second).toContain('code: not-ready')
  expect(second).toContain('/rvow-accept CHG-a1')
})

test('a declined accept is not asked again while the plan is unchanged', async ($, on) => {
  const { asked, ran } = fake(on, 'Not now', (_line) => {
    return [0, lines(40)]
  })
  const first = JSON.stringify(await $.tool.call(call(ACCEPT, { change: 'CHG-a1' })))
  expect(first).toContain('did not accept CHG-a1')
  const second = JSON.stringify(await $.tool.call(call(ACCEPT, { change: 'CHG-a1' })))
  expect(asked.length).toBe(1)
  expect(ran.filter((l) => l === 'accept CHG-a1 --yes').length).toBe(0)
  expect(second).toContain('did not accept CHG-a1')
})

test('a changed plan after a failure asks again', async ($, on) => {
  let reviews = 0
  const { asked } = fake(on, 'Accept', (line) => {
    if (line.startsWith('review')) {
      reviews++
      return [0, reviews === 1 ? lines(40) : `${lines(40)}one more line\n`]
    }
    return [1, 'error: CHG-a1 has no passing check\ncode: not-ready\n']
  })
  await $.tool.call(call(ACCEPT, { change: 'CHG-a1' }))
  await $.tool.call(call(ACCEPT, { change: 'CHG-a1' }))
  expect(asked.length).toBe(2)
})

test('repeatAcceptResult names the slash command the user can run', () => {
  expect(repeatAcceptResult('the user declined', 'CHG-a1')).toContain('/rvow-accept CHG-a1')
  expect(declinedAcceptReason('CHG-a1')).toContain('did not accept CHG-a1')
})
