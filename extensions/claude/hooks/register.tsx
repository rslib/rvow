import { atom, read, update } from 'claude-code'
import type { EngineInterface, Register } from 'claude-code'

import type { RvowChange, RvowView } from '../types'

// `rvow install` writes the absolute path of the binary here.
const INSTALLED_BIN = '__RVOW_BIN__'
const BIN = INSTALLED_BIN.startsWith('__') ? 'rvow' : INSTALLED_BIN
const MAX_BLOCKS = 8
const CHECKED = /^(Bash|Edit|Write|MultiEdit|NotebookEdit|mcp__.*)$/

type Event = 'pre-tool-use' | 'session-start' | 'stop' | 'status' | 'view'

const PANE = 'rvow'
const viewAtom = atom({ plugin: 'rvow', key: 'view' } as const, null)
// Drawing only: the state names and drift classes are rvow's own.
const MARKS: Record<string, [string, string | undefined]> = {
  proved: ['✓', 'green'],
  failed: ['✗', 'red'],
  stale: ['~', 'yellow'],
  flaky: ['≈', 'yellow'],
  blocked: ['⊘', 'magenta'],
  missing: ['·', undefined],
}
const DRIFT: Record<string, string | undefined> = { weaken: 'red', confirm: 'yellow' }

let blocks = 0

/** Runs `rvow hook claude <event>`. Any failure allows, so a broken rvow never locks the session. */
async function hook($: EngineInterface, event: Event, input: object): Promise<any> {
  const cwd = await $.session.cwd()
  try {
    const r = await $.process.run([BIN, 'hook', 'claude', event], {
      cwd,
      stdin: JSON.stringify({ cwd, ...input }),
      timeoutMs: event === 'pre-tool-use' ? 10_000 : 60_000,
    })
    const out = r.stdout.trim()
    return out ? JSON.parse(out) : undefined
  } catch {
    return undefined
  }
}

async function refreshStatus($: EngineInterface) {
  const out = await hook($, 'status', {})
  $.ui.status(typeof out?.status === 'string' ? out.status : undefined)
}

/** Fetches the view into `$.state` so render hooks read it synchronously, then the status line. */
async function refresh($: EngineInterface) {
  const out = await hook($, 'view', {})
  const view = out && Array.isArray(out.changes) ? (out as RvowView) : null
  await update($, viewAtom, () => view)
  await refreshStatus($)
}

async function userOnly($: EngineInterface, origin: { kind: string }, name: string, argv: string[]) {
  // Only the person's own Enter may decide: the engine stamps `composer`
  // and no hook or model can.
  if (origin.kind !== 'composer') return { text: `/${name} runs only when you type it yourself.` }
  try {
    const r = await $.process.run([BIN, ...argv], { cwd: await $.session.cwd(), timeoutMs: 600_000 })
    await refresh($)
    return { text: (r.stdout + r.stderr).trim() || 'no output' }
  } catch (err) {
    return { text: `cannot run ${BIN}: ${String(err)}` }
  }
}

