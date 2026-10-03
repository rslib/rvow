import type { On } from 'claude-code'
import type { Engine, Mounted } from 'claude-code/testing'
import { expect, mock, test } from 'claude-code/testing'
import { band, cmd, PLACED, pane, procResult } from './testkit'

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
const plan = (id: string, title: string, rest: object) => ({
  id,
  title,
  branch: 'feat/x',
  here: true,
  tasks: 2,
  questions: 0,
  issues: 0,
  ready: true,
  path: `/home/me/repo/.rvow/changes/backlog/${id}`,
  ...rest,
})
const LATER = plan('CHG-2', 'Later plan', { tasks: 3, questions: 1, ready: false })
const TRY = plan('CHG-3', 'Try rvow', {})
const ELSEWHERE = plan('CHG-4', 'Port the parser', { branch: 'feat/parser', here: false })
const ASKING = {
  branch: 'feat/x',
  changes: [],
  plans: [LATER],
  backlog: [LATER],
  archive: [],
  questions: [{ change: 'CHG-2', question: 'Which store?', recommended: 'sqlite' }],
  next: null,
  needs_you: { ...NONE, questions: 1, plans: 1 },
}
const READY = {
  ...ASKING,
  plans: [TRY],
  backlog: [TRY],
  questions: [],
  needs_you: { ...NONE, plans: 1 },
}
const ACTIVE = {
  branch: 'feat/x',
  changes: [CHANGE],
  plans: [],
  backlog: [],
  archive: [],
  questions: [],
  next: { change: 'CHG-1', task: 'T2', title: 'failed one', verify: 'cargo test', state: 'failed' },
  needs_you: { ...NONE, amendments: 1, blocked: 1 },
}
const LISTING =
  'AMD-1 for CHG-1\n  weaken: spec shrank\n  reason (from the agent): reword\ndigest abc123'

// Real `rvow show <id> --human` and `rvow hook claude view` output from a scratch repo: `rvow new
// "Try rvow"` with one task and one requirement, then accept, prove and archive.
const SHOWS: Record<string, string> = {
  'CHG-2q8jsb backlog':
    'CHG-2q8jsb\nstage: backlog\nbranch: trunk\ntitle: Try rvow\ntasks:\n  TSK-new1 Print the greeting with the name\ndeltas:\n  ADD REQ-new1 Greet by name',
  'CHG-2q8jsb active':
    'CHG-2q8jsb\nstage: active\nbranch: feat/greet\ntitle: Try rvow\ntasks:\n  TSK-wvdtb8 Print the greeting with the name\ndeltas:\n  ADD REQ-t9ecf6 Greet by name\nledger:\n  prove 1\n  vow 1',
  'CHG-2q8jsb done':
    'CHG-2q8jsb\nstage: done\nbranch: feat/greet\ntitle: Try rvow\ntasks:\n  TSK-wvdtb8 Print the greeting with the name\ndeltas:\n  ADD REQ-t9ecf6 Greet by name\nledger:\n  archive 1\n  prove 3\n  vow 1',
  'TSK-wvdtb8':
    '- TSK-wvdtb8 Print the greeting with the name\n  covers: REQ-t9ecf6\n  verify: sh test.sh\n  files: hello.sh, test.sh\nchange: CHG-2q8jsb\nREQ-t9ecf6 Greet by name',
  'REQ-t9ecf6':
    '## REQ-t9ecf6 Greet by name\n\nWHEN the user runs the greeting script with a name, the greeter SHALL print `hello, <name>` on one line.\n\n### Scenario: A name is given\n- GIVEN the argument `ray`\n- WHEN the script runs\n- THEN it prints `hello, ray`\n\n### Scenario: No name is given\n- WHEN the script runs with no argument\n- THEN it prints `hello, world`\n\nin: active/CHG-2q8jsb-try-rvow/specs/greet.md\ntouched by:\n  CHG-2q8jsb ADD',
}
const TRY_PLAN = plan('CHG-2q8jsb', 'Try rvow', { tasks: 1, branch: 'trunk' })
const TRY_ACTIVE = JSON.parse(
  '{"branch":"feat/greet","changes":[{"id":"CHG-2q8jsb","title":"Try rvow","stage":"active","tasks":[{"id":"TSK-wvdtb8","title":"Print the greeting with the name","state":"failed","reasons":[]}],"drift":[],"pending":[]}],"plans":[],"backlog":[],"archive":[],"questions":[],"next":{"change":"CHG-2q8jsb","task":"TSK-wvdtb8","title":"Print the greeting with the name","verify":"sh test.sh","state":"failed"},"needs_you":{"amendments":0,"questions":0,"blocked":0,"plans":0}}',
)
const TRY_DONE = JSON.parse(
  '{"branch":"feat/greet","changes":[],"plans":[],"backlog":[],"archive":[{"id":"CHG-2q8jsb","title":"Try rvow","outcome":"done","date":"2026-10-02"}],"questions":[],"next":null,"needs_you":{"amendments":0,"questions":0,"blocked":0,"plans":0}}',
)

