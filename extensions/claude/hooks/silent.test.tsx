import { expect, test } from 'claude-code/testing'

test('/rvow-status opens the pane and adds no chat line', async ($, on) => {
  let opened = 0
  on('session.cwd', async () => ({ value: '/tmp' }) as any)
  on('ui.status', async () => ({ value: undefined }) as any)
  on('process.run', async () => ({ value: { exitCode: 0, stdout: '', stderr: '' } }) as any)
  on('ui.open', async () => {
    opened++
    return { value: undefined } as any
  })
  const r: any = await $.command.run({
    command: 'rvow-status',
    args: '',
    origin: { kind: 'composer' },
  } as any)
  expect(r.text).toBeUndefined()
  expect(opened).toBe(1)
})
