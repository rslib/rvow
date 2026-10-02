import { expect, test } from 'claude-code/testing'

const CHANGE = {
  id: 'CHG-1',
  title: 'Add the thing',
  stage: 'active',
  tasks: [
    { id: 'T1', title: 'proved one', state: 'proved', reasons: [] },
    { id: 'T2', title: 'failed one', state: 'failed', reasons: [] },
    { id: 'T3', title: 'stale one', state: 'stale', reasons: ['code'] },
    { id: 'T4', title: 'flaky one', state: 'flaky', reasons: [] },
    { id: 'T5', title: 'blocked one', state: 'blocked', reasons: [] },
    { id: 'T6', title: 'missing one', state: 'missing', reasons: [] },
  ],
  drift: [{ class: 'weaken', what: 'spec shrank' }],
  pending: [{ id: 'AMD-1', reason: 'reword' }],
}
const NONE = { amendments: 0, questions: 0, blocked: 0, plans: 0 }
const ASKING = {
  branch: 'feat/x',
  changes: [],
  plans: [{ id: 'CHG-2', title: 'Later plan', tasks: 3, questions: 1, issues: 0, ready: false }],
  questions: [{ change: 'CHG-2', question: 'Which store?', recommended: 'sqlite' }],
  next: null,
  needs_you: { ...NONE, questions: 1, plans: 1 },
}
const READY = {
  ...ASKING,
  plans: [{ id: 'CHG-3', title: 'Try rvow', tasks: 2, questions: 0, issues: 0, ready: true }],
  questions: [],
  needs_you: { ...NONE, plans: 1 },
}
const ACTIVE = {
  branch: 'feat/x',
  changes: [CHANGE],
  plans: [],
  questions: [],
  next: { change: 'CHG-1', task: 'T2', title: 'failed one', verify: 'cargo test', state: 'failed' },
  needs_you: { ...NONE, amendments: 1, blocked: 1 },
}
const LISTING =
  'AMD-1 for CHG-1\n  weaken: spec shrank\n  reason (from the agent): reword\ndigest abc123'

const PANE = {
  plugin: 'rvow',
  surface: 'terminal',
  component: 'Pane',
  requestId: 'rvow',
  props: { title: 'rvow', isFocused: true, bodyColumns: 80, placement: 'inline' } as any,
} as const
const BAND = {
  plugin: 'rvow',
  surface: 'terminal',
  component: 'AbovePrompt',
  props: { hasSurvey: false, isWorking: false, maxRows: 10, bodyColumns: 80 } as any,
} as const

/** Fakes rvow: the view for `hook claude view`, the listing for `confirm`; records every argv. */
async function seed($: any, on: any, view: object | null) {
  const calls: string[][] = []
  on('session.cwd', async () => ({ value: '/tmp' }) as any)
  on('process.run', async (_$: any, e: any) => {
    const argv: string[] = e.argv ?? e.command
    calls.push(argv)
    const rest = argv.slice(1).join(' ')
    const stdout =
      rest === 'hook claude view'
        ? view
          ? JSON.stringify(view)
          : ''
        : rest === 'confirm'
          ? LISTING
          : ''
    return { value: { exitCode: 0, stdout, stderr: '' } } as any
  })
  on('ui.status', async () => ({ value: undefined }) as any)
  on('ui.open', async () => ({ value: undefined }) as any)
  on('ui.render', { component: 'AbovePrompt' }, async ($: any, e: any) => {
    const { Text } = $.ui.resolve(e)
    return <Text>fallthrough</Text>
  })
  await $.command.run({ command: 'rvow-status', args: '', origin: { kind: 'composer' } })
  return calls
}

// A rough text sketch of a drawn tree, for eyeballing the layout: borders,
// paddings, gaps and space-between rows; no colors.
type El = { type: string; props: Record<string, any>; children?: unknown[] }
const elems = (n: El): El[] =>
  ((n.children ?? []) as unknown[]).flat(9).filter((c): c is El => !!c && typeof c === 'object')
