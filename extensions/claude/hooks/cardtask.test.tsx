import type { Mounted } from 'claude-code/testing'
import { expect, mock, test } from 'claude-code/testing'
import { cmd, PLACED, pane, procResult } from './testkit'

const VIEW = {
  branch: 'trunk',
  changes: [
    {
      id: 'CHG-1',
      title: 'Change',
      stage: 'active',
      tasks: [{ id: 'TSK-a1', title: 'Print it', state: 'open', reasons: [] }],
      drift: [],
      pending: [],
    },
  ],
  plans: [],
  backlog: [],
  archive: [],
  questions: [],
  next: null,
  needs_you: { amendments: 0, questions: 0, blocked: 0, plans: 0 },
}
const PANE = pane(72)
const texts = async (pane: Mounted<'terminal', 'Pane'>) =>
  (await pane.findAll({ type: 'Text' })).map((t) => t.text).join('\n')

test('a task on the change card opens its detail; back returns to Active', async ($, on) => {
  mock.env(on, { HOME: '/home/me' })
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('ui.open', async () => PLACED)
  on('process.run', async (_$, e) => {
    const argv = [...e.argv]
    const stdout = argv.includes('show') ? 'TSK-a1\nverify: sh test.sh' : JSON.stringify(VIEW)
    return { value: procResult(0, stdout) }
  })
  await $.command.run(cmd('rvow-status', ''))
  const pane = await $.ui.mount(PANE)
  expect(await texts(pane)).toContain('0/1 proved')

  await pane.press({ key: 'task:TSK-a1' })
  expect(await texts(pane)).toContain('verify: sh test.sh')

  await pane.press({ key: 'back' })
  expect(await texts(pane)).toContain('0/1 proved')
  expect((await pane.find({ key: 'tab:active' }))?.props.variant).toBe('primary')
})
