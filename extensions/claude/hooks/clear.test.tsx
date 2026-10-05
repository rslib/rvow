import type { On } from 'claude-code'
import { expect, mock, test } from 'claude-code/testing'
import { procResult } from './testkit'

const BASE = { session_id: 's1', transcript_path: '/t/s1.jsonl', cwd: '/tmp' }
const START = { cwd: '/tmp', surface: null, isInteractive: false }
const BRIEF = 'rvow CHG-1: next TSK-1'

function fakeRvow(on: On) {
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('process.run', async (_$, e) => {
    const rest = (e.argv as string[]).slice(1).join(' ')
    const stdout =
      rest === 'hook claude session-start'
        ? JSON.stringify({ hookSpecificOutput: { additionalContext: BRIEF } })
        : ''
    return { value: procResult(0, stdout) }
  })
  on('session.start', async (_$, e) => ({ cwd: e.cwd }))
  on('command.register', () => ({ value: {} }) as never)
  on('tool.register', () => ({ value: {} }) as never)
  on('classic.UserPromptSubmit', () => ({}))
}

test('after /clear the same brief is sent again', async ($, on) => {
  mock.clock(on)
  fakeRvow(on)
  await $.session.start(START)
  const first = await $.classic.UserPromptSubmit({ ...BASE, prompt: 'hi' })
  expect(first.additionalContext).toEqual([BRIEF])
  const same = await $.classic.UserPromptSubmit({ ...BASE, prompt: 'again' })
  expect(same.additionalContext ?? []).toEqual([])
  await $.session.start(START)
  const afterClear = await $.classic.UserPromptSubmit({ ...BASE, prompt: 'hi' })
  expect(afterClear.additionalContext).toEqual([BRIEF])
})