const textOf = (n: unknown): string =>
  typeof n === 'string' || typeof n === 'number'
    ? String(n)
    : Array.isArray(n)
      ? n.map(textOf).join('')
      : n && typeof n === 'object'
        ? ((n as El).children ?? []).map(textOf).join('')
        : ''
function natural(n: El): string {
  if (n.type === 'Button')
    return n.props?.hotkey ? `${n.props.hotkey}: ${n.props.label}` : n.props?.label
  if (n.type === 'Text') return textOf(n)
  return elems(n)
    .map(natural)
    .join(' '.repeat(n.props?.gap ?? 0))
}
function sketch(n: El, width: number): string[] {
  if (n.type !== 'Box') return [natural(n).slice(0, width)]
  const p = n.props ?? {}
  const pad = p.paddingX ?? p.padding ?? 0
  const inner = width - (p.borderStyle ? 2 : 0) - 2 * pad
  const kids = elems(n)
  let lines: string[] = []
  if (p.flexDirection === 'column') {
    kids.forEach((k, i) => {
      if (i > 0) lines.push(...Array(p.gap ?? 0).fill(''))
      lines.push(...Array(k.props?.marginTop ?? 0).fill(''))
      lines.push(...sketch(k, inner))
      lines.push(...Array(k.props?.marginBottom ?? 0).fill(''))
    })
  } else {
    const parts = kids.map(natural)
    const used = parts.join('').length
    const sep =
      p.justifyContent === 'space-between' && parts.length === 2
        ? ' '.repeat(Math.max(1, inner - used))
        : ' '.repeat(p.gap ?? 0)
    lines = [parts.join(sep).slice(0, inner)]
  }
  lines = lines.map((l) => ' '.repeat(pad) + l.padEnd(inner) + ' '.repeat(pad))
  if (!p.borderStyle) return lines
  return [`╭${'─'.repeat(width - 2)}╮`, ...lines.map((l) => `│${l}│`), `╰${'─'.repeat(width - 2)}╯`]
}
async function show(name: string, ui: any) {
  const lines = sketch((await ui.drawn()) as El, 80).map((l) => l.trimEnd())
  ;(globalThis as any).console?.log(`--- ${name} ---\n${lines.join('\n')}`)
  return lines.join('\n')
}

test('a plan with open questions asks for the answers first', async ($, on) => {
  await seed($, on, ASKING)
  const pane = await $.ui.mount(PANE)
  const text = await show('plan with questions', pane)
  expect(text).toContain('PLAN  Later plan')
  expect(text).toContain('● waiting for your accept')
  expect(text).toContain('3 tasks · 1 open question · check: not ready')
  expect(text).toContain('Answer the open questions first.')
  expect(text).toContain('? Which store? (recommended: sqlite)')
  expect(await pane.find({ key: 'ask:CHG-2' })).toBeDefined()
  expect(await pane.find({ key: 'accept:CHG-2' })).toBeUndefined()

  const band = await $.ui.mount(BAND)
  expect((await band.find({ type: 'Text' }))?.text).toBe(
    'rvow · needs you: 1 plan to accept · 1 question /rvow-status',
  )
})

test('a ready plan shows the checklist and accepts on y', async ($, on) => {
  const calls = await seed($, on, READY)
  const pane = await $.ui.mount(PANE)
  const text = await show('ready plan', pane)
  expect(text).toContain('check: ready')
  expect(text).toContain('Review the plan, then accept it.')
  expect((await pane.find({ key: 'accept:CHG-3' }))?.props).toMatchObject({
    hotkey: 'a',
    variant: 'primary',
  })

  await pane.press({ key: 'accept:CHG-3' })
  await show('ready plan, review mode', pane)
  expect(await pane.find({ type: 'Text', text: 'before you accept, check:' })).toBeDefined()
  expect(await pane.find({ type: 'Text', text: /5\. Is the change small enough/ })).toBeDefined()
  expect(calls.some((c) => c.includes('accept'))).toBe(false)

  await pane.press({ key: 'yes:CHG-3' })
  expect(calls.some((c) => c.slice(1).join(' ') === 'accept CHG-3 --yes')).toBe(true)
  expect(await pane.find({ type: 'Text', text: 'before you accept, check:' })).toBeUndefined()
})

