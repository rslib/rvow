import type { On } from 'claude-code'
import type { Engine } from 'claude-code/testing'
import { expect, mock } from 'claude-code/testing'
import { parseHello, readAskAnswer, readHookAnswer } from './engine'
import { ACCEPT, CONFIRM } from './format'
import { procResult, test } from './testkit'

const BASE = { session_id: 's1', transcript_path: '/t/s1.jsonl', cwd: '/tmp' }
const START = { cwd: '/tmp', surface: null, isInteractive: false }
const SOCKET = '/home/me/.local/state/rvow/run/engine.sock'
const HELLO = { proto_major: 1, proto_minor: 0, build: '0.4.0', env: 'env-abc', ui: 'pane' }
const HELLO_LINE = JSON.stringify({ socket: SOCKET, hello: HELLO })
const ENGINE_REASON = 'rvow: the file is not part of the change'
const PROCESS_REASON = 'rvow: the file is not part of the leased task'

/** What `rvow hook claude pre-tool-use` prints to deny: the shape the engine sends back too. */
const deny = (reason: string) => ({
  hookSpecificOutput: { permissionDecision: 'deny', permissionDecisionReason: reason },
})
/** One `/v1/hook` answer: the text the hook would have printed, or no text at all. */
const hookReply = (output: string | null) => JSON.stringify({ reply: { output }, mismatch: null })
/** The answer of an engine that runs in another environment or speaks another protocol. */
const MISMATCH = JSON.stringify({ reply: null, mismatch: { env: 'other', proto_major: 2 } })
const askReply = (outcome: string, text: string) => JSON.stringify({ outcome, text })

const EDIT = {
  tool: 'Edit',
  file_path: '/tmp/a.txt',
  old_string: 'a',
  new_string: 'b',
} as never
const call = (tool: string, input: Record<string, unknown> = {}) => ({ tool, ...input }) as never

/**
 * Fakes the rvow binary: every command line (without the binary) lands in `ran`, and `reply` maps it
 * to an exit code and its stdout.
 */
