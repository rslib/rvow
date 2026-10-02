import { expect, test } from 'claude-code/testing'
import { cmd, PLACED, procResult } from './testkit'

test('/rvow-status opens the pane and adds no chat line', async ($, on) => {
  let opened = 0
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('process.run', async () => ({ value: procResult(0, '') }))
  on('ui.open', async () => {
    opened++
    return PLACED
  })
  const r = await $.command.run(cmd('rvow-status', ''))
  expect(r.text).toBeUndefined()
  expect(opened).toBe(1)
})
