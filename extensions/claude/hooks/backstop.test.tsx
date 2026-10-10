import type { On } from 'claude-code'
import { expect, test } from 'claude-code/testing'
import { procResult } from './testkit'

const ASK = JSON.stringify({
  hookSpecificOutput: {
    permissionDecision: 'ask',
    permissionDecisionReason: 'rvow: denied 3 times in a row. Allow this call?',
  },
})

function rvowAsks(on: On, answer: string) {
  const asked: string[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('process.run', async (_$, e) => ({
    value: procResult(0, e.argv[3] === 'pre-tool-use' ? ASK : ''),
  }))
  on('tool.call', { tool: 'AskUserQuestion' }, async (_$, e) => {
    asked.push(e.questions[0]?.question ?? '')
    return {
      result: { questions: e.questions, answers: { [e.questions[0]?.question ?? '']: answer } },
    }
  })
  on('tool.call', { tool: 'Edit' }, async () => ({ result: 'edited' }))
  return asked
}

const edit = { tool: 'Edit', file_path: '/tmp/a.txt', old_string: 'a', new_string: 'b' } as never

test('an ask from the hook puts the question to the user, and Allow lets the call run', async ($, on) => {
  const asked = rvowAsks(on, 'Allow')
  const out = await $.tool.call(edit)
  expect(asked[0]).toContain('Allow this call?')
  expect(JSON.stringify(out)).toContain('edited')
})

test('an ask that the user answers with Deny keeps the call denied', async ($, on) => {
  rvowAsks(on, 'Deny')
  const out = await $.tool.call(edit)
  expect(JSON.stringify(out)).not.toContain('edited')
  expect(JSON.stringify(out)).toContain('Allow this call?')
})