const PANE = pane()
const BAND = band()

/** Fakes rvow: the view for `hook claude view`, the listing for `confirm`, `shows[id]` for
 * `show <id> --human`; records every argv and each `ui.open`. */
async function seed($: Engine, on: On, view: object | null, shows: Record<string, string> = {}) {
  const calls: string[][] = []
  mock.env(on, { HOME: '/home/me' })
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('process.run', async (_$, e) => {
    const argv = [...e.argv]
    calls.push(argv)
    const rest = argv.slice(1).join(' ')
    const stdout =
      rest === 'hook claude view'
        ? view
          ? JSON.stringify(view)
          : ''
        : rest === 'confirm'
          ? LISTING
          : argv[1] === 'show'
            ? (shows[argv[2] ?? ''] ?? '')
            : ''
    return { value: procResult(0, stdout) }
  })
  on('prompt.submit', async (_$, e) => {
    calls.push([
      '@prompt',
      e.text,
      e.origin.kind === 'plugin' && e.origin.asUser ? 'asUser' : 'plugin',
    ])
    return { text: e.text }
  })
  on('ui.status', async () => ({ value: undefined }))
  on('ui.open', async (_$, e) => {
    calls.push(['@open', JSON.stringify(e)])
    return PLACED
  })
  on('ui.render', { component: 'AbovePrompt' }, async ($, e) => {
    const { Text } = $.ui.resolve(e)
    return <Text>fallthrough</Text>
  })
  await $.command.run(cmd('rvow-status'))
  return calls
}

