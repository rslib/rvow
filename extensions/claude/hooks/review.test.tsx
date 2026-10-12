import type { On, PaneOpenArgs } from 'claude-code'
import type { Engine } from 'claude-code/testing'
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
const run = ($: Engine, args: string) => $.command.run(cmd('rvow-review', args))

test('/rvow-review opens the pane on the plan with its review expanded', async ($, on) => {
  const opens = await seed(on)
  const r = await run($, ID.slice(0, 7))
  expect(r.text).toBeUndefined()
  expect(opens).toHaveLength(1)
  expect(opens[0]?.focus).toBe(true)
  expect(opens[0]?.closeOnEscape).toBe(true)
  const pane = await $.ui.mount(PANE)
  const texts = (await pane.findAll({ type: 'Text' })).map((t) => t.text)
  expect(texts).toContain('before you accept, check:')
  expect(await pane.find({ key: `yes:${ID}` })).toBeTruthy()
})

test('/rvow-review names an unknown id', async ($, on) => {
  const opens = await seed(on)
  expect((await run($, 'CHG-nope')).text).toContain('not a backlog plan')
  expect(opens).toHaveLength(0)
})
