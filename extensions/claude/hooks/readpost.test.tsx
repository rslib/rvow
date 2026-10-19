import { expect } from 'claude-code/testing'
import { procResult, test } from './testkit'

test('a read runs no post-tool-use hook, and an edit does', async ($, on) => {
  const events: string[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('process.run', async (_$, e) => {
    events.push(e.argv[3] ?? '')
    return { value: procResult(0, '') }
  })
  on('tool.call', { tool: 'Read' }, async () => ({ result: 'ok' }))
  on('tool.call', { tool: 'Edit' }, async () => ({ result: 'ok' }))
  await $.tool.call({ tool: 'Read', file_path: '/tmp/x' })
  await $.tool.call({ tool: 'Read', file_path: '/tmp/y' })
  expect(events).toEqual(['pre-tool-use', 'pre-tool-use'])
  await $.tool.call({ tool: 'Edit', file_path: '/tmp/x', old_string: 'a', new_string: 'b' })
  expect(events.slice(2)).toEqual(['pre-tool-use', 'post-tool-use'])
})
