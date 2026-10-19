import type { Mounted } from 'claude-code/testing'
import { expect, mock } from 'claude-code/testing'
import { cmd, PLACED, pane, procResult, test } from './testkit'

const PLANS = Array.from({ length: 200 }, (_, i) => ({
  id: `CHG-p${i}`,
  title: `Plan ${i} with a title long enough to be cut in a narrow pane`,
  branch: 'trunk',
  here: true,
  tasks: 3,
  questions: 0,
  issues: 0,
  ready: true,
  path: `/tmp/CHG-p${i}`,
}))
const CHANGES = Array.from({ length: 20 }, (_, c) => ({
  id: `CHG-c${c}`,
  title: `Change ${c}`,
  stage: 'active',
  tasks: Array.from({ length: 5 }, (_, t) => ({
    id: `TSK-c${c}t${t}`,
    title: `Task ${t}`,
    state: 'open',
    reasons: [],
  })),
  drift: [],
  pending: [],
}))
const VIEW = {
  branch: 'trunk',
  changes: CHANGES,
  plans: [],
  backlog: PLANS,
  archive: [],
  questions: [],
  next: null,
  needs_you: { amendments: 0, questions: 0, blocked: 0, plans: 0 },
}

// The budget for one tab switch and render, with room for a loaded CI machine.
const SWITCH_MS = 150

test('the pane switches tabs fast with 200 plans and 20 changes', async ($, on) => {
  mock.env(on, { HOME: '/home/me' })
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('ui.open', async () => PLACED)
  on('process.run', async () => ({ value: procResult(0, JSON.stringify(VIEW)) }))
  await $.command.run(cmd('rvow-status', ''))
  const view: Mounted<'terminal', 'Pane'> = await $.ui.mount(pane(72))
  await view.press({ key: 'tab:backlog' })
  const texts = (await view.findAll({ type: 'Text' })).map((t) => t.text).join('\n')
  expect(texts).toContain('Plan 1')
  // The fastest of 5 batches: a load spike slows one batch, a slow render slows them all.
  let fastest = Number.POSITIVE_INFINITY
  for (let batch = 0; batch < 5; batch++) {
    const start = performance.now()
    for (let i = 0; i < 4; i++) {
      await view.press({ key: i % 2 ? 'tab:backlog' : 'tab:active' })
      await view.findAll({ type: 'Text' })
    }
    fastest = Math.min(fastest, (performance.now() - start) / 4)
  }
  expect(fastest).toBeLessThan(SWITCH_MS)
})
