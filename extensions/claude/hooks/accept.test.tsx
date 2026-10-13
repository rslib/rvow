import type { On } from 'claude-code'
import { expect, mock } from 'claude-code/testing'
import { acceptTimeoutMs, baselineNotice, planBudget } from './accept'
import { ACCEPT } from './format'
import { procResult, test } from './testkit'

/** `reply` maps a command line (without the binary) to an exit code and its output.
 *  `spawnBody` replaces the child, for a test that needs its own behavior. */
type SpawnBody = (
  line: string,
  reply: (line: string) => [number, string],
) => AsyncGenerator<
  { stream: 'stdout'; text: string },
  { value: { code: number; signal: string | null } }
>

function fake(
  on: On,
  answer: string,
  reply: (line: string) => [number, string],
  spawnBody?: SpawnBody,
) {
  const order: string[] = []
  const ran: string[] = []
  const ranRun: string[] = []
  // The in-memory clock: a wait is held until the test moves the clock past it.
  const clock = mock.clock(on)
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('ui.toast', async () => ({ value: undefined }))
  on('process.run', async (_$, e) => {
    const line = [...(e as { argv: string[] }).argv].slice(1).join(' ')
    ranRun.push(line)
    order.push(line)
    const [code, out] = reply(line)
    return { value: procResult(code, out) }
  })
  // `rvow accept` streams, so a plan longer than the host's ten-minute cap still runs.
  on('process.spawn', async function* (_$, e) {
    const line = [...(e as { argv: string[] }).argv].slice(1).join(' ')
    ran.push(line)
    order.push(line)
    if (spawnBody) return yield* spawnBody(line, reply)
    const [code, out] = reply(line)
    yield { stream: 'stdout' as const, text: out }
    return { value: { code, signal: null } }
  })
  on('tool.call', { tool: 'AskUserQuestion' }, async (_$, e) => {
    const question = (e as { questions: { question: string }[] }).questions[0]?.question ?? ''
    return {
      result: {
        questions: (e as { questions: unknown[] }).questions,
        answers: { [question]: answer },
      },
    }
  })
  return { order, ran, ranRun, clock }
}

const call = (tool: string, input: Record<string, unknown>) => ({ tool, ...input }) as never

/** The view of a plan of four 10-minute tasks. */
const LONG_VIEW = JSON.stringify({
  plans: [
    { id: 'CHG-a1', tasks: 4, task_timeouts_secs: [600, 600, 600, 600], default_timeout_secs: 600 },
  ],
  backlog: [],
})

test('baselineNotice names the number of baseline checks', () => {
  expect(baselineNotice(3)).toBe('rvow accept: 3 baseline checks; this can take a while')
  expect(baselineNotice(1)).toBe('rvow accept: 1 baseline checks; this can take a while')
})

test('acceptTimeoutMs is at least the sum of the task timeouts', () => {
  expect(acceptTimeoutMs(null)).toBe(600_000)
  // Four tasks of 10 minutes each: the mod waits up to 40 minutes.
  expect(acceptTimeoutMs({ tasks: 4, timeouts: [600, 600, 600, 600], defaultSecs: 600 })).toBe(
    2_400_000,
  )
  // A task without a timeout counts the configured default.
  expect(acceptTimeoutMs({ tasks: 3, timeouts: [], defaultSecs: 300 })).toBe(900_000)
  expect(acceptTimeoutMs({ tasks: 2, timeouts: [120], defaultSecs: 300 })).toBe(600_000)
})

test('planBudget reads the accept budget from the hook view', () => {
  const view = {
    plans: [{ id: 'CHG-a', tasks: 2, task_timeouts_secs: [600, 0], default_timeout_secs: 300 }],
  }
  expect(planBudget(view, 'CHG-a')).toEqual({ tasks: 2, timeouts: [600, 0], defaultSecs: 300 })
  expect(planBudget(view, 'CHG-b')).toBe(null)
  expect(planBudget('not a view', 'CHG-a')).toBe(null)
})

test('rvow_accept reads the budget first and streams accept past the host cap', async ($, on) => {
  const { order, ran, ranRun } = fake(on, 'Accept', (line) => {
    if (line === 'hook claude view') return [0, LONG_VIEW]
    return line.startsWith('review')
      ? [0, 'the checklist\n']
      : [0, 'accepted CHG-a1\nnext: TSK-1 go\n']
  })
  const out = JSON.stringify(await $.tool.call(call(ACCEPT, { change: 'CHG-a1' })))
  // The plan's budget is read before accept starts, so the notice can name the count.
  expect(order.indexOf('hook claude view')).toBeLessThan(order.indexOf('accept CHG-a1 --yes'))
  expect(ran).toContain('accept CHG-a1 --yes')
  // A one-shot process.run caps at ten minutes; accept streams, so a long plan is not cut off.
  expect(ranRun).not.toContain('accept CHG-a1 --yes')
  expect(out).toContain('next: TSK-1 go')
})

test('an older rvow without the budget fields still accepts', async ($, on) => {
  const { ran } = fake(on, 'Accept', (line) => {
    if (line === 'hook claude view')
      return [0, JSON.stringify({ plans: [{ id: 'CHG-a1', tasks: 2 }] })]
    return line.startsWith('review') ? [0, 'the checklist\n'] : [0, 'accepted CHG-a1\n']
  })
  const out = JSON.stringify(await $.tool.call(call(ACCEPT, { change: 'CHG-a1' })))
  expect(ran).toContain('accept CHG-a1 --yes')
  expect(out).toContain('accepted CHG-a1')
})

test('a failed accept still tells the agent why', async ($, on) => {
  fake(on, 'Accept', (line) => {
    if (line === 'hook claude view') return [0, LONG_VIEW]
    return line.startsWith('review')
      ? [0, 'the checklist\n']
      : [1, 'error: CHG-a1 has no passing check\ncode: not-ready\n']
  })
  const out = JSON.stringify(await $.tool.call(call(ACCEPT, { change: 'CHG-a1' })))
  expect(out).toContain('rvow accept CHG-a1 failed')
  expect(out).toContain('code: not-ready')
})

test('a silent accept still times out, and the stream ends', async ($, on) => {
  let ended = false
  let release: (() => void) | undefined
  const { clock } = fake(
    on,
    'Accept',
    (line) => {
      if (line === 'hook claude view') return [0, LONG_VIEW]
      return line.startsWith('review') ? [0, 'the checklist\n'] : [0, 'accepted CHG-a1\n']
    },
    // A child that never writes: the deadline, not the child, must end the wait.
    async function* () {
      try {
        await new Promise<void>((resolve) => {
          release = resolve
        })
      } finally {
        ended = true
      }
      return { value: { code: 0, signal: null } }
    },
  )
  const pending = $.tool.call(call(ACCEPT, { change: 'CHG-a1' }))
  await clock.settle()
  await clock.advance(2_400_000)
  release?.()
  const out = JSON.stringify(await pending)
  expect(out).toContain("the plan's task timeouts passed")
  expect(ended).toBe(true)
})
