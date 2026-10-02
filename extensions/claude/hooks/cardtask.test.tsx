import { expect, mock, test } from 'claude-code/testing'

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
const PANE = {
  plugin: 'rvow',
  surface: 'terminal',
  component: 'Pane',
  requestId: 'rvow',
  props: { title: 'rvow', isFocused: true, bodyColumns: 72, placement: 'inline' } as any,
} as const
const texts = async (pane: any) =>
  (await pane.findAll({ type: 'Text' })).map((t: any) => t.text).join('\n')

test('a task on the change card opens its detail; back returns to Active', async ($, on) => {
  mock.env(on, { HOME: '/home/me' })
  on('session.cwd', async () => ({ value: '/tmp' }) as any)
  on('ui.status', async () => ({ value: undefined }) as any)
  on('ui.open', async () => ({ value: undefined }) as any)
  on('process.run', async (_$: any, e: any) => {
    const argv: string[] = e.argv ?? e.command
    const stdout = argv.includes('show') ? 'TSK-a1\nverify: sh test.sh' : JSON.stringify(VIEW)
    return { value: { exitCode: 0, stdout, stderr: '' } } as any
  })
  await $.command.run({ command: 'rvow-status', args: '', origin: { kind: 'composer' } } as any)
  const pane = await $.ui.mount(PANE)
  expect(await texts(pane)).toContain('0/1 proved')

  await pane.press({ key: 'task:TSK-a1' })
  expect(await texts(pane)).toContain('verify: sh test.sh')

  await pane.press({ key: 'back' })
  expect(await texts(pane)).toContain('0/1 proved')
  expect((await pane.find({ key: 'tab:active' }))?.props.variant).toBe('primary')
})
