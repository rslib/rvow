import type { On } from 'claude-code'
import { expect } from 'claude-code/testing'
import { YOLO, yoloTimes } from './format'
import { cmd, procResult, test } from './testkit'

const EDIT = { tool: 'Edit', file_path: '/tmp/x', old_string: 'a', new_string: 'b' } as const

// A time no clock of this session returns, so an entry that carries it comes from the output.
const AT = 1_700_000_000_000
const UNTIL = AT + 1_800_000

/** What `rvow yolo auto --for 30m` prints to the mod: the tier, its minutes and the times it sealed. */
const SWITCHED = `tier: auto\nminutes: 30\nat: ${AT}\nuntil: ${UNTIL}\nhelp[1]:\n  Run \`rvow yolo off\` to end it sooner\n`

type UserMade = { since: number; yolo: { tier: string; at: number; until: number }[] }

/** What a `yolo` run of the fake rvow prints and how it ends. */
type Switch = { out?: string; code?: number }

/**
 * A fake rvow whose `yolo` runs print `switched` and whose hook calls answer nothing, recording the
 * `user_made` of each pre-tool-use call and every run that is not a hook call.
 */
function fakeRvow(on: On, switched: Switch = {}) {
  const seen: UserMade[] = []
  const ran: string[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('ui.toast', async () => ({ value: undefined }))
  on('ui.open', async () => ({ value: { isPlaced: true } }))
  on('process.run', async (_$, e) => {
    const argv = [...e.argv].slice(1)
    if (argv[0] === 'hook') {
      if (argv[2] === 'pre-tool-use' && e.init?.stdin) seen.push(JSON.parse(e.init.stdin).user_made)
      return { value: procResult(0, '') }
    }
    ran.push(argv.join(' '))
    if (argv[0] !== 'yolo') return { value: procResult(0, '') }
    return { value: procResult(switched.code ?? 0, switched.out ?? SWITCHED) }
  })
  on('tool.call', { tool: 'Edit' }, async () => ({ result: 'edited' }))
  return { seen, ran }
}

/** The yolo entries the hook was told the user made, read from the last pre-tool-use call. */
const made = (seen: UserMade[]) => seen.at(-1)?.yolo

/** The user says yes to the yolo tool's dialog. */
const allow = (on: On) =>
  on('tool.call', { tool: 'AskUserQuestion' }, async (_$, e) => ({
    result: {
      questions: e.questions,
      answers: { [e.questions[0]?.question ?? '']: 'Allow' },
    },
  }))

test('/rvow-yolo records the at and until the run printed', async ($, on) => {
  const { seen, ran } = fakeRvow(on)
  await $.command.run(cmd('rvow-yolo', 'auto 30m'))
  await $.tool.call(EDIT)
  expect(ran).toContain('yolo auto --for 30m')
  expect(made(seen)).toEqual([{ tier: 'auto', at: AT, until: UNTIL }])
})

test('the yolo tool records the at and until the run printed', async ($, on) => {
  const { seen, ran } = fakeRvow(on)
  allow(on)
  await $.tool.call({ tool: YOLO, tier: 'auto', for: '30m' } as never)
  await $.tool.call(EDIT)
  expect(ran).toContain('yolo auto --for 30m')
  expect(made(seen)).toEqual([{ tier: 'auto', at: AT, until: UNTIL }])
})

test('/rvow-yolo records nothing when the output carries no times', async ($, on) => {
  const { seen } = fakeRvow(on, { out: 'tier: auto\nminutes: 30\n' })
  await $.command.run(cmd('rvow-yolo', 'auto 30m'))
  await $.tool.call(EDIT)
  expect(made(seen)).toEqual([])
})

test('the yolo tool records nothing when the output carries no times', async ($, on) => {
  const { seen } = fakeRvow(on, { out: 'tier: auto\nminutes: 30\n' })
  allow(on)
  await $.tool.call({ tool: YOLO, tier: 'auto', for: '30m' } as never)
  await $.tool.call(EDIT)
  expect(made(seen)).toEqual([])
})

test('a yolo run that failed records nothing, even with times in its output', async ($, on) => {
  const { seen } = fakeRvow(on, { code: 1 })
  await $.command.run(cmd('rvow-yolo', 'auto 30m'))
  await $.tool.call(EDIT)
  expect(made(seen)).toEqual([])
})

test('the times are read from the output, and only when it holds both', () => {
  expect(yoloTimes(SWITCHED)).toEqual({ at: AT, until: UNTIL })
  expect(yoloTimes(`at: ${AT}\n`)).toBeUndefined()
  expect(yoloTimes(`until: ${UNTIL}\n`)).toBeUndefined()
  expect(yoloTimes('tier: auto\nminutes: 30\n')).toBeUndefined()
  // The line an older rvow prints, with the times in its words and not as fields.
  expect(
    yoloTimes('yolo auto is on for 29 more minutes; the gate still checks at push\n'),
  ).toBeUndefined()
})