test('an active change draws progress, tasks, needs-you and next; c then y accepts all', async ($, on) => {
  const calls = await seed($, on, ACTIVE)
  const pane = await $.ui.mount(PANE)
  const text = await show('active change', pane)
  const all = await pane.findAll({ type: 'Text' })
  const color = (mark: string) => all.find((t) => t.text === mark)?.props.color
  expect(color('✓')).toBe('green')
  expect(color('✗')).toBe('red')
  expect(color('⊘')).toBe('magenta')
  for (const mark of ['~', '≈', '·']) expect(text).toContain(`${mark} T`)
  expect(text).toContain('1/6 proved')
  expect(text).toContain('needs you')
  expect(text).toContain('weaken spec shrank')
  expect(text).toContain('▸ next T2 failed one')
  expect(text).toContain('$ cargo test')

  await pane.press({ key: 'amend:CHG-1' })
  const listed = await show('active change, amendments listed', pane)
  expect(listed).toContain('digest abc123')
  await pane.press({ key: 'amend-yes:CHG-1' })
  expect(calls.some((c) => c.slice(1).join(' ') === 'confirm --accept --all abc123')).toBe(true)

  const band = await $.ui.mount(BAND)
  expect((await band.find({ type: 'Text' }))?.text).toContain('rvow · needs you: 1 amendment')
})

test('a fresh plan from rvow new asks for tasks', async ($, on) => {
  // `rvow hook claude view` right after `rvow init` and `rvow new "Try rvow"`.
  const fresh = JSON.parse(
    '{"branch":"trunk","changes":[],"plans":[{"id":"CHG-722x8a","title":"Try rvow","tasks":0,"questions":0,"issues":0,"ready":false}],"questions":[],"next":null,"needs_you":{"amendments":0,"questions":0,"blocked":0,"plans":1}}',
  )
  await seed($, on, fresh)
  const text = await show('fresh plan (real rvow output)', await $.ui.mount(PANE))
  expect(text).toContain('Add tasks with verify commands.')
  expect(text).toContain('0 tasks · 0 open questions · check: not ready')
})

test('nothing from rvow draws the empty state and no band', async ($, on) => {
  await seed($, on, null)
  const pane = await $.ui.mount(PANE)
  const text = await show('empty', pane)
  expect(text).toContain('No rvow change on this branch.')
  expect(text).toContain('rvow new "<title>"')
  const band = await $.ui.mount(BAND)
  expect((await band.find({ type: 'Text' }))?.text).toBe('fallthrough')
})

test('rvow_ask asks each question and records the answer', async ($, on) => {
  const calls: string[][] = []
  const round = {
    questions: [
      {
        ref: 'CHG-1:proposal.md:12',
        question: 'Which store?',
        header: 'Which store?',
        options: [
          { label: 'sqlite (Recommended)', description: 'one file' },
          { label: 'json', description: '' },
        ],
      },
    ],
    remaining: 3,
  }
  on('session.cwd', async () => ({ value: '/tmp' }) as any)
  on('process.run', async (_$: any, e: any) => {
    calls.push(e.argv ?? e.command)
    const out = String(calls.at(-1)).includes('clarify') ? JSON.stringify(round) : ''
    return { value: { exitCode: 0, stdout: out, stderr: '' } } as any
  })
  on('tool.call', { tool: 'AskUserQuestion' }, async (_$: any, e: any) => ({
    result: { questions: e.questions, answers: { [e.questions[0].question]: 'json' } },
  }))
  on('ui.status', async () => ({ value: undefined }) as any)
  const out = await $.tool.call({ tool: 'mcp__rvow__rvow_ask' } as any)
  const text = JSON.stringify(out)
  expect(text).toContain('CHG-1:proposal.md:12 Which store? -> json')
  expect(text).toContain('3 questions remain')
  expect(text).toContain('Edit the text each answer changes.')
  expect(
    calls.some((c) => c.includes('answer') && c.includes('json') && c.includes('dialog')),
  ).toBe(true)
})
