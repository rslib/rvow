import { expect, test } from 'claude-code/testing'

const COMPOSER = { kind: 'composer' }
const NOISE = Array.from({ length: 500 }, (_, i) => `check line ${i}`).join('\n')

function fakeRvow(on: any, stdout: string) {
  on('session.cwd', async () => ({ value: '/tmp' }) as any)
  on('ui.status', async () => ({ value: undefined }) as any)
  on('process.run', async (_$: any, e: any) => {
    const isAccept = (e.argv as string[])[1] === 'accept'
    return {
      value: {
        exitCode: isAccept ? 1 : 0,
        stdout: isAccept ? stdout : '',
        stderr: isAccept ? NOISE : '',
      },
    } as any
  })
}

test('a refused accept shows the reason, not the live check output', async ($, on) => {
  fakeRvow(on, 'error[baseline]: the baseline run refused some tasks\n')
  const r: any = await $.command.run({
    command: 'rvow-accept',
    args: 'CHG-1 yes',
    origin: COMPOSER,
  } as any)
  expect(r.text).toContain('error[baseline]')
  expect(r.text).not.toContain('check line')
})

test('with no result, only the last 20 lines of stderr show', async ($, on) => {
  fakeRvow(on, '')
  const r: any = await $.command.run({
    command: 'rvow-accept',
    args: 'CHG-1 yes',
    origin: COMPOSER,
  } as any)
  expect(r.text).toContain('check line 499')
  expect(r.text).toContain('check line 480')
  expect(r.text).not.toContain('check line 479')
})
