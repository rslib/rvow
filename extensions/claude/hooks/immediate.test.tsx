import type { On } from 'claude-code'
import type { Engine, Mounted } from 'claude-code/testing'
import { expect, mock, test } from 'claude-code/testing'
import { cmd, PLACED, pane, procResult } from './testkit'

const VIEW = {
  branch: 'trunk',
  changes: [],
  plans: [],
  backlog: [],
  archive: [],
  questions: [],
  next: null,
  needs_you: { amendments: 0, questions: 0, blocked: 0, plans: 0 },
}
const PANE = pane()

async function start($: Engine, on: On, exitCode: number) {
  const clock = mock.clock(on)
  let release: () => void = () => {}
  const gate = new Promise<void>((r) => (release = r))
  let opened = 0
  mock.env(on, { HOME: '/home/me' })
  on('store.get', async () => ({ value: undefined }))
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('ui.open', async () => {
    opened++
    return PLACED
  })
  on('process.run', async () => {
    await gate
    return { value: procResult(exitCode, exitCode ? '' : JSON.stringify(VIEW)) }
  })
  const done = $.command.run(cmd('rvow-status', ''))
  while (!opened) await clock.advance(0)
  return { clock, release, done, opened: () => opened }
}

const texts = async (pane: Mounted<'terminal', 'Pane'>) =>
  (await pane.findAll({ type: 'Text' })).map((t) => t.text)

test('the pane opens before rvow answers and shows loading', async ($, on) => {
  const run = await start($, on, 0)
  expect(run.opened()).toBe(1)
  const pane = await $.ui.mount(PANE)
  expect(await texts(pane)).toContain('loading rvow...')
  run.release()
  await run.done
  expect(await pane.find({ key: 'tab:backlog' })).toBeTruthy()
})

test('a failed view tells the user to run it again', async ($, on) => {
  const run = await start($, on, 1)
  run.release()
  await run.done
  const pane = await $.ui.mount(PANE)
  expect((await texts(pane)).join('\n')).not.toContain('loading rvow...')
  expect(JSON.stringify(await pane.findAll({ type: 'Text' }))).toContain('rvow did not answer')
})
