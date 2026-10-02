import { expect, mock, test } from 'claude-code/testing'

const BASE = { session_id: 's1', transcript_path: '/t/s1.jsonl', cwd: '/tmp' }
const BRIEF = 'rvow CHG-1: next TSK-1'

function fakeRvow($: any, on: any): string[] {
  const calls: string[] = []
  on('session.cwd', async () => ({ value: '/tmp' }) as any)
  on('process.run', async (_$: any, e: any) => {
    const rest = (e.argv as string[]).slice(1).join(' ')
    calls.push(rest)
    const stdout =
      rest === 'hook claude session-start'
        ? JSON.stringify({ hookSpecificOutput: { additionalContext: BRIEF } })
        : ''
    return { value: { exitCode: 0, stdout, stderr: '' } } as any
  })
  on('classic.SessionStart', () => ({}))
  on('classic.UserPromptSubmit', () => ({}))
  return calls
}

test('session start runs no rvow; the first prompt carries the brief', async ($, on) => {
  mock.clock(on)
  const calls = fakeRvow($, on)
  const started = await $.classic.SessionStart({ ...BASE, source: 'startup' } as any)
  expect(started.additionalContext ?? []).toEqual([])
  expect(calls).toEqual([])
  const first = await $.classic.UserPromptSubmit({ ...BASE, prompt: 'hi' } as any)
  expect(first.additionalContext).toEqual([BRIEF])
})
