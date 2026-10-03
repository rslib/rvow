import type { On } from 'claude-code'
import type { Engine } from 'claude-code/testing'
import { expect, mock, test } from 'claude-code/testing'
import { band, cmd, PLACED, procResult } from './testkit'

const NONE = { amendments: 0, questions: 0, blocked: 0, plans: 0 }
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
const view = (needs: object) => ({
  branch: 'trunk',
  changes: [],
  plans: [PLAN],
  backlog: [PLAN],
  archive: [],
  questions: [],
  next: null,
  needs_you: { ...NONE, ...needs },
})
const BAND = band()

async function bandText($: Engine, on: On, v: object) {
  mock.env(on, { HOME: '/home/me' })
  on('ui.open', async () => PLACED)
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('process.run', async (_$, e) => ({
    value: procResult(0, e.argv.join(' ').endsWith('hook claude view') ? JSON.stringify(v) : ''),
  }))
  on('ui.render', { component: 'AbovePrompt' }, async (_$, e) => {
    const { Text } = _$.ui.resolve(e)
    return <Text>fallthrough</Text>
  })
  await $.command.run(cmd('rvow-status', ''))
  return (await (await $.ui.mount(BAND)).find({ type: 'Text' }))?.text
}

test('the band names the accept command for a ready plan', async ($, on) => {
  expect(await bandText($, on, view({ plans: 1 }))).toContain('1 plan to accept: /rvow-accept CHG-')
})

test('the band names the confirm command for an amendment', async ($, on) => {
  expect(await bandText($, on, view({ amendments: 1 }))).toContain('1 amendment: /rvow-confirm')
})
