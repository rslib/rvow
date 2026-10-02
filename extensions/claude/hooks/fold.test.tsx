import { expect, test } from 'claude-code/testing'

const row = (command: string, text: string) =>
  ({
    plugin: 'rvow',
    surface: 'terminal',
    component: 'CommandOutput',
    props: { command, args: '', text, isErrored: false },
  }) as any
const LONG = Array.from({ length: 40 }, (_, i) => `line ${i}`).join('\n')

test('a long rvow command output shows 8 lines and a count of the rest', async ($, on) => {
  on('ui.render', { component: 'CommandOutput' }, async (_$: any, e: any) => {
    const { Text } = _$.ui.resolve(e)
    return <Text>engine</Text>
  })
  const long = await $.ui.mount(row('rvow-accept', LONG))
  const text = (await long.findAll({ type: 'Text' })).map((t: any) => t.text).join('\n')
  expect(text).toContain('line 7')
  expect(text).not.toContain('line 8')
  expect(text).toContain('32 more lines')
  const short = await $.ui.mount(row('rvow-accept', 'accepted CHG-1'))
  expect((await short.find({ type: 'Text' }))?.text).toBe('engine')
  const other = await $.ui.mount(row('cost', LONG))
  expect((await other.find({ type: 'Text' }))?.text).toBe('engine')
})
