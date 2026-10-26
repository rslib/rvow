import type { On } from 'claude-code'
import type { Engine, MountTarget } from 'claude-code/testing'
import { expect } from 'claude-code/testing'
import { cmd, PLACED, procResult, spawnHook, test } from './testkit'

const DIALOG: MountTarget<'terminal', 'Pane'> = {
  plugin: 'rvow',
  surface: 'terminal',
  component: 'Pane',
  requestId: 'rvow-dialog',
  props: {
    title: 'rvow',
    isFocused: true,
    bodyColumns: 80,
    placement: 'inline',
    scroll: { offset: 0, bodyRows: 20 },
    view: {},
  },
}
const NONE = { amendments: 0, questions: 0, blocked: 0, plans: 0 }
const VIEW = {
  branch: 'feat/x',
  changes: [
    {
      id: 'CHG-1',
      title: 'Add the thing',
      stage: 'active',
      tasks: [{ id: 'T1', title: 'one', state: 'proved', reasons: [] }],
      drift: [],
      pending: [],
    },
  ],
  plans: [],
  backlog: [],
  archive: [],
  questions: [],
  next: null,
  needs_you: NONE,
}

const ROUND = {
  questions: [
    {
      ref: 'Q1',
      question: 'Which store?',
      header: 'Store',
      options: [{ label: 'SQLite' }, { label: 'Files' }],
    },
  ],
  remaining: 0,
}

/** A session with AskUserQuestion off: rvow's own pane asks, and a press writes the answer pipe. */
function seed(on: On) {
  const calls: string[] = []
  let opened: () => void = () => undefined
  const open = new Promise<void>((resolve) => {
    opened = resolve
  })
  let pressed: (label: string) => void = () => undefined
  const press = new Promise<string>((resolve) => {
    pressed = resolve
  })
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('env.get', async () => ({ value: '/home/me' }))
  on('ui.status', async () => ({ value: undefined }))
  on('ui.toast', async () => ({ value: undefined }))
  on('ui.close', async () => ({ value: undefined }))
  on('ui.open', async (_$, e) => {
    if (e.id === 'rvow-dialog') opened()
    return PLACED
  })
  on('tool.call', { tool: 'AskUserQuestion' }, async () => ({
    deny: '$.tool.call: no tool named "AskUserQuestion" in this session',
  }))
  on('process.run', async (_$, e) => {
    const argv = e.argv as string[]
    if (argv[0] === 'mktemp') return { value: procResult(0, '/tmp/rvow-dialog-x\n') }
    if (argv[0] === 'mkfifo' || argv[0] === 'rm') return { value: procResult(0, '') }
    if (argv[0] === 'sh') {
      pressed(argv[4] ?? '')
      return { value: procResult(0, '') }
    }
    const rest = argv.slice(1).join(' ')
    calls.push(rest)
    if (rest === 'clarify --json') return { value: procResult(0, JSON.stringify(ROUND)) }
    if (rest === 'decide --list') return { value: procResult(1, '') }
    return { value: procResult(0, rest === 'hook claude view' ? JSON.stringify(VIEW) : '') }
  })
  on('process.spawn', async function* (_$, e) {
    if (e.argv[0] === 'cat') {
      const label = await press
      if (label) yield { stream: 'stdout' as const, text: label }
      return { value: { code: 0, signal: null } }
    }
    calls.push([...e.argv].slice(1).join(' '))
    return yield* spawnHook(() => [0, 'archived'])(_$, e)
  })
  return { calls, open }
}

async function texts($: Engine) {
  const pane = await $.ui.mount(DIALOG)
  return { pane, texts: (await pane.findAll({ type: 'Text' })).map((t) => t.text) }
}

test('with AskUserQuestion off, rvow asks in its own pane and a press answers', async ($, on) => {
  const { calls, open } = seed(on)
  const run = $.command.run(cmd('rvow-archive', 'CHG-1'))
  await open
  const { pane, texts: shown } = await texts($)
  expect(shown.some((t) => t.includes('Archive it?'))).toBe(true)
  await pane.press({ key: 'answer:0' })
  await run
  expect(calls).toContain('archive CHG-1')
})

test('a no in rvow pane leaves the change active', async ($, on) => {
  const { calls, open } = seed(on)
  const run = $.command.run(cmd('rvow-archive', 'CHG-1'))
  await open
  const { pane } = await texts($)
  await pane.press({ key: 'answer:1' })
  const r = await run
  expect(calls).not.toContain('archive CHG-1')
  expect(r.text).toContain('did not archive')
})

test('an open question in rvow pane can go to chat, and the agent is told to record it', async ($, on) => {
  const { calls, open } = seed(on)
  const run = $.tool.call({ tool: 'mcp__rvow__rvow_ask' } as never)
  await open
  const { pane } = await texts($)
  expect(await pane.find({ key: 'answer:2' })).toBeTruthy()
  await pane.press({ key: 'answer:2' })
  const out = JSON.stringify(await run)
  expect(out).toContain('The user answers Q1 in chat')
  expect(out).toContain('rvow answer Q1')
  expect(calls.some((c) => c.startsWith('answer '))).toBe(false)
})
