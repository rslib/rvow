import type { On } from 'claude-code'
import type { Engine } from 'claude-code/testing'
import { expect, mock } from 'claude-code/testing'
import { cmd, procResult, test } from './testkit'

let clock: { advance: (ms: number) => Promise<void> }

function fake(on: On, exitCode: number, stdout: string) {
  clock = mock.clock(on)
  const prompts: string[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('process.run', async () => ({ value: procResult(exitCode, stdout) }))
  on('prompt.submit', async (_$, e) => {
    prompts.push(e.text)
    return { text: e.text }
  })
  return prompts
}

async function run($: Engine, command: string, args: string) {
  await $.command.run(cmd(command, args))
  await clock.advance(0)
}

test('accept queues the next task for the agent', async ($, on) => {
  const prompts = fake(on, 0, 'accepted CHG-1\nnext: TSK-1 Do it\n')
  await run($, 'rvow-accept', 'CHG-1')
  expect(prompts).toHaveLength(1)
  expect(prompts[0]).toContain('CHG-1')
  expect(prompts[0]).toContain('next: TSK-1 Do it')
})

test('confirm accept and reject name the amendments', async ($, on) => {
  const prompts = fake(on, 0, 'AMD-b: done\n')
  await run($, 'rvow-confirm', 'accept AMD-a')
  await run($, 'rvow-confirm', 'reject --all abc123')
  expect(prompts).toHaveLength(2)
  expect(prompts[0]).toContain('accepted the plan edit AMD-a, AMD-b')
  expect(prompts[1]).toContain('rejected the plan edit AMD-b')
})

test('archive queues the closed change', async ($, on) => {
  const prompts = fake(on, 0, 'archived\n')
  await run($, 'rvow-archive', 'CHG-1')
  expect(prompts).toHaveLength(1)
  expect(prompts[0]).toContain('CHG-1')
})

test('a plain listing queues nothing', async ($, on) => {
  const prompts = fake(on, 0, 'AMD-a pending\n')
  await run($, 'rvow-confirm', '')
  expect(prompts).toEqual([])
})

test('a failed run tells the agent what failed and why', async ($, on) => {
  const prompts = fake(on, 1, 'error: refused\ncode: not-ready\n')
  on('ui.toast', async () => ({ value: undefined }))
  await run($, 'rvow-accept', 'CHG-1')
  await run($, 'rvow-confirm', 'accept AMD-a')
  await run($, 'rvow-archive', 'CHG-1')
  expect(prompts).toHaveLength(3)
  expect(prompts[0]).toContain('I tried `rvow accept CHG-1` and it failed:')
  expect(prompts[1]).toContain('I tried `rvow confirm --accept AMD-a` and it failed:')
  expect(prompts[2]).toContain('I tried `rvow archive CHG-1` and it failed:')
  for (const p of prompts) expect(p).toContain('error: refused\ncode: not-ready')
})

test('a listing that fails is not a decision, so the agent is not told', async ($, on) => {
  const prompts = fake(on, 1, 'error: no project\n')
  on('ui.toast', async () => ({ value: undefined }))
  await run($, 'rvow-confirm', '')
  expect(prompts).toEqual([])
})
