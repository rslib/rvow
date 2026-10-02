import { expect, test } from 'claude-code/testing'

test('a broken rvow shows one toast per session and still allows', async ($, on) => {
  const toasts: string[] = []
  on('session.cwd', async () => ({ value: '/tmp' }) as any)
  on('ui.status', async () => ({ value: undefined }) as any)
  on('ui.toast', async (_$: any, e: any) => {
    toasts.push(e.text)
    return { value: undefined } as any
  })
  on('process.run', async () => {
    throw new Error('spawn rvow ENOENT')
  })
  on('tool.call', { tool: 'Edit' }, async () => ({ result: 'ok' }) as any)
  await $.tool.call({ tool: 'Edit' } as any)
  await $.tool.call({ tool: 'Edit' } as any)
  expect(toasts.length).toBe(1)
  expect(toasts[0]).toContain('rvow could not run')
})
