import { expect, mock, test } from 'claude-code/testing'

const BASE = { session_id: 's1', transcript_path: '/t/s1.jsonl', cwd: '/tmp' }

test('a prompt carries the brief only when it changed since the last one sent', async ($, on) => {
  mock.clock(on)
  let brief = 'plan CHG-1 waits for accept'
  on('session.cwd', async () => ({ value: '/tmp' }) as any)
  on('process.run', async () => {
    const stdout = JSON.stringify({ hookSpecificOutput: { additionalContext: brief } })
    return { value: { exitCode: 0, stdout, stderr: '' } } as any
  })
  on('classic.UserPromptSubmit', () => ({}))
  const prompt = async () =>
    (await $.classic.UserPromptSubmit({ ...BASE, prompt: 'go' } as any)).additionalContext ?? []
  expect(await prompt()).toEqual([brief])
  expect(await prompt()).toEqual([])
  brief = 'CHG-1 accepted; next TSK-1'
  expect(await prompt()).toEqual([brief])
  expect(await prompt()).toEqual([])
})
