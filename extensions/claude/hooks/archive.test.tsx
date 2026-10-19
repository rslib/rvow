import type { On } from 'claude-code'
import type { Engine } from 'claude-code/testing'
import { expect } from 'claude-code/testing'
import { cmd, PLACED, pane, procResult, spawnHook, test } from './testkit'

const PANE = pane()
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

async function seed($: Engine, on: On, state: string): Promise<string[]> {
  const calls: string[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('env.get', async () => ({ value: '/home/me' }))
  on('ui.status', async () => ({ value: undefined }))
  on('ui.open', async () => PLACED)
  on('process.run', async (_$, e) => {
    const rest = (e.argv as string[]).slice(1).join(' ')
    calls.push(rest)
    const stdout = rest === 'hook claude view' ? JSON.stringify(view(state)) : ''
    return { value: procResult(0, stdout) }
  })
  on('process.spawn', async function* (_$, e) {
    calls.push([...e.argv].slice(1).join(' '))
    return yield* spawnHook(() => [0, ''])(_$, e)
  })
  on('tool.call', { tool: 'AskUserQuestion' }, async (_$, e) => {
    const question = e.questions[0]?.question ?? ''
    return { result: { questions: e.questions, answers: { [question]: 'Archive' } } }
  })
  await $.command.run(cmd('rvow-status', ''))
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
  const labels = (await pane.findAll({ type: 'Button' })).map((b) => b.props.label)
  expect(labels).toContain('Details')
  expect(labels).not.toContain('Archive')
})

test('/rvow-archive runs rvow archive only when the user types it', async ($, on) => {
  const calls = await seed($, on, 'proved')
  const r = await $.command.run(cmd('rvow-archive', 'CHG-1'))
  expect(calls).toContain('archive CHG-1')
  const model = await $.command.run(cmd('rvow-archive', 'CHG-1', { kind: 'sdk' }))
  expect(model.text).toContain('only when you type it')
  expect(r.text).toBeDefined()
})
