import { expect, mock, test } from 'claude-code/testing'

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
const PANE = {
  plugin: 'rvow',
  surface: 'terminal',
  component: 'Pane',
  requestId: 'rvow',
  props: { title: 'rvow', isFocused: true, bodyColumns: 80, placement: 'inline' } as any,
} as const

async function seed($: any, on: any) {
  const opens: any[] = []
  mock.env(on, { HOME: '/home/me' })
  on('session.cwd', async () => ({ value: '/tmp' }) as any)
  on('process.run', async (_$: any, e: any) => {
    const argv: string[] = e.argv ?? e.command
    const rest = argv.slice(1).join(' ')
    const stdout =
      rest === 'hook claude view'
        ? JSON.stringify(VIEW)
        : rest === `show ${ID} --human`
          ? `${ID}\nstage: backlog\ntitle: Try rvow\ntasks:\n  TSK-new1 Print the greeting`
          : ''
    return { value: { exitCode: 0, stdout, stderr: '' } } as any
  })
  on('ui.status', async () => ({ value: undefined }) as any)
  on('ui.open', async (_$: any, e: any) => {
    opens.push(e)
    return { value: undefined } as any
  })
  return opens
}
const run = ($: any, args: string) =>
  $.command.run({ command: 'rvow-review', args, origin: { kind: 'composer' } })

test('/rvow-review opens the pane on the plan with its review expanded', async ($, on) => {
  const opens = await seed($, on)
  const r = await run($, ID.slice(0, 7))
  expect(r.text).toBeUndefined()
  expect(opens).toHaveLength(1)
  expect(opens[0].focus).toBe(true)
  expect(opens[0].closeOnEscape).toBe(true)
  const pane = await $.ui.mount(PANE)
  const texts = (await pane.findAll({ type: 'Text' })).map((t: any) => t.text)
  expect(texts).toContain('before you accept, check:')
  expect(await pane.find({ key: `yes:${ID}` })).toBeTruthy()
})

test('/rvow-review names an unknown id', async ($, on) => {
  const opens = await seed($, on)
  expect((await run($, 'CHG-nope')).text).toContain('not a backlog plan')
  expect(opens).toHaveLength(0)
})
