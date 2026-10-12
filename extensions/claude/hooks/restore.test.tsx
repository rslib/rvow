import type { On } from 'claude-code'
import type { Engine } from 'claude-code/testing'
import { expect, mock } from 'claude-code/testing'
import { cmd, PLACED, pane, procResult, test } from './testkit'

const PLAN = {
  id: 'CHG-1',
  title: 'Plan',
  branch: 'trunk',
  here: true,
  tasks: 1,
  questions: 0,
  issues: 0,
  ready: true,
  path: '/tmp/CHG-1',
}
const VIEW = {
  branch: 'trunk',
  changes: [],
  plans: [PLAN],
  backlog: [PLAN],
  archive: [{ id: 'CHG-9', title: 'Shipped', outcome: 'done', date: '2026-09-30' }],
  questions: [],
  next: null,
  needs_you: { amendments: 0, questions: 0, blocked: 0, plans: 0 },
}
const PANE = pane()

const SAVED = { details: [{ kind: 'plan', id: 'CHG-1', title: 'Plan' }], review: ['CHG-1'] }

async function seed($: Engine, on: On, entries?: Record<string, unknown>) {
  mock.env(on, { HOME: '/home/me' })
  if (entries) mock.store(on, entries)
  else {
    on('store.get', async () => {
      throw new Error('no store')
    })
    on('store.set', async () => {
      throw new Error('no store')
    })
  }
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('ui.open', async () => PLACED)
  on('process.run', async (_$, e) => {
    const argv = [...e.argv]
    const stdout = argv.includes('show') ? 'CHG-1\nstage: backlog' : JSON.stringify(VIEW)
    return { value: procResult(0, stdout) }
  })
  on('ui.render', { component: 'AbovePrompt' }, async ($, e) => {
    const { Text } = $.ui.resolve(e)
    return <Text>fallthrough</Text>
  })
  await $.command.run(cmd('rvow-status', ''))
  return $.ui.mount(PANE)
}

test('the pane reopens on the saved plan detail with its review expanded', async ($, on) => {
  const pane = await seed($, on, { pane: SAVED })
  expect(await pane.find({ key: 'back' })).toBeTruthy()
  expect(await pane.find({ key: 'yes:CHG-1' })).toBeTruthy()
})

test('a saved id that is not in the view is dropped', async ($, on) => {
  const gone = { details: [{ kind: 'plan', id: 'CHG-404', title: 'Gone' }], review: ['CHG-404'] }
  const pane = await seed($, on, { pane: gone })
  expect(await pane.find({ key: 'back' })).toBeFalsy()
  expect((await pane.find({ key: 'tab:backlog' }))?.props.variant).toBe('primary')
})

test('a store that rejects does not break /rvow-status', async ($, on) => {
  const pane = await seed($, on)
  expect((await pane.find({ key: 'tab:backlog' }))?.props.variant).toBe('primary')
})
