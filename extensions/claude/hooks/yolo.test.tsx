import type { On } from 'claude-code'
import { expect, test } from 'claude-code/testing'
import { YOLO, YOLO_DESCRIPTION, yoloArgv } from './format'
import { procResult } from './testkit'

function fake(on: On, answer: string) {
  const ran: string[][] = []
  const asked: string[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('process.run', async (_$, e) => {
    ran.push([...e.argv].slice(1))
    return { value: procResult(0, 'tier: auto\nminutes: 30\n') }
  })
  on('tool.call', { tool: 'AskUserQuestion' }, async (_$, e) => {
    const question = e.questions[0]?.question ?? ''
    asked.push(question)
    return { result: { questions: e.questions, answers: { [question]: answer } } }
  })
  return { ran, asked }
}

const yolo = (input: Record<string, unknown>) => ({ tool: YOLO, ...input }) as never
const switched = (ran: string[][]) => ran.filter((argv) => argv[0] === 'yolo')

test('a tier is switched on only after the user allows it', async ($, on) => {
  const { ran, asked } = fake(on, 'Allow')
  const out = await $.tool.call(yolo({ tier: 'auto', for: '30m' }))
  expect(asked[0]).toContain('yolo auto for 30m')
  expect(asked[0]).toContain('The gate still checks at push')
  expect(switched(ran)).toEqual([['yolo', 'auto', '--for', '30m']])
  expect(JSON.stringify(out)).toContain('tier: auto')
})

test('a tier the user refuses is not switched on', async ($, on) => {
  const { ran } = fake(on, 'Deny')
  const out = await $.tool.call(yolo({ tier: 'bypass' }))
  expect(switched(ran)).toEqual([])
  expect(JSON.stringify(out)).toContain('The user declined')
})

test('off and the status need no dialog', async ($, on) => {
  const { ran, asked } = fake(on, 'Deny')
  await $.tool.call(yolo({ tier: 'off' }))
  await $.tool.call(yolo({}))
  expect(asked).toEqual([])
  expect(switched(ran)).toEqual([['yolo', 'off'], ['yolo']])
})

test('input that is not allowed is refused before anything runs', async ($, on) => {
  const { ran, asked } = fake(on, 'Allow')
  for (const input of [
    { tier: 'root' },
    { for: '2h' },
    { tier: 'off', for: '2h' },
    { tier: 'auto', for: 'soon' },
    { x: 1 },
  ]) {
    expect(JSON.stringify(await $.tool.call(yolo(input)))).toContain('rvow:')
  }
  expect(asked).toEqual([])
  expect(switched(ran)).toEqual([])
})

test('the arguments and the description are exact', () => {
  expect(yoloArgv({ tier: 'audit' })).toEqual(['yolo', 'audit'])
  expect(yoloArgv({})).toEqual(['yolo'])
  for (const word of ['audit', 'auto', 'bypass', 'off', 'gate still checks', 'rvow block']) {
    expect(YOLO_DESCRIPTION).toContain(word)
  }
})
