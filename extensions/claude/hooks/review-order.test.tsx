import type { On, PaneOpenArgs } from 'claude-code'
import type { Engine, Mounted } from 'claude-code/testing'
import { expect, mock } from 'claude-code/testing'
import { cmd, PLACED, pane, procResult, test } from './testkit'

const ID = 'CHG-2q8jsb'
const VIEW = {
  branch: 'trunk',
  changes: [],
  plans: [],
  backlog: [
    {
      id: ID,
      title: 'Try rvow',
      branch: 'trunk',
      here: true,
      tasks: 1,
      questions: 0,
      issues: 0,
      ready: true,
      path: `/home/me/repo/.rvow/changes/backlog/${ID}`,
    },
  ],
  archive: [],
  questions: [],
  next: null,
  needs_you: { amendments: 0, questions: 0, blocked: 0, plans: 1 },
}
const PANE = pane()

async function seed(on: On) {
  const opens: PaneOpenArgs[] = []
  mock.env(on, { HOME: '/home/me' })
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('process.run', async (_$, e) => {
    const argv = [...e.argv]
    const rest = argv.slice(1).join(' ')
    const stdout =
      rest === 'hook claude view'
        ? JSON.stringify(VIEW)
        : rest === `show ${ID} --human`
          ? `${ID}\nstage: backlog\ntitle: Try rvow\ntasks:\n  TSK-new1 Print the greeting`
          : ''
    return { value: procResult(0, stdout) }
  })
  on('ui.status', async () => ({ value: undefined }))
  on('ui.open', async (_$, e) => {
    opens.push(e)
    return PLACED
  })
  return opens
}
const run = ($: Engine, command: string, args = '') => $.command.run(cmd(command, args))

/** Document-order index of the Accept-side button and of the plan text. */
async function order(pane: Mounted<'terminal', 'Pane'>, key: string): Promise<[number, number]> {
  const all = await pane.findAll({})
  return [
    all.findIndex((n) => n.props?.key === key),
    all.findIndex((n) => n.type === 'Text' && String(n.text).includes('Print the greeting')),
  ]
}

test('an expanded review draws Accept before the plan text', async ($, on) => {
  await seed(on)
  await run($, 'rvow-review', ID)
  const [button, text] = await order(await $.ui.mount(PANE), `yes:${ID}`)
  expect(button).toBeGreaterThan(-1)
  expect(text).toBeGreaterThan(button)
})

test('a collapsed review keeps the plan text first', async ($, on) => {
  await seed(on)
  await run($, 'rvow-status')
  const pane = await $.ui.mount(PANE)
  await pane.press({ key: `details:${ID}` })
  const [button, text] = await order(pane, `accept:${ID}`)
  expect(text).toBeGreaterThan(-1)
  expect(button).toBeGreaterThan(text)
})
