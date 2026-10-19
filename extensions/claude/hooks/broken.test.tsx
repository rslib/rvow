import { expect } from 'claude-code/testing'
import { procResult, test } from './testkit'

const EDIT = { tool: 'Edit', file_path: '/tmp/x', old_string: 'a', new_string: 'b' } as const
const READ = { tool: 'Read', file_path: '/tmp/x' } as const

test('a broken rvow shows one toast per session, denies writes and allows reads', async ($, on) => {
  const toasts: string[] = []
  const status: (string | undefined)[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async (_$, e) => {
    status.push(e.text)
    return { value: undefined }
  })
  on('ui.toast', async (_$, e) => {
    toasts.push(e.text)
    return { value: undefined }
  })
  on('process.run', async () => {
    throw new Error('spawn rvow ENOENT')
  })
  let read = 0
  on('tool.call', { tool: 'Edit' }, async () => ({ result: 'ok' }))
  on('tool.call', { tool: 'Read' }, async () => {
    read++
    return { result: 'ok' }
  })
  const first = await $.tool.call(EDIT)
  await $.tool.call(EDIT)
  expect(toasts.length).toBe(1)
  expect(toasts[0]).toContain('rvow could not run')
  expect(first.deny).toContain('the hook failed')
  expect(first.deny).toContain('rvow install')
  expect(status).toContain('rvow: the hook fails, so tools that write are denied')
  const r = await $.tool.call(READ)
  expect(r.deny).toBeUndefined()
  expect(read).toBe(1)
})

test('a hook that exits non-zero, prints bad JSON or times out denies a write', async ($, on) => {
  const modes = ['exit', 'json', 'timeout']
  let mode = ''
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('ui.toast', async () => ({ value: undefined }))
  on('process.run', async () => {
    if (mode === 'exit') return { value: procResult(101, '', 'panicked') }
    if (mode === 'json') return { value: procResult(0, 'not json') }
    throw new Error('the process timed out after 10000 ms')
  })
  on('tool.call', { tool: 'Edit' }, async () => ({ result: 'ok' }))
  for (const m of modes) {
    mode = m
    const r = await $.tool.call(EDIT)
    expect(r.deny).toContain('the hook failed')
  }
})

test('a hook that works again clears the warning in the status line', async ($, on) => {
  let broken = true
  const status: (string | undefined)[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async (_$, e) => {
    status.push(e.text)
    return { value: undefined }
  })
  on('ui.toast', async () => ({ value: undefined }))
  on('process.run', async () => {
    if (broken) throw new Error('spawn rvow ENOENT')
    return { value: procResult(0, '') }
  })
  on('tool.call', { tool: 'Edit' }, async () => ({ result: 'ok' }))
  await $.tool.call(EDIT)
  broken = false
  const r = await $.tool.call(EDIT)
  expect(r.deny).toBeUndefined()
  expect(status.at(-1)).not.toBe('rvow: the hook fails, so tools that write are denied')
})
