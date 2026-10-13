import type { CommandRunInput, OpEventResult, ProcessRunResult, PromptOrigin } from 'claude-code'
import type { MountTarget, TestRest } from 'claude-code/testing'
import { test as kitTest } from 'claude-code/testing'

export const TEST_MS = 30_000

export const test: (name: string, ...rest: TestRest) => void = (name, ...rest) => {
  if (rest.length === 1) {
    const [body] = rest
    kitTest(name, { timeoutMs: TEST_MS }, body)
  } else {
    const [options, body] = rest
    kitTest(name, { timeoutMs: TEST_MS, ...options }, body)
  }
}

export const COMPOSER: PromptOrigin = { kind: 'composer' }

export const procResult = (exitCode: number, stdout: string, stderr = ''): ProcessRunResult => ({
  exitCode,
  stdout,
  stderr,
  isStdoutTruncated: false,
  isStderrTruncated: false,
})

export const cmd = (
  command: string,
  args = '',
  origin: PromptOrigin = COMPOSER,
): CommandRunInput => ({
  command,
  args,
  origin,
  presentation: { isFullscreen: false, columns: 80 },
})

export const PLACED = { value: { isPlaced: true } } satisfies OpEventResult<'ui.open'>

export const pane = (bodyColumns = 80): MountTarget<'terminal', 'Pane'> => ({
  plugin: 'rvow',
  surface: 'terminal',
  component: 'Pane',
  requestId: 'rvow',
  props: {
    title: 'rvow',
    isFocused: true,
    bodyColumns,
    placement: 'inline',
    scroll: { offset: 0, bodyRows: 20 },
    view: {},
  },
})

export const band = (): MountTarget<'terminal', 'AbovePrompt'> => ({
  plugin: 'rvow',
  surface: 'terminal',
  component: 'AbovePrompt',
  props: {
    hasSurvey: false,
    isWorking: false,
    maxRows: 10,
    bodyColumns: 80,
    scroll: { offset: 0, bodyRows: 20 },
    view: {},
  },
})
