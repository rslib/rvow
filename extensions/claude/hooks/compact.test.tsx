import type { On, SessionCompactInput } from 'claude-code'
import { expect, mock, test } from 'claude-code/testing'
import { procResult } from './testkit'

const BASE = { session_id: 's1', transcript_path: '/t/s1.jsonl', cwd: '/tmp' }
const START = { cwd: '/tmp', surface: null, isInteractive: false }
const BRIEF = 'rvow CHG-1: next TSK-1'
const COMPACT: SessionCompactInput = {
  trigger: 'manual',
  messages: [{ role: 'user', text: 'old', toolUses: [] }],
}

function fake(on: On, compacted: boolean) {
  mock.clock(on)
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
  on('session.compact', async () =>
    compacted
      ? { messages: [{ role: 'user', text: 'summary', toolUses: [] }] }
      : { skip: 'nothing to do' },
  )
}

const prompt = ($: {
  classic: { UserPromptSubmit: (e: never) => Promise<{ additionalContext?: string[] }> }
}) => $.classic.UserPromptSubmit({ ...BASE, prompt: 'hi' } as never)

test('after a compaction the brief is sent again', async ($, on) => {
  fake(on, true)
  await $.session.start(START)
  expect((await prompt($ as never)).additionalContext).toEqual([BRIEF])
  expect((await prompt($ as never)).additionalContext ?? []).toEqual([])
  await $.session.compact(COMPACT)
  expect((await prompt($ as never)).additionalContext).toEqual([BRIEF])
})

test('a compaction that was skipped leaves the brief alone', async ($, on) => {
  fake(on, false)
  await $.session.start(START)
  await prompt($ as never)
  await $.session.compact(COMPACT)
  expect((await prompt($ as never)).additionalContext ?? []).toEqual([])
})