function fakeRvow(on: On, reply: (line: string) => [number, string]) {
  const ran: string[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('ui.toast', async () => ({ value: undefined }))
  on('process.run', async (_$, e) => {
    const line = [...(e.argv as string[])].slice(1).join(' ')
    ran.push(line)
    const [code, out] = reply(line)
    return { value: procResult(code, out) }
  })
  return ran
}

/**
 * The rvow of a hook test: the hello `hello` (an empty one is a daemon that did not answer), and a
 * deny for every event but session-start, which always answers no brief.
 */
function fakeHook(on: On, hello: string, reason: string) {
  return fakeRvow(on, (line) => {
    if (line === 'daemon --print-hello') return [hello ? 0 : 1, hello]
    if (line.startsWith('hook claude ') && line !== 'hook claude session-start')
      return [0, JSON.stringify(deny(reason))]
    return [0, '']
  })
}

/** The rvow of an accept test: the hello, the checklist of the review, and an accepted change. */
function fakeAccept(on: On, hello: string) {
  return fakeRvow(on, (line) => {
    if (line === 'daemon --print-hello') return [0, hello]
    if (line.startsWith('review ')) return [0, 'the checklist\n']
    if (line.startsWith('accept ')) return [0, 'accepted CHG-a1\nnext: TSK-1 go\n']
    return [0, '']
  })
}

/** One answer of the fake engine: the status and body it sends, or a socket that refuses. */
type Answer = { status: number; text: string } | 'fail'

/** Fakes the engine socket: every POST lands in `posted`, and `answer` gives the reply of its route. */
function fakeEngine(on: On, answer: (route: string, body: string) => Answer) {
  const posted: { url: string; route: string; body: string }[] = []
  on('http.fetch', async (_$, e) => {
    const route = new URL(e.url).pathname
    const body = e.init?.body ?? ''
    posted.push({ url: e.url, route, body })
    const a = answer(route, body)
    if (a === 'fail') throw new Error('connect ECONNREFUSED /tmp/engine.sock')
    return { value: { status: a.status, ok: a.status === 200, headers: {}, text: a.text } }
  })
  return posted
}

/**
 * Fakes the harness dialog, which is what the tools show when no host draws the question: it records
 * each question and answers `pick`. A test whose answer came from a host passes no `pick`, so a
 * dialog that opens there fails the call instead of passing as one nobody drew.
 */
function fakeDialog(on: On, pick?: string) {
  const asked: string[] = []
  on('tool.call', { tool: 'AskUserQuestion' }, async (_$, e) => {
    const question = e.questions[0]?.question ?? ''
    asked.push(question)
    if (pick === undefined) throw new Error('a dialog opened where a host drew the question')
    return { result: { questions: e.questions, answers: { [question]: pick } } }
  })
  return asked
}

/** Starts a session and runs its first prompt: the session-start hook, then the engine's hello. */
async function start($: Engine, on: On) {
  mock.clock(on)
  on('session.start', async (_$, e) => ({ cwd: e.cwd }))
  on('command.register', () => ({ value: {} }) as never)
  on('tool.register', () => ({ value: {} }) as never)
  on('classic.SessionStart', () => ({}))
  on('classic.UserPromptSubmit', () => ({}))
  await $.session.start(START)
  await $.classic.UserPromptSubmit({ ...BASE, prompt: 'hi' })
}

/** Every hook test runs the same tool call: an Edit that a deny must keep from running. */
const fakeEdit = (on: On) => {
  let edited = 0
  on('tool.call', { tool: 'Edit' }, async () => {
    edited++
    return { result: 'edited' }
  })
  return () => edited
}

test('a hook call goes through the engine, whose deny denies the call with no rvow process', async ($, on) => {
  const ran = fakeHook(on, HELLO_LINE, ENGINE_REASON)
  const posted = fakeEngine(on, () => ({
    status: 200,
    text: hookReply(JSON.stringify(deny(ENGINE_REASON))),
  }))
  const edited = fakeEdit(on)
  await start($, on)
  const out = await $.tool.call(EDIT)
  expect(out.deny).toContain(ENGINE_REASON)
  expect(edited()).toBe(0)
  expect(ran).not.toContain('hook claude pre-tool-use')
  expect(posted.map((p) => p.route)).toEqual(['/v1/hook'])
  expect(posted[0]?.url).toBe('http://rvow/v1/hook')
  const sent: {
    hello: unknown
    call: { harness: string; event: string; input: Record<string, unknown> }
  } = JSON.parse(posted[0]?.body ?? '{}')
  expect(sent.hello).toEqual(HELLO)
  expect(sent.call.harness).toBe('claude')
  expect(sent.call.event).toBe('pre-tool-use')
  expect(sent.call.input.tool_name).toBe('Edit')
  expect(sent.call.input.cwd).toBe('/tmp')
})

test('an engine that fails or refuses falls back to rvow hook, which still denies', async ($, on) => {
  const ran = fakeHook(on, HELLO_LINE, PROCESS_REASON)
  let answer: Answer = 'fail'
  fakeEngine(on, () => answer)
  const edited = fakeEdit(on)
  await start($, on)
  expect((await $.tool.call(EDIT)).deny).toContain(PROCESS_REASON)
  // 403 is the engine saying this process is not a recorded harness; the process answers too.
  answer = { status: 403, text: '{"error":"not a harness"}' }
  expect((await $.tool.call(EDIT)).deny).toContain(PROCESS_REASON)
  expect(edited()).toBe(0)
  expect(ran.filter((l) => l === 'hook claude pre-tool-use')).toHaveLength(2)
})

test('a hello that never arrives leaves every call on rvow hook', async ($, on) => {
  const ran = fakeHook(on, '', PROCESS_REASON)
  const posted = fakeEngine(on, () => ({
    status: 200,
    text: hookReply(JSON.stringify(deny(ENGINE_REASON))),
  }))
  const edited = fakeEdit(on)
  await start($, on)
  expect((await $.tool.call(EDIT)).deny).toContain(PROCESS_REASON)
  expect(edited()).toBe(0)
  expect(posted).toEqual([])
  expect(ran).toContain('hook claude pre-tool-use')
})

test('a mismatch makes every later call run rvow hook, and never the engine again', async ($, on) => {
  const ran = fakeHook(on, HELLO_LINE, PROCESS_REASON)
  const posted = fakeEngine(on, () => ({ status: 200, text: MISMATCH }))
  const edited = fakeEdit(on)
  await start($, on)
  expect((await $.tool.call(EDIT)).deny).toContain(PROCESS_REASON)
  expect((await $.tool.call(EDIT)).deny).toContain(PROCESS_REASON)
  expect(edited()).toBe(0)
  expect(posted).toHaveLength(1)
  expect(ran.filter((l) => l === 'hook claude pre-tool-use')).toHaveLength(2)
})

test('three engine failures in a row retire the engine for the rest of the session', async ($, on) => {
  const ran = fakeHook(on, HELLO_LINE, PROCESS_REASON)
  let failures = 0
  const posted = fakeEngine(on, () => {
    failures++
    return failures <= 3 ? 'fail' : { status: 200, text: hookReply(null) }
  })
  const edited = fakeEdit(on)
  await start($, on)
  for (let i = 0; i < 5; i++) await $.tool.call(EDIT)
  expect(posted).toHaveLength(3)
  expect(edited()).toBe(0)
  expect(ran.filter((l) => l === 'hook claude pre-tool-use')).toHaveLength(5)
})

test('an ask that no host answers shows the accept dialog as today', async ($, on) => {
  const ran = fakeAccept(on, HELLO_LINE)
  const posted = fakeEngine(on, () => ({ status: 200, text: askReply('no-host', '') }))
  const asked = fakeDialog(on, 'Accept')
  const edited = fakeEdit(on)
  await start($, on)
  const out = await $.tool.call(call(ACCEPT, { change: 'CHG-a1' }))
  expect(edited()).toBe(0)
  expect(asked).toHaveLength(1)
  expect(asked[0]).toContain('Accept CHG-a1?')
  expect(ran).toContain('accept CHG-a1 --yes')
  expect(JSON.stringify(out)).toContain('next: TSK-1 go')
  // The ask comes first; the refresh the dialog path runs goes through the engine too.
  expect(posted[0]?.route).toBe('/v1/ask')
  expect(posted.slice(1).every((p) => p.route === '/v1/hook')).toBe(true)
  const sent: {
    kind: string
    change: string
    cwd: string
    session: string | null
    hello: unknown
  } = JSON.parse(posted[0]?.body ?? '{}')
  expect(sent.kind).toBe('accept')
  expect(sent.change).toBe('CHG-a1')
  expect(sent.cwd).toBe('/tmp')
  expect(sent.session).toBe('s1')
  expect(sent.hello).toEqual(HELLO)
})

test('an ask a host answers returns its text and opens no dialog', async ($, on) => {
  const ran = fakeAccept(on, HELLO_LINE)
  const hosted = 'The user accepted CHG-a1; next TSK-1 go.'
  fakeEngine(on, () => ({ status: 200, text: askReply('done', hosted) }))
  const asked = fakeDialog(on)
  const edited = fakeEdit(on)
  await start($, on)
  const out = await $.tool.call(call(ACCEPT, { change: 'CHG-a1' }))
  expect(edited()).toBe(0)
  expect(asked).toEqual([])
  expect(JSON.stringify(out)).toContain(hosted)
  expect(ran).not.toContain('review CHG-a1')
  expect(ran).not.toContain('accept CHG-a1 --yes')
})

test('a confirm asks the host for the branch, since it names no change', async ($, on) => {
  const ran = fakeHook(on, HELLO_LINE, PROCESS_REASON)
  const hosted = 'The user accepted both pending plan edits.'
  const posted = fakeEngine(on, () => ({ status: 200, text: askReply('done', hosted) }))
  const asked = fakeDialog(on)
  fakeEdit(on)
  await start($, on)
  const out = await $.tool.call(call(CONFIRM))
  expect(asked).toEqual([])
  expect(JSON.stringify(out)).toContain(hosted)
  expect(ran).not.toContain('confirm --full')
  const sent: { kind: string; change: string; session: string | null } = JSON.parse(
    posted[0]?.body ?? '{}',
  )
  expect(sent.kind).toBe('confirm')
  expect(sent.change).toBe('')
  expect(sent.session).toBe('s1')
})

test('a session-start hook runs rvow hook in the process, never the engine', async ($, on) => {
  const ran = fakeHook(on, HELLO_LINE, ENGINE_REASON)
  const posted = fakeEngine(on, () => ({
    status: 200,
    text: hookReply(JSON.stringify(deny(ENGINE_REASON))),
  }))
  fakeEdit(on)
  await start($, on)
  // A second prompt runs the session-start hook again, with the engine of the session in hand.
  await $.classic.UserPromptSubmit({ ...BASE, prompt: 'again' })
  expect(ran.filter((l) => l === 'hook claude session-start')).toHaveLength(2)
  // The hello is asked for once per session, however many prompts come.
  expect(ran.filter((l) => l === 'daemon --print-hello')).toHaveLength(1)
  expect(posted).toEqual([])
})

test('parseHello reads the one line, and nothing else', () => {
  expect(parseHello(HELLO_LINE)).toEqual({ socket: SOCKET, hello: HELLO })
  expect(parseHello(`${HELLO_LINE}\n`)).toEqual({ socket: SOCKET, hello: HELLO })
  expect(parseHello('')).toBeNull()
  expect(parseHello('not json')).toBeNull()
  expect(parseHello(JSON.stringify({ hello: HELLO }))).toBeNull()
  expect(parseHello(JSON.stringify({ socket: '', hello: HELLO }))).toBeNull()
  expect(parseHello(JSON.stringify({ socket: SOCKET, hello: null }))).toBeNull()
})

test('readHookAnswer reads the output, a mismatch, and an answer to fall back on', () => {
  expect(readHookAnswer(hookReply(JSON.stringify(deny(ENGINE_REASON))))).toEqual({
    kind: 'out',
    out: deny(ENGINE_REASON),
  })
  expect(readHookAnswer(hookReply(null))).toEqual({ kind: 'out', out: undefined })
  // Text that is JSON but not an object is no output, as it is no output through `rvow hook`.
  expect(readHookAnswer(hookReply('5'))).toEqual({ kind: 'out', out: undefined })
  expect(readHookAnswer(MISMATCH)).toEqual({ kind: 'mismatch' })
  expect(readHookAnswer(hookReply('rvow: not json'))).toEqual({ kind: 'bad' })
  expect(readHookAnswer(JSON.stringify({ reply: null }))).toEqual({ kind: 'bad' })
  expect(readHookAnswer('{}')).toEqual({ kind: 'bad' })
  expect(readHookAnswer('')).toEqual({ kind: 'bad' })
})

test('readAskAnswer reads an outcome, and null for anything else', () => {
  expect(readAskAnswer(askReply('done', 'accepted'))).toEqual({ outcome: 'done', text: 'accepted' })
  expect(readAskAnswer(askReply('no-host', ''))).toEqual({ outcome: 'no-host', text: '' })
  expect(readAskAnswer(askReply('waiting', 'the user left'))).toEqual({
    outcome: 'waiting',
    text: 'the user left',
  })
  expect(readAskAnswer(askReply('later', 'x'))).toBeNull()
  expect(readAskAnswer('not json')).toBeNull()
})
