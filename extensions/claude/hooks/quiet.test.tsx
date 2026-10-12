import type { On } from 'claude-code'
import { expect } from 'claude-code/testing'
import { cmd, procResult, test } from './testkit'

const NOISE = Array.from({ length: 500 }, (_, i) => `check line ${i}`).join('\n')

function fakeRvow(on: On, stdout: string) {
  on('session.cwd', async () => ({ value: '/tmp' }))
  on('ui.status', async () => ({ value: undefined }))
  on('process.run', async (_$, e) => {
    const isAccept = e.argv[1] === 'accept'
    return { value: procResult(isAccept ? 1 : 0, isAccept ? stdout : '', isAccept ? NOISE : '') }
  })
}

test('a refused accept shows the reason, not the live check output', async ($, on) => {
  fakeRvow(on, 'error[baseline]: the baseline run refused some tasks\n')
  const r = await $.command.run(cmd('rvow-accept', 'CHG-1 yes'))
  expect(r.text).toContain('error[baseline]')
  expect(r.text).not.toContain('check line')
})

test('with no result, only the last 20 lines of stderr show', async ($, on) => {
  fakeRvow(on, '')
  const r = await $.command.run(cmd('rvow-accept', 'CHG-1 yes'))
  expect(r.text).toContain('check line 499')
  expect(r.text).toContain('check line 480')
  expect(r.text).not.toContain('check line 479')
})
