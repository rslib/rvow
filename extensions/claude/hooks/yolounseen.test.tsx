import type { On } from 'claude-code'
import { expect } from 'claude-code/testing'
import { UNSEEN_YOLO_OPTIONS, type UnseenYolo, unseenYoloQuestion } from './format'
import { procResult, test } from './testkit'

const AT = 1_700_000_000_000
const UNSEEN: UnseenYolo = { tier: 'audit', at: AT, until: AT + 3_600_000 }
const EDIT_REASON = 'rvow: CHG-abc123 is not accepted yet'
const STOP_REASON = 'rvow: TSK-abc123 has no proof'

/** A pre-tool-use reply that denies, with the unseen tier when one is given. */
const denied = (reason: string, unseen?: UnseenYolo) =>
  JSON.stringify({
    hookSpecificOutput: { permissionDecision: 'deny', permissionDecisionReason: reason },
    ...(unseen ? { unseen_yolo: unseen } : {}),
  })

/** A stop reply that holds the stop back, with the unseen tier when one is given. */
const held = (reason: string, unseen?: UnseenYolo) =>
  JSON.stringify({ decision: 'block', reason, ...(unseen ? { unseen_yolo: unseen } : {}) })

const edit = { tool: 'Edit', file_path: '/tmp/a.txt', old_string: 'a', new_string: 'b' } as never
const stopTurn = {
  session_id: 's1',
  transcript_path: '/t/s1.jsonl',
  cwd: '/tmp',
  stop_hook_active: false,
}

type Reply = {
  /** What the pre-tool-use hook answers, one reply per call. */
  pre?: string[]
  /** What the stop hook answers, one reply per call. */
  stop?: string[]
  /** The user's answer in the unseen yolo dialog; none when they never answer. */
  answer?: string
  /** Makes rvow's own dialog fail to open, which is a session that never answers. */
  noDialog?: boolean
}

type Call = { event: string; input: Record<string, unknown> }

/**
 * A fake rvow that answers each hook call from `replies`, and records every hook call, every
 * command run outside the hooks, and every question put to the user.
 */
