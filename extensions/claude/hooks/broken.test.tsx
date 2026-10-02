import { expect, test } from 'claude-code/testing'

const EDIT = { tool: 'Edit', file_path: '/tmp/x', old_string: 'a', new_string: 'b' } as const

test('a broken rvow shows one toast per session and still allows', async ($, on) => {
  const toasts: string[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('ui.toast', async (_$, e) => {
    toasts.push(e.text)
    return { value: undefined }
  })
  on('process.run', async () => {
    throw new Error('spawn rvow ENOENT')
  })
  on('tool.call', { tool: 'Edit' }, async () => ({ result: 'ok' }))
  await $.tool.call(EDIT)
  await $.tool.call(EDIT)
  expect(toasts.length).toBe(1)
  expect(toasts[0]).toContain('rvow could not run')
})
