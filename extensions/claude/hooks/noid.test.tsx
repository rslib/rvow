import type { On, PaneOpenArgs } from 'claude-code'
import type { Engine } from 'claude-code/testing'
import { expect, mock } from 'claude-code/testing'
import { cmd, PLACED, procResult, test } from './testkit'

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
async function seed(on: On, view: object = VIEW) {
  const opens: PaneOpenArgs[] = []
  const argvs: string[][] = []
  mock.env(on, { HOME: '/home/me' })
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('process.run', async (_$, e) => {
    const argv = [...e.argv]
    argvs.push(argv)
    const rest = argv.slice(1).join(' ')
    const stdout =
      rest === 'hook claude view'
        ? JSON.stringify(view)
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
  return { opens, argvs }
}
const run = ($: Engine, command: string) => $.command.run(cmd(command))
const two = {
  ...VIEW,
  backlog: [VIEW.backlog[0], { ...VIEW.backlog[0], id: 'CHG-other1', title: 'Second' }],
}

test('/rvow-review with one ready plan opens the pane', async ($, on) => {
  const { opens } = await seed(on)
  const r = await run($, 'rvow-review')
  expect(r.text).toBeUndefined()
  expect(opens).toHaveLength(1)
})

test('/rvow-review with two ready plans lists both', async ($, on) => {
  const { opens } = await seed(on, two)
  const r = await run($, 'rvow-review')
  expect(r.text).toContain('2 plans wait')
  expect(r.text).toContain(ID)
  expect(r.text).toContain('CHG-other1')
  expect(opens).toHaveLength(0)
})

test('/rvow-review with no ready plan says so', async ($, on) => {
  await seed(on, { ...VIEW, backlog: [] })
  expect((await run($, 'rvow-review')).text).toContain('no plan waits')
})

test('/rvow-accept with no id and one plan does not accept', async ($, on) => {
  const { argvs } = await seed(on)
  const r = await run($, 'rvow-accept')
  expect(r.text).toBe(
    `one plan waits: ${ID} Try rvow; read it with /rvow-review, then run /rvow-accept ${ID}`,
  )
  expect(argvs.some((a) => a.includes('accept'))).toBe(false)
})