function fakeRvow(on: On, replies: Reply = {}) {
  const calls: Call[] = []
  const ran: string[][] = []
  const asked: string[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('ui.toast', async () => ({ value: undefined }))
  on('ui.open', async () => ({ value: { isPlaced: true } }))
  on('ui.close', async () => ({ value: undefined }))
  on('process.run', async (_$, e) => {
    const argv = [...e.argv].slice(1)
    const input: Record<string, unknown> = e.init?.stdin ? JSON.parse(e.init.stdin) : {}
    if (argv[0] === 'hook' && argv[1] === 'claude') {
      const event = argv[2] ?? ''
      calls.push({ event, input })
      const queue = event === 'pre-tool-use' ? replies.pre : event === 'stop' ? replies.stop : []
      return { value: procResult(0, queue?.shift() ?? '') }
    }
    if (argv[0] === 'mktemp') {
      const dir = replies.noDialog ? '' : '/tmp/rvow-dialog-x'
      return { value: procResult(replies.noDialog ? 1 : 0, dir) }
    }
    ran.push(argv)
    return { value: procResult(0, '') }
  })
  on('tool.call', { tool: 'AskUserQuestion' }, async (_$, e) => {
    asked.push(e.questions[0]?.question ?? '')
    if (replies.answer === undefined)
      return { deny: '$.tool.call: no tool named "AskUserQuestion" in this session' }
    return {
      result: {
        questions: e.questions,
        answers: { [e.questions[0]?.question ?? '']: replies.answer },
      },
    }
  })
  on('tool.call', { tool: 'Edit' }, async () => ({ result: 'edited' }))
  // The stop the plugin's handler wraps: a call with no answer lets the turn end.
  on('classic.Stop', async () => ({}))
  return { calls, ran, asked }
}

const hookCalls = (calls: Call[], event: string) => calls.filter((c) => c.event === event)
const yoloRuns = (ran: string[][]) => ran.filter((argv) => argv[0] === 'yolo')
const made = (call: Call | undefined) =>
  call?.input.user_made as { since: number; yolo: UnseenYolo[] } | undefined

test('the status and the stop hook carry what the user made', async ($, on) => {
  const { calls } = fakeRvow(on, { stop: [''] })
  on('tool.call', { tool: 'Bash' }, async () => ({ result: 'ok' }))
  await $.tool.call({ tool: 'Bash', command: 'rvow status' } as never)
  await $.classic.Stop(stopTurn as never)
  for (const event of ['status', 'stop']) {
    const record = made(hookCalls(calls, event)[0])
    expect(typeof record?.since).toBe('number')
    expect(record?.yolo).toEqual([])
  }
})

test('a Keep puts the tier in the record and the call is asked once more', async ($, on) => {
  const { calls, asked } = fakeRvow(on, {
    pre: [denied(EDIT_REASON, UNSEEN), ''],
    answer: 'Keep',
  })
  const out = await $.tool.call(edit)
  expect(asked).toHaveLength(1)
  expect(asked[0]).toContain('yolo audit is on until')
  expect(asked[0]).toContain('did not see you switch it on')
  expect(asked[0]).toContain('rvow yolo')
  const pres = hookCalls(calls, 'pre-tool-use')
  expect(pres).toHaveLength(2)
  expect(made(pres[1])?.yolo).toEqual([UNSEEN])
  expect(JSON.stringify(out)).toContain('edited')
})

test('a Turn off runs rvow yolo off and the call stays denied', async ($, on) => {
  const { calls, ran, asked } = fakeRvow(on, {
    pre: [denied(EDIT_REASON, UNSEEN)],
    answer: 'Turn off',
  })
  const out = await $.tool.call(edit)
  expect(asked).toHaveLength(1)
  expect(yoloRuns(ran)).toEqual([['yolo', 'off']])
  // The call is denied from the reply it had, and is not asked again.
  expect(hookCalls(calls, 'pre-tool-use')).toHaveLength(1)
  expect(JSON.stringify(out)).toContain(EDIT_REASON)
  expect(JSON.stringify(out)).not.toContain('edited')
  // The status line is redrawn from the record, which holds no yolo tier.
  expect(made(hookCalls(calls, 'status')[0])?.yolo).toEqual([])
})

test('a dialog that never answers denies the call and asks no more for that tier', async ($, on) => {
  const { calls, ran, asked } = fakeRvow(on, {
    pre: [denied(EDIT_REASON, UNSEEN), denied(EDIT_REASON, UNSEEN)],
    noDialog: true,
  })
  const first = await $.tool.call(edit)
  const second = await $.tool.call(edit)
  expect(asked).toHaveLength(1)
  expect(hookCalls(calls, 'pre-tool-use')).toHaveLength(2)
  expect(yoloRuns(ran)).toEqual([])
  for (const out of [first, second]) {
    expect(JSON.stringify(out)).toContain(EDIT_REASON)
    expect(JSON.stringify(out)).not.toContain('edited')
  }
})

test('a reply without an unseen tier asks nothing', async ($, on) => {
  const { asked, ran } = fakeRvow(on, { pre: [denied(EDIT_REASON)], answer: 'Keep' })
  const out = await $.tool.call(edit)
  expect(asked).toEqual([])
  expect(yoloRuns(ran)).toEqual([])
  expect(JSON.stringify(out)).toContain(EDIT_REASON)
  expect(JSON.stringify(out)).not.toContain('edited')
})

test('an unseen tier that another yolo file reports is asked about once more', async ($, on) => {
  const later: UnseenYolo = { tier: 'bypass', at: AT + 5_000, until: AT + 3_600_000 }
  const { asked } = fakeRvow(on, {
    pre: [denied(EDIT_REASON, UNSEEN), denied(EDIT_REASON, later)],
    answer: 'Turn off',
  })
  await $.tool.call(edit)
  await $.tool.call(edit)
  expect(asked).toHaveLength(2)
  expect(asked[1]).toContain('yolo bypass is on until')
})

test('a Keep at the stop uses the answer the stop hook gives next', async ($, on) => {
  const { calls, asked } = fakeRvow(on, {
    stop: [held(STOP_REASON, UNSEEN), ''],
    answer: 'Keep',
  })
  const out = await $.classic.Stop(stopTurn as never)
  expect(asked).toHaveLength(1)
  expect(asked[0]).toContain('yolo audit is on until')
  const stops = hookCalls(calls, 'stop')
  expect(stops).toHaveLength(2)
  expect(made(stops[1])?.yolo).toEqual([UNSEEN])
  expect(JSON.stringify(out)).not.toContain(STOP_REASON)
})

test('a Turn off at the stop switches yolo off and holds the stop with its reason', async ($, on) => {
  const { calls, ran } = fakeRvow(on, { stop: [held(STOP_REASON, UNSEEN)], answer: 'Turn off' })
  const out = await $.classic.Stop(stopTurn as never)
  expect(yoloRuns(ran)).toEqual([['yolo', 'off']])
  expect(hookCalls(calls, 'stop')).toHaveLength(1)
  expect(JSON.stringify(out)).toContain(STOP_REASON)
})

test('a stop with no answer holds back and asks no more for that tier', async ($, on) => {
  const { asked } = fakeRvow(on, {
    stop: [held(STOP_REASON, UNSEEN), held(STOP_REASON, UNSEEN)],
    noDialog: true,
  })
  const first = await $.classic.Stop(stopTurn as never)
  const second = await $.classic.Stop(stopTurn as never)
  expect(asked).toHaveLength(1)
  expect(JSON.stringify(first)).toContain(STOP_REASON)
  expect(JSON.stringify(second)).toContain(STOP_REASON)
})

test('a stop reply without an unseen tier asks nothing', async ($, on) => {
  const { asked } = fakeRvow(on, { stop: [held(STOP_REASON)], answer: 'Keep' })
  const out = await $.classic.Stop(stopTurn as never)
  expect(asked).toEqual([])
  expect(JSON.stringify(out)).toContain(STOP_REASON)
})

test('the question names the tier, its end and who may have switched it on', () => {
  expect(UNSEEN_YOLO_OPTIONS).toEqual(['Keep', 'Turn off'])
  const question = unseenYoloQuestion({ tier: 'bypass', until: AT + 3_600_000 })
  for (const word of [
    'yolo bypass is on until',
    'did not see you switch it on',
    '/rvow-yolo',
    'Keep it or turn it off',
    'every hook only audits',
  ])
    expect(question).toContain(word)
})

test('a call that comes while the dialog is open waits for the same answer', async ($, on) => {
  const { calls, asked } = fakeRvow(on, {
    pre: [denied(EDIT_REASON, UNSEEN), denied(EDIT_REASON, UNSEEN), '', ''],
    answer: 'Keep',
  })
  const [first, second] = await Promise.all([$.tool.call(edit), $.tool.call(edit)])
  expect(asked).toHaveLength(1)
  expect(hookCalls(calls, 'pre-tool-use')).toHaveLength(4)
  expect(JSON.stringify(first)).toContain('edited')
  expect(JSON.stringify(second)).toContain('edited')
})
