import type { On } from 'claude-code'
import { expect } from 'claude-code/testing'
import { ASK, CONFIRM } from './format'
import { cmd, procResult, test } from './testkit'

const TWO =
  'AMD-a1 for CHG-a\n  weaken: TSK-1 verify changed\n  reason (from the agent): one\n' +
  'AMD-b2 for CHG-a\n  weaken: TSK-2 verify changed\n  reason (from the agent): two\n' +
  'digest def456\ndecide with: rvow confirm <id>... --accept|--reject, or --accept --all <digest>\n'

/** `answers` are the replies of the dialogs in order; `reply` maps a command line to its output. */
function fake(on: On, answers: string[], reply: (line: string) => [number, string]) {
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
    const answer = answers[asked.length] ?? 'Later'
    asked.push(question)
    return { result: { questions: e.questions, answers: { [question]: answer } } }
  })
  return { ran, asked }
}

const call = (tool: string, input: Record<string, unknown>) => ({ tool, ...input }) as never
const decided = (ran: string[]) => ran.filter((l) => /^confirm AMD-/.test(l))

const reply = (line: string): [number, string] =>
  line === 'confirm'
    ? [0, TWO]
    : line.startsWith('confirm AMD-')
      ? [0, `${line.split(' ')[1]}: decided\n`]
      : [0, '']

test('accept the first and later the second leaves the second pending', async ($, on) => {
  const { ran, asked } = fake(on, ['Accept', 'Later'], reply)
  const out = JSON.stringify(await $.command.run(cmd('rvow-confirm', '')))
  expect(asked).toHaveLength(2)
  expect(asked[0]).toContain('AMD-a1')
  expect(asked[0]).toContain('reason (from the agent): one')
  expect(asked[1]).toContain('AMD-b2')
  expect(decided(ran)).toEqual(['confirm AMD-a1 --accept'])
  expect(out).toContain('AMD-b2 is still pending')
})

test('the id param asks only about that amendment', async ($, on) => {
  const { ran, asked } = fake(on, ['Yes'], reply)
  const out = JSON.stringify(await $.tool.call(call(CONFIRM, { decision: 'reject', id: 'AMD-b2' })))
  expect(asked).toHaveLength(1)
  expect(asked[0]).toContain('AMD-b2')
  expect(asked[0]).not.toContain('AMD-a1')
  expect(decided(ran)).toEqual(['confirm AMD-b2 --reject'])
  expect(out).toContain('AMD-b2: decided')
})

test('the tool asks about each of two amendments', async ($, on) => {
  const { ran, asked } = fake(on, ['Reject', 'Accept'], reply)
  await $.tool.call(call(CONFIRM, { decision: 'reject' }))
  expect(asked).toHaveLength(2)
  expect(decided(ran)).toEqual(['confirm AMD-a1 --reject', 'confirm AMD-b2 --accept'])
})

test('ask with a change runs clarify with that id', async ($, on) => {
  const { ran } = fake(on, [], () => [0, '{"questions":[],"remaining":0}'])
  await $.tool.call(call(ASK, { change: 'CHG-b' }))
  expect(ran).toContain('clarify CHG-b --json')
})

test('ask without a change runs clarify as before', async ($, on) => {
  const { ran } = fake(on, [], () => [0, '{"questions":[],"remaining":0}'])
  await $.tool.call(call(ASK, {}))
  expect(ran).toContain('clarify --json')
})