// A rough text sketch of a drawn tree, for eyeballing the layout: borders,
// paddings, gaps, wrapping rows and space-between rows; no colors. It does not
// clip: a row too wide for its box shows as a line longer than the width.
type ElProps = {
  key?: string
  label?: string
  hotkey?: string
  plain?: boolean
  wrap?: string
  gap?: number
  columnGap?: number
  rowGap?: number
  flexWrap?: string
  flexDirection?: string
  justifyContent?: string
  padding?: number
  paddingX?: number
  paddingLeft?: number
  marginTop?: number
  marginBottom?: number
  borderStyle?: unknown
  [other: string]: unknown
}
type El = { type: string; props?: ElProps; children?: unknown[] }
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
const truncates = (n: El) => n.type === 'Text' && /^truncate/.test(n.props?.wrap ?? '')
/** Word-wraps as Ink does, breaking a word only when it alone is too long. */
const wrap = (t: string, width: number) => {
  const lines: string[] = []
  for (const word of t.split(/(?<= )/)) {
    const last = lines.at(-1)
    if (last !== undefined && (last + word).trimEnd().length <= width)
      lines[lines.length - 1] = last + word
    else lines.push(...(word.match(new RegExp(`.{1,${width}}`, 'g')) ?? ['']))
  }
  return lines.length ? lines.map((l) => l.trimEnd()) : ['']
}
function natural(n: El): string {
  if (n.type === 'Button') {
    const { plain, hotkey, label } = n.props ?? {}
    return plain ? (hotkey ? `${hotkey}: ${label}` : String(label)) : `[ ${label} ]`
  }
  if (n.type === 'Text') return textOf(n)
  return elems(n)
    .map(natural)
    .join(' '.repeat(n.props?.columnGap ?? n.props?.gap ?? 0))
}
function rowLines(n: El, inner: number): string[] {
  const p = n.props ?? {}
  const kids = elems(n)
  const parts = kids.map(natural)
  const gap = p.columnGap ?? p.gap ?? 0
  if (p.flexWrap === 'wrap') {
    const lines: string[] = []
    for (const part of parts) {
      const last = lines.at(-1)
      if (last !== undefined && last.length + gap + part.length <= inner)
        lines[lines.length - 1] = last + ' '.repeat(gap) + part
      else lines.push(part)
    }
    return lines.length ? lines : ['']
  }
  // Ink shrinks a truncating Text to make the row fit.
  let excess = parts.join(' '.repeat(gap)).length - inner
  kids.forEach((k, i) => {
    if (excess > 0 && truncates(k)) {
      const keep = Math.max(0, (parts[i] ?? '').length - excess)
      excess -= (parts[i] ?? '').length - keep
      parts[i] = (parts[i] ?? '').slice(0, keep)
    }
  })
  const used = parts.join('').length
  const sep =
    p.justifyContent === 'space-between' && parts.length === 2
      ? ' '.repeat(Math.max(gap, inner - used))
      : ' '.repeat(gap)
  return [parts.join(sep)]
}
function sketch(n: El, width: number): string[] {
  if (n.type === 'Text') {
    const t = textOf(n)
    if (truncates(n)) return [t.slice(0, width)]
    return wrap(t, width)
  }
  if (n.type !== 'Box') return [natural(n)]
  const p = n.props ?? {}
  const pad = p.paddingX ?? p.padding ?? 0
  const left = pad + (p.paddingLeft ?? 0)
  const inner = width - (p.borderStyle ? 2 : 0) - 2 * pad - (p.paddingLeft ?? 0)
  let lines: string[] = []
  if (p.flexDirection === 'column') {
    elems(n).forEach((k, i) => {
      if (i > 0) lines.push(...Array(p.rowGap ?? p.gap ?? 0).fill(''))
      lines.push(...Array(k.props?.marginTop ?? 0).fill(''))
      lines.push(...sketch(k, inner))
      lines.push(...Array(k.props?.marginBottom ?? 0).fill(''))
    })
  } else {
    lines = rowLines(n, inner)
  }
  lines = lines.map((l) => ' '.repeat(left) + l.padEnd(inner) + ' '.repeat(pad))
  if (!p.borderStyle) return lines
  return [`╭${'─'.repeat(width - 2)}╮`, ...lines.map((l) => `│${l}│`), `╰${'─'.repeat(width - 2)}╯`]
}
const host: {
  process?: { env?: Record<string, string | undefined> }
  console?: { log: (text: string) => void }
} = { process: Reflect.get(globalThis, 'process'), console: Reflect.get(globalThis, 'console') }
async function show(name: string, ui: Mounted<'terminal', 'Pane'>, width = 80) {
  const lines = sketch(await ui.drawn(), width).map((l) => l.trimEnd())
  if (host.process?.env?.RVOW_SHOW === '1')
    host.console?.log(`--- ${name} (${width} columns) ---\n${lines.join('\n')}`)
  const wide = lines.filter((l) => l.length > width)
  expect(wide).toEqual([])
  return lines.join('\n')
}
const mount = ($: Engine, width = 80) =>
  $.ui.mount({ ...PANE, props: { ...PANE.props, bodyColumns: width } })

