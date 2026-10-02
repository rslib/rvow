import { expect, mock, test } from 'claude-code/testing'

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
const VIEW = {
  branch: 'trunk',
  changes: [],
  plans: [PLAN],
  backlog: [PLAN],
  archive: [{ id: 'CHG-9', title: 'Shipped', outcome: 'done', date: '2026-09-30' }],
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

test('the pane reopens on the tab chosen last time', async ($, on) => {
  mock.env(on, { HOME: '/home/me' })
  mock.store(on)
  on('session.cwd', async () => ({ value: '/tmp' }) as any)
  on('ui.status', async () => ({ value: undefined }) as any)
  on('ui.open', async () => ({ value: undefined }) as any)
  on('process.run', async () => {
    return { value: { exitCode: 0, stdout: JSON.stringify(VIEW), stderr: '' } } as any
  })
  on('ui.render', { component: 'AbovePrompt' }, async ($: any, e: any) => {
    const { Text } = $.ui.resolve(e)
    return <Text>fallthrough</Text>
  })
  const status = () =>
    $.command.run({ command: 'rvow-status', args: '', origin: { kind: 'composer' } } as any)
  await status()
  const pane = await $.ui.mount(PANE)
  expect((await pane.find({ key: 'tab:backlog' }))?.props.variant).toBe('primary')

  await pane.press({ key: 'tab:archive' })
  await status()
  await pane.unmount()
  const again = await $.ui.mount(PANE)
  expect((await again.find({ key: 'tab:archive' }))?.props.variant).toBe('primary')
  expect((await again.find({ key: 'tab:backlog' }))?.props.variant).toBeUndefined()
})
