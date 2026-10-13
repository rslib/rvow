import type { On } from 'claude-code'
import type { Engine } from 'claude-code/testing'
import { expect, mock } from 'claude-code/testing'
import { cmd, procResult, test } from './testkit'

let clock: { advance: (ms: number) => Promise<void> }

function fake(on: On) {
  clock = mock.clock(on)
  const ran: string[] = []
  const prompts: string[] = []
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('process.run', async (_$, e) => {
    ran.push([...e.argv].slice(1).join(' '))
    return { value: procResult(0, 'tier: auto\n') }
  })
  on('prompt.submit', async (_$, e) => {
    prompts.push(e.text)
    return { text: e.text }
  })
  return { ran, prompts }
}

async function run($: Engine, args: string) {
  await $.command.run(cmd('rvow-yolo', args))
  await clock.advance(0)
}

test('/rvow-yolo switches a tier on and tells the agent', async ($, on) => {
  const { ran, prompts } = fake(on)
  await run($, 'auto 30m')
  expect(ran.filter((l) => l.startsWith('yolo'))).toEqual(['yolo auto --for 30m'])
  expect(prompts[0]).toContain('I switched on yolo auto: also lets the agent run rvow accept')
  await run($, 'off')
  expect(prompts[1]).toBe('I switched yolo off. The usual rules apply again.')
})

test('/rvow-yolo with nothing shows the state and says nothing to the agent', async ($, on) => {
  const { ran, prompts } = fake(on)
  await run($, '')
  expect(ran.filter((l) => l.startsWith('yolo'))).toEqual(['yolo'])
  expect(prompts).toEqual([])
})

test('/rvow-yolo refuses a tier or length it cannot read, and runs nothing', async ($, on) => {
  const { ran, prompts } = fake(on)
  await run($, 'root')
  await run($, 'off 2h')
  await run($, 'auto soon')
  expect(ran.filter((l) => l.startsWith('yolo'))).toEqual([])
  expect(prompts).toEqual([])
})