test('a plan with open questions asks for the answers first', async ($, on) => {
  await seed($, on, ASKING)
  const pane = await $.ui.mount(PANE)
  const text = await show('plan with questions', pane)
  expect(text).toContain('PLAN  Later plan')
  expect(text).toContain('● waiting for your accept')
  expect(text).toContain('3 tasks · 1 open question · check: not ready')
  expect(text).toContain('to accept: answer the open questions first')
  expect(text).toContain('? Which store? (recommended: sqlite)')
  expect(text).toContain('files: ~/repo/.rvow/changes/backlog/CHG-2')
  expect(await pane.find({ key: 'ask:CHG-2' })).toBeDefined()
  expect((await pane.find({ key: 'accept:CHG-2' }))?.props).toMatchObject({
    hotkey: 'a',
    dimColor: true,
  })

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

// `rvow hook claude view` right after `rvow init` and `rvow new "Try rvow"`.
const FRESH = JSON.parse(
  '{"branch":"trunk","changes":[],"plans":[],"backlog":[{"id":"CHG-722x8a","title":"Try rvow","branch":"trunk","here":true,"tasks":0,"questions":0,"issues":0,"ready":false,"path":"/home/me/repo/.rvow/changes/backlog/CHG-722x8a"}],"archive":[],"questions":[],"next":null,"needs_you":{"amendments":0,"questions":0,"blocked":0,"plans":1}}',
)

test('a fresh plan from rvow new shows a dim accept that only says why', async ($, on) => {
  const calls = await seed($, on, FRESH)
  const pane = await $.ui.mount(PANE)
  const text = await show('Backlog tab, fresh plan (real rvow output)', pane)
  expect(text).toContain('0 tasks · 0 open questions · check: not ready')
  expect(text).toContain('files: ~/repo/.rvow/changes/backlog/CHG-722x8a')
  expect(text).toContain('a: Review & accept')
  expect(text).toContain('to accept: add at least one task with a verify command')
  expect((await pane.find({ key: 'accept:CHG-722x8a' }))?.props.dimColor).toBe(true)

  await pane.press({ key: 'accept:CHG-722x8a' })
  expect(calls.some((c) => c.includes('accept'))).toBe(false)
  expect(await pane.find({ type: 'Text', text: 'before you accept, check:' })).toBeUndefined()
  expect(await show('Backlog tab, fresh plan after a', pane)).toContain(
    'Not ready: add at least one task with a verify command.',
  )
})

const ALL = {
  ...ACTIVE,
  plans: READY.plans,
  backlog: READY.backlog,
  archive: [
    { id: 'CHG-9', title: 'Shipped thing', outcome: 'done', date: '2026-09-30' },
    { id: 'CHG-8', title: 'Bad idea', outcome: 'dropped', date: '2026-09-20' },
    { id: 'CHG-7', title: 'Old plan', outcome: 'superseded', date: '2026-09-10' },
  ],
}

test('tabs: Active by default, 1/2/3 switch what renders', async ($, on) => {
  await seed($, on, ALL)
  const pane = await $.ui.mount(PANE)
  const tab = async (k: string) => {
    const b = (await pane.findAll({ type: 'Button' })).find((x) => x.props.hotkey === k)
    await pane.press({ key: String(b?.props.key) })
  }
  const active = await show('Active tab', pane)
  expect(active).toContain('[ Backlog 1 ] [ Active 1 ] [ Archive 3 ]')
  expect(active).toContain('1/6 proved')
  expect(active).not.toContain('Try rvow')
  expect((await pane.find({ key: 'tab:active' }))?.props.variant).toBe('primary')
  expect((await pane.find({ key: 'tab:backlog' }))?.props.variant).toBeUndefined()

  await tab('1')
  const plans = await show('Backlog tab, ready plan', pane)
  expect(plans).toContain('PLAN  Try rvow')
  expect(plans).not.toContain('1/6 proved')
  expect(await pane.find({ key: 'amend:CHG-1' })).toBeUndefined()

  await tab('3')
  const archive = await show('Archive tab', pane)
  expect(archive).toContain('✓ 2026-09-30 CHG-9 Shipped thing')
  expect(archive).toContain('✗ 2026-09-20 CHG-8 Bad idea')
  expect(archive).toContain('↷ 2026-09-10 CHG-7 Old plan')
  const all = await pane.findAll({ type: 'Text' })
  expect(all.find((t) => t.text === '↷')?.props.color).toBe('yellow')
  expect(archive).not.toContain('PLAN')

  await tab('2')
  expect(await show('back to Active', pane)).toContain('1/6 proved')
})

test('the default tab is Backlog with a plan and no active change', async ($, on) => {
  await seed($, on, { ...READY, archive: ALL.archive })
  expect(await show('default Backlog', await $.ui.mount(PANE))).toContain('PLAN  Try rvow')
})

test('the default tab is Archive with neither; empty tabs say so', async ($, on) => {
  await seed($, on, { ...READY, plans: [], backlog: [], archive: ALL.archive, needs_you: NONE })
  const pane = await $.ui.mount(PANE)
  expect(await show('default Archive', pane)).toContain('CHG-9 Shipped thing')
  await pane.press({ key: 'tab:active' })
  expect(await show('empty Active', pane)).toContain('No active change on this branch.')
  await pane.press({ key: 'tab:backlog' })
  const empty = await show('empty Backlog', pane)
  expect(empty).toContain('No plan in the backlog.')
  expect(empty).toContain('rvow new "<title>"')
})

test('Backlog: other-branch plans take t, e queues a refine, f refines the selection', async ($, on) => {
  const calls = await seed($, on, {
    ...READY,
    backlog: [TRY, ELSEWHERE],
    needs_you: { ...NONE, plans: 1 },
  })
  const pane = await $.ui.mount(PANE)
  const before = await show('Backlog, two plans', pane)
  expect(before).toContain('on feat/parser')
  expect(before).toContain('t: Take to this branch')
  expect(before).toContain('to accept: take it to this branch first')
  expect((await pane.find({ key: 'accept:CHG-4' }))?.props).toMatchObject({ dimColor: true })
  expect((await pane.find({ key: 'accept:CHG-4' }))?.props.hotkey).toBeUndefined()
  expect((await pane.find({ key: 'accept:CHG-3' }))?.props.hotkey).toBe('a')

  await pane.press({ key: 'accept:CHG-4' })
  expect(calls.some((c) => c.includes('accept'))).toBe(false)
  await pane.press({ key: 'claim:CHG-4' })
  expect(calls.some((c) => c.slice(1).join(' ') === 'claim CHG-4')).toBe(true)

  await pane.press({ key: 'refine:CHG-3' })
  const one = calls.filter((c) => c[0] === '@prompt')
  expect(one).toHaveLength(1)
  expect(one[0]?.[1]).toContain('Review and refine the rvow plan CHG-3 (Try rvow).')
  expect(one[0]?.[1]).toContain('`rvow check CHG-3`')
  expect(one[0]?.[2]).toBe('asUser')
  expect(await show('Backlog, after e', pane)).toContain('queued for the agent')

  expect(await pane.find({ key: 'refine-picked' })).toBeUndefined()
  await pane.press({ key: 'select:CHG-4' })
  await show('Backlog, one selected', pane)
  await pane.press({ key: 'select:CHG-3' })
  const picked = await show('Backlog, two selected', pane)
  expect(picked).toContain('f: Refine 2 selected')
  expect(picked).toContain('◉  PLAN  Try rvow')
  await pane.press({ key: 'refine-picked' })
  const many = calls.filter((c) => c[0] === '@prompt')
  expect(many).toHaveLength(2)
  expect(many[1]?.[1]).toContain('- CHG-3 (Try rvow)\n- CHG-4 (Port the parser)')
  expect(many[1]?.[1]).toContain('one plan at a time')
  expect(await pane.find({ key: 'refine-picked' })).toBeUndefined()
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
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('process.run', async (_$, e) => {
    calls.push([...e.argv])
    const out = String(calls.at(-1)).includes('clarify') ? JSON.stringify(round) : ''
    return { value: procResult(0, out) }
  })
  on('tool.call', { tool: 'AskUserQuestion' }, async (_$, e) => ({
    result: { questions: e.questions, answers: { [e.questions[0]?.question ?? '']: 'json' } },
  }))
  on('ui.status', async () => ({ value: undefined }))
  const out = await $.tool.call({ tool: 'mcp__rvow__rvow_ask' })
  const text = JSON.stringify(out)
  expect(text).toContain('CHG-1:proposal.md:12 Which store? -> json')
  expect(text).toContain('3 questions remain')
  expect(text).toContain('Edit the text each answer changes.')
  expect(
    calls.some((c) => c.includes('answer') && c.includes('json') && c.includes('dialog')),
  ).toBe(true)
})

const backlogTry = () => ({ ...READY, plans: [TRY_PLAN], backlog: [TRY_PLAN] })
const real = (stage: string) => ({ ...SHOWS, 'CHG-2q8jsb': SHOWS[`CHG-2q8jsb ${stage}`] ?? '' })
const shown = (calls: string[][]) => calls.filter((c) => c[1] === 'show').map((c) => c[2])

test('the pane opens without focus, size or closeOnEscape', async ($, on) => {
  const calls = await seed($, on, READY)
  const open = JSON.parse(calls.find((c) => c[0] === '@open')?.[1] ?? '{}')
  expect(open).toMatchObject({ id: 'rvow', title: 'rvow' })
  for (const k of ['focus', 'columns', 'rows', 'closeOnEscape', 'holdToasts'])
    expect(open[k]).toBeUndefined()
})

for (const width of [44, 72, 100]) {
  test(`every view fits ${width} columns`, async ($, on) => {
    await seed(
      $,
      on,
      { ...ALL, backlog: [TRY, ELSEWHERE], questions: ASKING.questions },
      { 'CHG-3': SHOWS['CHG-2q8jsb backlog'] ?? '' },
    )
    const pane = await mount($, width)
    const first = await show(`Active at ${width}`, pane, width)
    expect(first.split('\n')[1]).toBe('─'.repeat(width))
    if (width === 44) expect(first.split('\n')[0]).toBe('[ Backlog 2 ] [ Active 1 ] [ Archive 3 ]')
    else
      expect(first.split('\n')[0]).toMatch(
        /^\[ Backlog 2 \] \[ Active 1 \] \[ Archive 3 \] +⎇ feat\/x$/,
      )
    for (const key of ['tab:backlog', 'tab:archive']) {
      await pane.press({ key })
      await show(`${key} at ${width}`, pane, width)
    }
    await pane.press({ key: 'tab:backlog' })
    await pane.press({ key: 'accept:CHG-3' })
    await show(`review mode at ${width}`, pane, width)
    await pane.press({ key: 'no:CHG-3' })
    await pane.press({ key: 'details:CHG-3' })
    await show(`plan detail at ${width}`, pane, width)
  })
}

test('d opens the plan detail with its actions; b returns to the Backlog tab', async ($, on) => {
  const calls = await seed($, on, backlogTry(), real('backlog'))
  const pane = await mount($, 72)
  await show('Backlog with the plan', pane, 72)
  expect(shown(calls)).toEqual([])

  await pane.press({ key: 'details:CHG-2q8jsb' })
  expect(shown(calls)).toEqual(['CHG-2q8jsb'])
  const text = await show('plan detail', pane, 72)
  expect(text).toContain('b: Back')
  expect(text).toMatch(/ PLAN {2}Try rvow +CHG-2q8jsb/)
  expect(text).toContain('  TSK-new1 Print the greeting with the name')
  expect(text).toContain('a: Review & accept')
  expect(text).toContain('e: Refine with agent')
  expect(text).not.toContain('d: Details')
  expect(text).not.toContain('files:')
  const exact = async (text: string) =>
    (await pane.findAll({ type: 'Text' })).find((t) => t.text === text)?.props
  expect((await exact('tasks:'))?.bold).toBe(true)
  expect((await exact('TSK-new1'))?.dimColor).toBe(true)
  expect(shown(calls)).toHaveLength(1)

  await pane.press({ key: 'accept:CHG-2q8jsb' })
  expect(await show('plan detail, review mode', pane, 72)).toContain('before you accept, check:')
  await pane.press({ key: 'no:CHG-2q8jsb' })

  await pane.press({ key: 'back' })
  const back = await show('back to Backlog', pane, 72)
  expect(back).toContain('files: ~/repo/.rvow/changes/backlog/CHG-2q8jsb')
  expect((await pane.find({ key: 'tab:backlog' }))?.props.variant).toBe('primary')
})

test('a change detail opens a task detail; b walks back to the Active tab', async ($, on) => {
  const calls = await seed($, on, TRY_ACTIVE, real('active'))
  const pane = await mount($, 72)
  await show('Active, real change', pane, 72)

  await pane.press({ key: 'details:CHG-2q8jsb' })
  const change = await show('change detail', pane, 72)
  expect(change).toMatch(/ CHANGE {2}Try rvow +CHG-2q8jsb/)
  expect(change).toContain('  ✗ TSK-wvdtb8 Print the greeting with the name')
  expect(change).toContain('  prove 1')
  expect(change).toContain('Press a task to read its requirements.')

  await pane.press({ key: 'task:TSK-wvdtb8' })
  expect(shown(calls)).toEqual(['CHG-2q8jsb', 'TSK-wvdtb8', 'REQ-t9ecf6'])
  const task = await show('task detail', pane, 72)
  expect(task).toMatch(/ TASK {2}Print the greeting with the name +TSK-wvdtb8/)
  expect(task).toContain('  verify: sh test.sh')
  expect(task).toContain('REQ-t9ecf6 Greet by name')
  expect(task).toContain('WHEN the user runs the greeting script with a name')
  expect(task).toContain('Scenario: No name is given')
  expect((await pane.find({ type: 'Text', text: '  verify: sh test.sh' }))?.props.dimColor).toBe(
    true,
  )
  expect((await pane.find({ type: 'Text', text: 'Scenario: A name is given' }))?.props.bold).toBe(
    true,
  )

  await pane.press({ key: 'back' })
  expect(await show('back to the change', pane, 72)).toContain('Press a task')
  await pane.press({ key: 'back' })
  const active = await show('back to Active', pane, 72)
  expect(active).toContain('0/1 proved')
  expect((await pane.find({ key: 'tab:active' }))?.props.variant).toBe('primary')
})

test('an archive entry opens its detail; a tab press leaves it', async ($, on) => {
  await seed($, on, TRY_DONE, real('done'))
  const pane = await mount($, 72)
  expect(await show('Archive, real entry', pane, 72)).toContain('✓ 2026-10-02 CHG-2q8jsb Try rvow')
  await pane.press({ key: 'open:CHG-2q8jsb' })
  const text = await show('archive detail', pane, 72)
  expect(text).toMatch(/ ARCHIVED {2}Try rvow +CHG-2q8jsb/)
  expect(text).toContain('stage: done')
  await pane.press({ key: 'tab:archive' })
  expect(await pane.find({ key: 'back' })).toBeUndefined()
})

test('a very long rvow show is cut with a count', async ($, on) => {
  const long = Array.from({ length: 250 }, (_, i) => `line ${i}`).join('\n')
  await seed($, on, TRY_DONE, { 'CHG-2q8jsb': long })
  const pane = await mount($, 72)
  await pane.press({ key: 'open:CHG-2q8jsb' })
  const text = await show('long detail', pane, 72)
  expect(text).toContain('line 199')
  expect(text).not.toContain('line 200')
  expect(text).toContain('… 50 more lines cut; run rvow show CHG-2q8jsb')
})

for (const [name, view] of [
  ['Backlog, fresh plan', FRESH],
  ['Backlog, ready plan', READY],
  ['Active', ACTIVE],
  ['Archive', { ...READY, backlog: [], archive: ALL.archive }],
] as const) {
  test(`${name} fits 72 columns`, async ($, on) => {
    await seed($, on, view)
    await show(name, await mount($, 72), 72)
  })
}
