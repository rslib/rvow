import type { On } from 'claude-code'
import { expect, mock } from 'claude-code/testing'
import { procResult, test } from './testkit'

const BASE = { session_id: 's1', transcript_path: '/t/s1.jsonl', cwd: '/tmp' }
const BRIEF = 'rvow CHG-1: next TSK-1'

function fakeRvow(on: On): string[] {
  const calls: string[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('process.run', async (_$, e) => {
    const rest = (e.argv as string[]).slice(1).join(' ')
    calls.push(rest)
    const stdout =
      rest === 'hook claude session-start'
        ? JSON.stringify({ hookSpecificOutput: { additionalContext: BRIEF } })
        : ''
    return { value: procResult(0, stdout) }
  })
  on('classic.SessionStart', () => ({}))
  on('classic.UserPromptSubmit', () => ({}))
  return calls
}

test('session start runs no rvow; the first prompt carries the brief', async ($, on) => {
  mock.clock(on)
  const calls = fakeRvow(on)
  const started = await $.classic.SessionStart({ ...BASE, source: 'startup' })
  expect(started.additionalContext ?? []).toEqual([])
  expect(calls).toEqual([])
  const first = await $.classic.UserPromptSubmit({ ...BASE, prompt: 'hi' })
  expect(first.additionalContext).toEqual([BRIEF])
})
