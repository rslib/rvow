import type { On, SessionCompactInput } from 'claude-code'
import { expect, mock } from 'claude-code/testing'
import { procResult, test } from './testkit'

const BASE = { session_id: 's1', transcript_path: '/t/s1.jsonl', cwd: '/tmp' }
const START = { cwd: '/tmp', surface: null, isInteractive: false }
const BRIEF = 'rvow CHG-1: next TSK-1'
const COMPACT: SessionCompactInput = {
  trigger: 'manual',
  messages: [{ role: 'user', text: 'old', toolUses: [] }],
}

function fake(on: On, compacted: boolean, inputs: Record<string, unknown>[] = []) {
  mock.clock(on)
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('process.run', async (_$, e) => {
    const rest = (e.argv as string[]).slice(1).join(' ')
    if (rest === 'hook claude session-start') inputs.push(JSON.parse(String(e.init?.stdin ?? '{}')))
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

test('after a compaction the next brief request says compact', async ($, on) => {
  const inputs: Record<string, unknown>[] = []
  fake(on, true, inputs)
  await $.session.start(START)
  await prompt($ as never)
  await prompt($ as never)
  expect(inputs.map((i) => i.source)).toEqual(['startup', undefined])
  await $.session.compact(COMPACT)
  expect((await prompt($ as never)).additionalContext).toEqual([BRIEF])
  expect(inputs[2]?.session_id).toBe('s1')
  expect(inputs[2]?.source).toBe('compact')
  await prompt($ as never)
  expect(inputs[3]?.source).toBeUndefined()
})

test('a compaction that was skipped sends no source', async ($, on) => {
  const inputs: Record<string, unknown>[] = []
  fake(on, false, inputs)
  await $.session.start(START)
  await prompt($ as never)
  await $.session.compact(COMPACT)
  await prompt($ as never)
  expect(inputs[1]?.source).toBeUndefined()
})