export const register: Register = on => {
  on('session.start', async ($, e, next) => {
    const started = await next(e)
    await $.command.register({
      name: 'rvow-accept',
      description: 'Show the rvow plan summary and review checklist: <CHG-id>; record the vow: <CHG-id> yes',
    })
    await $.command.register({
      name: 'rvow-confirm',
      description: 'List pending rvow amendments, or: accept|reject <AMD-id>..., accept --all <digest>',
    })
    await $.command.register({ name: 'rvow', description: 'Show the rvow state of this branch in a pane' })
    await refresh($)
    return started
  })

  on('tool.call', async ($, e, next) => {
    if (!CHECKED.test(e.tool)) return next(e)
    const { tool, tool_use_id, agentId, consent, ...args } = e as Record<string, unknown>
    const out = await hook($, 'pre-tool-use', { tool_name: tool, tool_input: args })
    const decision = out?.hookSpecificOutput
    if (decision?.permissionDecision === 'deny') {
      return { deny: String(decision.permissionDecisionReason) }
    }
    const ran = await next(e)
    if (tool === 'Bash' && /\brvow\b/.test(String(args.command ?? ''))) await refresh($)
    return ran
  })

  on('classic.SessionStart', async ($, e, next) => {
    const result = await next(e)
    const text = (await hook($, 'session-start', {}))?.hookSpecificOutput?.additionalContext
    if (typeof text !== 'string' || !text) return result
    return { ...result, additionalContext: [...(result.additionalContext ?? []), text] }
  })

  // Claude Code caps settings Stop hooks at 8 blocks in a row; this keeps the
  // same cap for the mod's own block.
  on('classic.Stop', async ($, e, next) => {
    const result = await next(e)
    const reason = (await hook($, 'stop', {}))?.reason
    if (typeof reason !== 'string' || !reason || blocks >= MAX_BLOCKS) {
      blocks = 0
      return result
    }
    blocks += 1
    return { ...result, block: reason }
  })

  on('turn.complete', async ($, e, next) => {
    const done = await next(e)
    await refresh($)
    return done
  })

  on('command.run', { command: 'rvow-confirm' }, async ($, e) => {
    const [verb, ...rest] = e.args.trim().split(/\s+/).filter(Boolean)
    const argv =
      verb === 'accept' || verb === 'reject' ? ['confirm', `--${verb}`, ...rest] : ['confirm']
    return userOnly($, e.origin, 'rvow-confirm', argv)
  })

  on('command.run', { command: 'rvow-accept' }, async ($, e) => {
    const [id, answer] = e.args.trim().split(/\s+/).filter(Boolean)
    if (!id) return { text: 'usage: /rvow-accept <CHG-id>, then /rvow-accept <CHG-id> yes' }
    const argv = answer === 'yes' ? ['accept', id, '--yes'] : ['accept', id]
    return userOnly($, e.origin, 'rvow-accept', argv)
  })

  on('command.run', { command: 'rvow' }, async $ => {
    await refresh($)
    await $.ui.open({ id: PANE, title: 'rvow' })
    return { text: 'rvow pane opened.' }
  })

  on('ui.render', { component: 'Pane', requestId: PANE }, async ($, e) => {
    const { Box, Text } = $.ui.resolve(e)
    const view = await read($, viewAtom)
    if (!view) return <Text dimColor>no active rvow change on this branch</Text>

    const head = (label: string) => <Text bold>{label}</Text>
    const section = (c: RvowChange) => (
      <Box flexDirection="column" marginBottom={1}>
        <Text wrap="truncate-end">
          <Text dimColor>{c.id} </Text>
          <Text bold>{c.title}</Text>
        </Text>
        {c.tasks.map(t => {
          const [mark, color] = MARKS[t.state] ?? ['?', undefined]
          return (
            <Text wrap="truncate-end">
              <Text color={color} dimColor={!color}>{mark}</Text> <Text dimColor>{t.id} </Text>
              {t.title}
              {t.reasons.length > 0 && <Text dimColor> ({t.reasons.join(', ')})</Text>}
            </Text>
          )
        })}
        {c.drift.length > 0 && head('drift')}
        {c.drift.map(d => (
          <Text wrap="truncate-end">
            <Text color={DRIFT[d.class]} dimColor={!DRIFT[d.class]}>{d.class}</Text> {d.what}
          </Text>
        ))}
        {c.pending.length > 0 && head('pending amendments')}
        {c.pending.map(p => (
          <Text wrap="truncate-end">
            <Text dimColor>{p.id} </Text>
            {p.reason} <Text dimColor>/rvow-confirm</Text>
          </Text>
        ))}
      </Box>
    )

    return (
      <Box flexDirection="column">
        {view.branch && <Text dimColor>branch {view.branch}</Text>}
        {view.changes.map(section)}
        {view.plans.length > 0 && head('plans waiting for accept')}
        {view.plans.map(p => (
          <Text wrap="truncate-end">
            <Text dimColor>{p.id} </Text>
            {p.title} <Text dimColor>/rvow-accept {p.id}</Text>
          </Text>
        ))}
        {view.questions.map(q => (
          <Text wrap="truncate-end">
            <Text dimColor>{q.change} </Text>? {q.question}
            {q.recommended && <Text dimColor> (recommended: {q.recommended})</Text>}
          </Text>
        ))}
        {view.next && (
          <Box flexDirection="column" marginTop={1}>
            {head('next')}
            <Text wrap="truncate-end">
              <Text dimColor>{view.next.task} </Text>
              {view.next.title}
            </Text>
            <Text dimColor wrap="truncate-end">$ {view.next.verify}</Text>
          </Box>
        )}
      </Box>
    )
  })

  on('ui.render', { component: 'AbovePrompt' }, async ($, e, next) => {
    const n = (await read($, viewAtom))?.needs_you
    const s = (c: number) => (c === 1 ? '' : 's')
    const parts = n
      ? [
          n.amendments > 0 && `${n.amendments} amendment${s(n.amendments)}`,
          n.questions > 0 && `${n.questions} question${s(n.questions)}`,
          n.plans > 0 && `${n.plans} plan${s(n.plans)} to accept`,
        ].filter(Boolean)
      : []
    if (e.props.hasSurvey || parts.length === 0) return next(e)
    const { Text } = $.ui.resolve(e)
    return (
      <Text wrap="truncate-end">
        <Text color="yellow">rvow needs you: {parts.join(' · ')}</Text>{' '}
        <Text dimColor>/rvow-confirm /rvow-accept</Text>
      </Text>
    )
  })
}
