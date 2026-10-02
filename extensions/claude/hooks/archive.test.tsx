import { expect, test } from 'claude-code/testing'

const PANE = {
  plugin: 'rvow',
  surface: 'terminal',
  component: 'Pane',
  requestId: 'rvow',
  props: { title: 'rvow', isFocused: true, bodyColumns: 80, placement: 'inline' } as any,
} as const
const NONE = { amendments: 0, questions: 0, blocked: 0, plans: 0 }
const change = (state: string) => ({
  id: 'CHG-1',
  title: 'Add the thing',
  stage: 'active',
  tasks: [{ id: 'T1', title: 'one', state, reasons: [] }],
  drift: [],
  pending: [],
})
const view = (state: string) => ({
  branch: 'feat/x',
  changes: [change(state)],
  plans: [],
  backlog: [],
  archive: [],
  questions: [],
  next: null,
  needs_you: NONE,
})

async function seed($: any, on: any, state: string): Promise<string[]> {
  const calls: string[] = []
  on('session.cwd', async () => ({ value: '/tmp' }) as any)
  on('env.get', async () => ({ value: '/home/me' }) as any)
  on('ui.status', async () => ({ value: undefined }) as any)
  on('ui.open', async () => ({ value: undefined }) as any)
  on('process.run', async (_$: any, e: any) => {
    const rest = (e.argv as string[]).slice(1).join(' ')
    calls.push(rest)
    const stdout = rest === 'hook claude view' ? JSON.stringify(view(state)) : ''
    return { value: { exitCode: 0, stdout, stderr: '' } } as any
  })
  await $.command.run({ command: 'rvow-status', args: '', origin: { kind: 'composer' } })
  return calls
}

test('x archives a change whose tasks are all proved', async ($, on) => {
  const calls = await seed($, on, 'proved')
  const pane = await $.ui.mount(PANE)
  await pane.press({ key: 'archive:CHG-1' })
  expect(calls).toContain('archive CHG-1')
})

test('no archive action while a task is not proved', async ($, on) => {
  await seed($, on, 'failed')
  const pane = await $.ui.mount(PANE)
  const labels = (await pane.findAll({ type: 'Button' })).map((b: any) => b.props.label)
  expect(labels).toContain('Details')
  expect(labels).not.toContain('Archive')
})

test('/rvow-archive runs rvow archive only when the user types it', async ($, on) => {
  const calls = await seed($, on, 'proved')
  const r: any = await $.command.run({
    command: 'rvow-archive',
    args: 'CHG-1',
    origin: { kind: 'composer' },
  } as any)
  expect(calls).toContain('archive CHG-1')
  const model: any = await $.command.run({
    command: 'rvow-archive',
    args: 'CHG-1',
    origin: { kind: 'model' },
  } as any)
  expect(model.text).toContain('only when you type it')
  expect(r.text).toBeDefined()
})
