import type { On } from 'claude-code'
import { expect, mock } from 'claude-code/testing'
import { procResult, test } from './testkit'

const BASE = { session_id: 's1', transcript_path: '/t/s1.jsonl', cwd: '/tmp' }
const START = { cwd: '/tmp', surface: null, isInteractive: false }
const BRIEF = 'rvow CHG-1: next TSK-1'

function fakeRvow(on: On, inputs: Record<string, unknown>[], answer: () => string) {
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('process.run', async (_$, e) => {
    const rest = (e.argv as string[]).slice(1).join(' ')
    if (rest === 'hook claude session-start') inputs.push(JSON.parse(String(e.init?.stdin ?? '{}')))
    const text = answer()
    const stdout =
      rest === 'hook claude session-start' && text
        ? JSON.stringify({ hookSpecificOutput: { additionalContext: text } })
        : ''
    return { value: procResult(0, stdout) }
  })
  on('session.start', async (_$, e) => ({ cwd: e.cwd }))
  on('command.register', () => ({ value: {} }) as never)
  on('tool.register', () => ({ value: {} }) as never)
  on('classic.UserPromptSubmit', () => ({}))
}

test('the mod sends the session id and the source, and forwards rvow answer as is', async ($, on) => {
  mock.clock(on)
  const inputs: Record<string, unknown>[] = []
  let answer = BRIEF
  fakeRvow(on, inputs, () => answer)
  await $.session.start(START)
  const first = await $.classic.UserPromptSubmit({ ...BASE, prompt: 'hi' })
  expect(first.additionalContext).toEqual([BRIEF])
  expect(inputs[0]?.session_id).toBe('s1')
  expect(inputs[0]?.source).toBe('startup')
  answer = ''
  const same = await $.classic.UserPromptSubmit({ ...BASE, prompt: 'again' })
  expect(same.additionalContext ?? []).toEqual([])
  expect(inputs[1]?.session_id).toBe('s1')
  expect(inputs[1]?.source).toBeUndefined()
  answer = BRIEF
  await $.session.start(START)
  const afterClear = await $.classic.UserPromptSubmit({ ...BASE, prompt: 'hi' })
  expect(afterClear.additionalContext).toEqual([BRIEF])
  expect(inputs[2]?.source).toBe('clear')
  await $.classic.UserPromptSubmit({ ...BASE, prompt: 'more' })
  expect(inputs[3]?.source).toBeUndefined()
})

test('a brief that times out is forgotten by rvow, so the next prompt asks for the full brief', async ($, on) => {
  const clock = mock.clock(on)
  const inputs: Record<string, unknown>[] = []
  let slow = true
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('process.run', async (_$, e) => {
    const rest = (e.argv as string[]).slice(1).join(' ')
    if (rest === 'hook claude session-start') {
      inputs.push(JSON.parse(String(e.init?.stdin ?? '{}')))
      if (slow) await new Promise(() => {})
      return {
        value: procResult(0, JSON.stringify({ hookSpecificOutput: { additionalContext: BRIEF } })),
      }
    }
    return { value: procResult(0, '') }
  })
  on('session.start', async (_$, e) => ({ cwd: e.cwd }))
  on('command.register', () => ({ value: {} }) as never)
  on('tool.register', () => ({ value: {} }) as never)
  on('classic.UserPromptSubmit', () => ({}))
  await $.session.start(START)
  const first = $.classic.UserPromptSubmit({ ...BASE, prompt: 'hi' })
  while (inputs.length === 0) await clock.advance(0)
  await clock.advance(5_000)
  expect((await first).additionalContext ?? []).toEqual([])
  expect(inputs[0]?.source).toBe('startup')
  slow = false
  const second = await $.classic.UserPromptSubmit({ ...BASE, prompt: 'again' })
  expect(inputs[1]?.source).toBe('clear')
  expect(second.additionalContext).toEqual([BRIEF])
})
