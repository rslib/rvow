import { expect, mock, test } from 'claude-code/testing'

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
const BAND = {
  plugin: 'rvow',
  surface: 'terminal',
  component: 'AbovePrompt',
  props: { hasSurvey: false, isWorking: false, maxRows: 10, bodyColumns: 80 } as any,
} as const

async function bandText($: any, on: any, v: object) {
  mock.env(on, { HOME: '/home/me' })
  on('ui.open', async () => ({ value: undefined }) as any)
  on('session.cwd', async () => ({ value: '/tmp' }) as any)
  on('ui.status', async () => ({ value: undefined }) as any)
  on(
    'process.run',
    async (_$: any, e: any) =>
      ({
        value: {
          exitCode: 0,
          stdout: (e.argv as string[]).join(' ').endsWith('hook claude view')
            ? JSON.stringify(v)
            : '',
          stderr: '',
        },
      }) as any,
  )
  on('ui.render', { component: 'AbovePrompt' }, async (_$: any, e: any) => {
    const { Text } = _$.ui.resolve(e)
    return <Text>fallthrough</Text>
  })
  await $.command.run({ command: 'rvow-status', args: '', origin: { kind: 'composer' } })
  return (await (await $.ui.mount(BAND)).find({ type: 'Text' }))?.text
}

test('the band names the accept command for a ready plan', async ($, on) => {
  expect(await bandText($, on, view({ plans: 1 }))).toContain('1 plan to accept: /rvow-accept CHG-')
})

test('the band names the confirm command for an amendment', async ($, on) => {
  expect(await bandText($, on, view({ amendments: 1 }))).toContain('1 amendment: /rvow-confirm')
})
