import { expect, mock, test } from 'claude-code/testing'

const TASK = { id: 'TSK-a1', title: 'Print it', state: 'open' }
const VIEW = {
  branch: 'trunk',
  changes: [
    {
      id: 'CHG-1',
      title: 'Change',
      branch: 'trunk',
      here: true,
      tasks: [TASK],
      questions: 0,
      issues: 0,
      path: '/tmp/CHG-1',
    },
  ],
  plans: [],
  backlog: [],
  archive: [],
  questions: [],
  next: null,
  needs_you: { amendments: 0, questions: 0, blocked: 0, plans: 0 },
}
const PANE = {
  plugin: 'rvow',
  surface: 'terminal',
  component: 'Pane',
  requestId: 'rvow',
  props: { title: 'rvow', isFocused: true, bodyColumns: 80, placement: 'inline' } as any,
} as const

async function seed($: any, on: any, show: { out: string; exitCode: number }) {
  mock.env(on, { HOME: '/home/me' })
  mock.store(on, {
    pane: { details: [{ kind: 'task', id: TASK.id, title: TASK.title }], review: [] },
  })
  on('session.cwd', async () => ({ value: '/tmp' }) as any)
  on('ui.status', async () => ({ value: undefined }) as any)
  on('ui.open', async () => ({ value: undefined }) as any)
  on('process.run', async (_$: any, e: any) => {
    const argv: string[] = e.argv ?? e.command
    const isShow = argv.includes('show')
    const stdout = isShow ? show.out : JSON.stringify(VIEW)
    return { value: { exitCode: isShow ? show.exitCode : 0, stdout, stderr: '' } } as any
  })
  on('tool.call', { tool: 'Bash' }, async () => ({ result: 'ok' }) as any)
  await $.command.run({ command: 'rvow-status', args: '', origin: { kind: 'composer' } } as any)
  return $.ui.mount(PANE)
}
const texts = async (pane: any) =>
  (await pane.findAll({ type: 'Text' })).map((t: any) => t.text).join('\n')
const prove = ($: any) => $.tool.call({ tool: 'Bash', command: 'rvow prove TSK-a1' } as any)

test('an open task page reloads after an rvow command', async ($, on) => {
  const show = { out: 'TSK-a1\nstate: open', exitCode: 0 }
  const pane = await seed($, on, show)
  expect(await texts(pane)).toContain('state: open')
  show.out = 'TSK-a1\nstate: open\nproof runs: 1 in total'
  await prove($)
  expect(await texts(pane)).toContain('proof runs: 1 in total')
})

test('a failing rvow show keeps the good text', async ($, on) => {
  const show = { out: 'TSK-a1\nstate: open', exitCode: 0 }
  const pane = await seed($, on, show)
  show.out = 'error: boom'
  show.exitCode = 1
  await prove($)
  const all = await texts(pane)
  expect(all).toContain('state: open')
  expect(all).not.toContain('boom')
})
