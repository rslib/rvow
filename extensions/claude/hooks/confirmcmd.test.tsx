import type { On } from 'claude-code'
import type { Engine } from 'claude-code/testing'
import { expect, mock } from 'claude-code/testing'
import { cmd, procResult, test } from './testkit'

let clock: { advance: (ms: number) => Promise<void> }

const ONE =
  'AMD-a1 for CHG-a\n  weaken: TSK-1 verify changed\n  reason (from the agent): edit\n' +
  'digest abc123\ndecide with: rvow confirm <id>... --accept|--reject, or --accept --all <digest>\n'
const TWO =
  'AMD-a1 for CHG-a\n  weaken: TSK-1 verify changed\n  reason (from the agent): one\n' +
  'AMD-b2 for CHG-a\n  weaken: TSK-2 verify changed\n  reason (from the agent): two\n' +
  'digest def456\ndecide with: rvow confirm <id>... --accept|--reject, or --accept --all <digest>\n'

/** `reply` maps a command line (without the binary) to an exit code and its output. */
function fake(on: On, answer: string, reply: (line: string) => [number, string]) {
  clock = mock.clock(on)
  const ran: string[] = []
  const asked: string[] = []
  const prompts: string[] = []
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
  on('prompt.submit', async (_$, e) => {
    prompts.push(e.text)
    return { text: e.text }
  })
  return { ran, asked, prompts }
}

async function run($: Engine, args: string) {
  const out = await $.command.run(cmd('rvow-confirm', args))
  await clock.advance(0)
  return JSON.stringify(out)
}

const decided = (ran: string[]) => ran.filter((l) => l.startsWith('confirm --'))

test('no argument opens the dialog and Accept all decides with the digest', async ($, on) => {
  const { ran, asked } = fake(on, 'Accept all', (line) =>
    line === 'confirm'
      ? [0, ONE]
      : line === 'confirm --accept --all abc123'
        ? [0, 'AMD-a1: accepted\nnext: the plan edit stands; continue the tasks\n']
        : [0, ''],
  )
  const out = await run($, '')
  expect(asked).toHaveLength(1)
  expect(asked[0]).toContain('digest abc123')
  expect(decided(ran)).toEqual(['confirm --accept --all abc123'])
  expect(out).toContain('AMD-a1: accepted')
})

test('all opens the same dialog and Reject all rejects both', async ($, on) => {
  const { ran, asked } = fake(on, 'Reject all', (line) =>
    line === 'confirm'
      ? [0, TWO]
      : line === 'confirm --reject --all def456'
        ? [0, 'AMD-a1: rejected\nAMD-b2: rejected\nnext: undo the rejected plan edit\n']
        : [0, ''],
  )
  const out = await run($, 'all')
  expect(asked).toHaveLength(1)
  expect(decided(ran)).toEqual(['confirm --reject --all def456'])
  expect(out).toContain('AMD-b2: rejected')
})

test('Later decides nothing', async ($, on) => {
  const { ran, asked } = fake(on, 'Later', (line) => (line === 'confirm' ? [0, ONE] : [0, '']))
  const out = await run($, '')
  expect(asked).toHaveLength(1)
  expect(decided(ran)).toEqual([])
  expect(out).toContain('still pending')
})

test('nothing pending says so and asks nothing', async ($, on) => {
  const { ran, asked } = fake(on, 'Accept all', (line) =>
    line === 'confirm' ? [0, 'no pending amendments\n'] : [0, ''],
  )
  const out = await run($, '')
  expect(asked).toEqual([])
  expect(decided(ran)).toEqual([])
  expect(out).toContain('no pending amendments')
})

test('an id still works without a dialog', async ($, on) => {
  const { ran, asked, prompts } = fake(on, 'Accept all', (line) =>
    line === 'confirm --accept AMD-a1' ? [0, 'AMD-a1: accepted\nnext: continue\n'] : [0, ''],
  )
  const out = await run($, 'accept AMD-a1')
  expect(asked).toEqual([])
  expect(decided(ran)).toEqual(['confirm --accept AMD-a1'])
  expect(out).toContain('AMD-a1: accepted')
  expect(prompts).toHaveLength(1)
})
