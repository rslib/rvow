import { expect, mock, test } from 'claude-code/testing'

const VIEW = {
  branch: 'trunk',
  changes: [],
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

async function start($: any, on: any, exitCode: number) {
  const clock = mock.clock(on)
  let release: () => void = () => {}
  const gate = new Promise<void>((r) => (release = r))
  let opened = 0
  mock.env(on, { HOME: '/home/me' })
  on('store.get', async () => ({ value: undefined }) as any)
  on('session.cwd', async () => ({ value: '/tmp' }) as any)
  on('ui.status', async () => ({ value: undefined }) as any)
  on('ui.open', async () => {
    opened++
    return { value: undefined } as any
  })
  on('process.run', async () => {
    await gate
    return { value: { exitCode, stdout: exitCode ? '' : JSON.stringify(VIEW), stderr: '' } } as any
  })
  const done = $.command.run({
    command: 'rvow-status',
    args: '',
    origin: { kind: 'composer' },
  } as any)
  while (!opened) await clock.advance(0)
  return { clock, release, done, opened: () => opened }
}

const texts = async (pane: any) => (await pane.findAll({ type: 'Text' })).map((t: any) => t.text)

test('the pane opens before rvow answers and shows loading', async ($, on) => {
  const run = await start($, on, 0)
  expect(run.opened()).toBe(1)
  const pane = await $.ui.mount(PANE)
  expect(await texts(pane)).toContain('loading rvow...')
  run.release()
  await run.done
  expect(await pane.find({ key: 'tab:backlog' })).toBeTruthy()
})

test('a failed view tells the user to run it again', async ($, on) => {
  const run = await start($, on, 1)
  run.release()
  await run.done
  const pane = await $.ui.mount(PANE)
  expect((await texts(pane)).join('\n')).not.toContain('loading rvow...')
  expect(JSON.stringify(await pane.findAll({ type: 'Text' }))).toContain('rvow did not answer')
})
