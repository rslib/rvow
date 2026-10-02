import type { EngineInterface, Register } from 'claude-code'

// `rvow install` writes the absolute path of the binary here.
const INSTALLED_BIN = '__RVOW_BIN__'
const BIN = INSTALLED_BIN.startsWith('__') ? 'rvow' : INSTALLED_BIN
const MAX_BLOCKS = 8
const CHECKED = /^(Bash|Edit|Write|MultiEdit|NotebookEdit|mcp__.*)$/

type Event = 'pre-tool-use' | 'session-start' | 'stop' | 'status'

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

async function userOnly($: EngineInterface, origin: { kind: string }, name: string, argv: string[]) {
  // Only the person's own Enter may decide: the engine stamps `composer`
  // and no hook or model can.
  if (origin.kind !== 'composer') return { text: `/${name} runs only when you type it yourself.` }
  try {
    const r = await $.process.run([BIN, ...argv], { cwd: await $.session.cwd(), timeoutMs: 600_000 })
    await refreshStatus($)
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
    await refreshStatus($)
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
    return next(e)
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
    await refreshStatus($)
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
}
