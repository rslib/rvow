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
const run = ($: any, command: string, args = '') =>
  $.command.run({ command, args, origin: { kind: 'composer' } })

/** Document-order index of the Accept-side button and of the plan text. */
async function order($: any, pane: any, key: string) {
  const all = await pane.findAll({})
  return [
    all.findIndex((n: any) => n.props?.key === key),
    all.findIndex((n: any) => n.type === 'Text' && String(n.text).includes('Print the greeting')),
  ]
}

test('an expanded review draws Accept before the plan text', async ($, on) => {
  await seed($, on)
  await run($, 'rvow-review', ID)
  const [button, text] = await order($, await $.ui.mount(PANE), `yes:${ID}`)
  expect(button).toBeGreaterThan(-1)
  expect(text).toBeGreaterThan(button)
})

test('a collapsed review keeps the plan text first', async ($, on) => {
  await seed($, on)
  await run($, 'rvow-status')
  const pane = await $.ui.mount(PANE)
  await pane.press({ key: `details:${ID}` })
  const [button, text] = await order($, pane, `accept:${ID}`)
  expect(text).toBeGreaterThan(-1)
  expect(button).toBeGreaterThan(text)
})
