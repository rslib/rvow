import { expect, test } from 'claude-code/testing'

const VIEW = {
  branch: 'feat/x',
  changes: [
    {
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
    },
  ],
  plans: [{ id: 'CHG-2', title: 'Later plan' }],
  questions: [{ change: 'CHG-2', question: 'Which?', recommended: 'this' }],
  next: { change: 'CHG-1', task: 'T2', title: 'failed one', verify: 'cargo test', state: 'failed' },
  needs_you: { amendments: 1, questions: 2, blocked: 1, plans: 1 },
}

async function seed($: any, on: any, stdout: string) {
  on('session.cwd', async () => ({ value: '/tmp' }) as any)
  on('process.run', async () => ({ value: { exitCode: 0, stdout, stderr: '' } }) as any)
  on('ui.status', async () => ({ value: undefined }) as any)
  on('ui.open', async () => ({ value: undefined }) as any)
  on('ui.render', { component: 'AbovePrompt' }, async ($: any, e: any) => {
    const { Text } = $.ui.resolve(e)
    return <Text>fallthrough</Text>
  })
  await $.command.run({ command: 'rvow', args: '', origin: { kind: 'composer' } })
}

test('pane and band draw the view', async ($, on) => {
  await seed($, on, JSON.stringify(VIEW))
  const pane = await $.ui.mount({
    plugin: 'rvow',
    surface: 'terminal',
    component: 'Pane',
    requestId: 'rvow',
    props: { title: 'rvow', isFocused: false, bodyColumns: 80, placement: 'inline' } as any,
  })
  const lines = (await pane.findAll({ type: 'Text' })).map((t) => t.text)
  for (const mark of ['✓', '✗', '~', '≈', '⊘', '·']) {
    expect(lines.some((l) => l.startsWith(mark))).toBe(true)
  }
  const all = await pane.findAll({ type: 'Text' })
  const colors = async (mark: string) => all.find((t) => t.text === mark)?.props.color
  expect(await colors('✓')).toBe('green')
  expect(await colors('✗')).toBe('red')
  expect(await colors('⊘')).toBe('magenta')
  expect(lines.some((l) => l.includes('$ cargo test'))).toBe(true)

  const band = await $.ui.mount({
    plugin: 'rvow',
    surface: 'terminal',
    component: 'AbovePrompt',
    props: { hasSurvey: false, isWorking: false, maxRows: 10, bodyColumns: 80 } as any,
  })
  const text = (await band.find({ type: 'Text' }))?.text ?? ''
  expect(text).toContain('rvow needs you: 1 amendment · 2 questions · 1 plan to accept')
})

test('nothing from rvow draws the empty pane and no band', async ($, on) => {
  await seed($, on, '')
  const pane = await $.ui.mount({
    plugin: 'rvow',
    surface: 'terminal',
    component: 'Pane',
    requestId: 'rvow',
    props: { title: 'rvow', isFocused: false, bodyColumns: 80, placement: 'inline' } as any,
  })
  expect((await pane.find({ type: 'Text' }))?.text).toBe('no active rvow change on this branch')
  const band = await $.ui.mount({
    plugin: 'rvow',
    surface: 'terminal',
    component: 'AbovePrompt',
    props: { hasSurvey: false, isWorking: false, maxRows: 10, bodyColumns: 80 } as any,
  })
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
