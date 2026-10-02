import type { EngineInterface, Register, RenderChildren } from 'claude-code'
import { atom, read, update } from 'claude-code'

import type { RvowChange, RvowNotice, RvowPlan, RvowTab, RvowView } from '../types'

// `rvow install` writes the absolute path of the binary here.
const INSTALLED_BIN = '__RVOW_BIN__'
const BIN = INSTALLED_BIN.startsWith('__') ? 'rvow' : INSTALLED_BIN
const MAX_BLOCKS = 8
const CHECKED = /^(Bash|Edit|Write|MultiEdit|NotebookEdit|mcp__.*)$/

type Event = 'pre-tool-use' | 'session-start' | 'stop' | 'status' | 'view'

const PANE = 'rvow'
const viewAtom = atom({ plugin: 'rvow', key: 'view' } as const, null)
const reviewAtom = atom({ plugin: 'rvow', key: 'review' } as const, {})
const amendmentsAtom = atom({ plugin: 'rvow', key: 'amendments' } as const, null)
const noticeAtom = atom({ plugin: 'rvow', key: 'notice' } as const, null)
const tabAtom = atom({ plugin: 'rvow', key: 'tab' } as const, null)
const selectedAtom = atom({ plugin: 'rvow', key: 'selected' } as const, [])
const ACCENT = 'cyan'
// The checklist `rvow accept` prints (crates/rvow/src/main.rs REVIEW).
const REVIEW = [
  'before you accept, check:',
  '  1. Does each requirement say what the system does, not how?',
  "  2. Does each task's check fail today for the reason the task fixes?",
  '  3. Is anything in Out of scope that you need now?',
  '  4. Is every Assumption one you agree with?',
  '  5. Is the change small enough to review its diff in one sitting?',
]
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
const OUTCOME = {
  done: ['✓', 'green'],
  dropped: ['✗', 'red'],
  superseded: ['↷', 'yellow'],
} as const

const firstTab = (v: RvowView): RvowTab =>
  v.changes.length > 0 ? 'active' : v.backlog.length > 0 ? 'backlog' : 'archive'

/** Why a plan cannot be accepted here, in the order the author should fix it. */
const blocker = (p: RvowPlan) =>
  !p.here
    ? 'take it to this branch first'
    : p.questions > 0
      ? 'answer the open questions first'
      : p.issues > 0
        ? 'fix what rvow check reports'
        : 'add at least one task with a verify command'

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
  // An older rvow binary has no `backlog` or `archive`; fall back so the pane still draws.
  const view =
    out && Array.isArray(out.changes)
      ? ({
          ...out,
          backlog: out.backlog ?? out.plans ?? [],
          archive: out.archive ?? [],
        } as RvowView)
      : null
  await update($, viewAtom, () => view)
  await refreshStatus($)
}

/** Runs `rvow <argv>`; the timeout covers `accept`, whose baseline runs every verify command. */
async function rvow($: EngineInterface, argv: string[]): Promise<RvowNotice> {
  try {
    const r = await $.process.run([BIN, ...argv], {
      cwd: await $.session.cwd(),
      timeoutMs: 600_000,
    })
    return {
      tone: r.exitCode === 0 ? 'ok' : 'fail',
      text: (r.stdout + r.stderr).trim() || 'no output',
    }
  } catch (err) {
    return { tone: 'fail', text: `cannot run ${BIN}: ${String(err)}` }
  }
}

/** A pane press: runs rvow, keeps the outcome's short form as the notice, then refreshes. */
async function act($: EngineInterface, argv: string[]) {
  await update($, noticeAtom, () => ({ tone: 'busy', text: `running rvow ${argv.join(' ')}` }))
  const r = await rvow($, argv)
  const lines = r.text.split('\n').filter((l) => l.trim())
  const text = r.tone === 'ok' ? (lines.at(-1) ?? '') : lines.slice(0, 3).join('\n')
  await update($, noticeAtom, () => ({ ...r, text }))
  await refresh($)
}

/** Decides every pending amendment with the digest of the listing the person read. */
async function decide($: EngineInterface, listing: string, verb: 'accept' | 'reject') {
  await update($, amendmentsAtom, () => null)
  const digest = /^digest (\S+)$/m.exec(listing)?.[1]
  if (!digest) {
    await update($, noticeAtom, () => ({
      tone: 'fail',
      text: 'no digest in the rvow confirm list',
    }))
    return
  }
  await act($, ['confirm', `--${verb}`, '--all', digest])
}

async function userOnly(
  $: EngineInterface,
  origin: { kind: string },
  name: string,
  argv: string[],
) {
  // Only the person's own Enter may decide: the engine stamps `composer`
  // and no hook or model can.
  if (origin.kind !== 'composer') return { text: `/${name} runs only when you type it yourself.` }
  const r = await rvow($, argv)
  await refresh($)
  return { text: r.text }
}

const REFINE_STEPS =
  'Follow `rvow guide proposal`, `rvow guide spec` and `rvow guide tasks`. Ask me about open points with the rvow_ask tool before changing scope. Then tighten the proposal and tasks so `rvow check` passes, and tell me what changed.'

/** The prompt that asks the agent to refine one plan, or several one at a time. */
function refinePrompt(plans: RvowPlan[]): string {
  const [p] = plans
  if (plans.length === 1 && p) {
    return `Review and refine the rvow plan ${p.id} (${p.title}). Run \`rvow show ${p.id}\` and \`rvow check ${p.id}\`. ${REFINE_STEPS}`
  }
  return [
    'Review and refine these rvow plans, one plan at a time, in this order:',
    ...plans.map((x) => `- ${x.id} (${x.title})`),
    `For each plan: run \`rvow show <id>\` and \`rvow check <id>\`. ${REFINE_STEPS}`,
  ].join('\n')
}

/** Queues the refine prompt as the agent's next turn. */
async function refine($: EngineInterface, plans: RvowPlan[]) {
  void $.prompt
    .submit({ text: refinePrompt(plans), asUser: true })
    .catch((err: unknown) =>
      update($, noticeAtom, () => ({ tone: 'fail', text: `cannot queue: ${String(err)}` })),
    )
  await update($, noticeAtom, () => ({ tone: 'ok', text: 'queued for the agent' }))
}

const ASK = 'mcp__rvow__rvow_ask'
const EDIT_HINT = 'Edit the text each answer changes.'

type Asked = {
  ref: string
  question: string
  header?: string
  options: { label: string; description?: string }[]
}

/** One interview round: `$.ui.ask` per question, `rvow answer` per answer. */
async function ask($: EngineInterface): Promise<string> {
  const cwd = await $.session.cwd()
  const run = (argv: string[]) => $.process.run([BIN, ...argv], { cwd, timeoutMs: 60_000 })
  let round: { questions?: Asked[]; remaining?: number }
  try {
    round = JSON.parse((await run(['clarify', '--json'])).stdout)
  } catch (err) {
    return `cannot read the open questions from ${BIN}: ${String(err)}`
  }
  const qs = round.questions ?? []
  if (qs.length === 0) return 'no open questions'
  const done: string[] = []
  for (const [i, q] of qs.entries()) {
    const detail = q.options.filter((o) => o.description).map((o) => `${o.label}: ${o.description}`)
    const text = [q.question, ...detail].join('\n')
    const stay = () =>
      `cancelled; still open: ${qs
        .slice(i)
        .map((x) => x.ref)
        .join(', ')}`
    let choice: string
    try {
      choice = await $.ui.ask(text, { header: q.header, options: q.options.map((o) => o.label) })
    } catch {
      return [...done, stay()].join('\n')
    }
    const r = await run(['answer', q.ref, choice, '--via', 'dialog']).catch(() => undefined)
    if (!r || r.exitCode !== 0) return [...done, `cannot record ${q.ref}; ${stay()}`].join('\n')
    done.push(`${q.ref} ${q.question} -> ${choice}`)
  }
  return [...done, `${round.remaining ?? 0} questions remain.`, EDIT_HINT].join('\n')
}

export const register: Register = (on) => {
  on('session.start', async ($, e, next) => {
    const started = await next(e)
    await $.command.register({
      name: 'rvow-accept',
      description:
        'Show the rvow plan summary and review checklist: <CHG-id>; record the vow: <CHG-id> yes',
    })
    await $.command.register({
      name: 'rvow-confirm',
      description:
        'List pending rvow amendments, or: accept|reject <AMD-id>..., accept --all <digest>',
    })
    await $.command.register({
      name: 'rvow-status',
      description: 'Show the rvow state of this branch in a pane',
    })
    await $.tool.register({
      name: 'rvow_ask',
      description:
        'Ask the user the open questions of the rvow plan on this branch, one round of at most 4, and record the answers. Use it instead of asking in chat.',
    })
    await refresh($)
    return started
  })

  on('tool.call', { tool: ASK }, async ($) => {
    const result = await ask($)
    await refresh($)
    return { result }
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

  on('command.run', { command: 'rvow-status' }, async ($) => {
    await refresh($)
    await update($, tabAtom, () => null)
    await $.ui.open({ id: PANE, title: 'rvow' })
    return { text: 'rvow pane opened.' }
  })

  on('ui.render', { component: 'Pane', requestId: PANE }, async ($, e) => {
    const { Box, Text, Button } = $.ui.resolve(e)
    const view = await read($, viewAtom)
    const review = await read($, reviewAtom)
    const listing = await read($, amendmentsAtom)
    const notice = await read($, noticeAtom)
    const cols = e.props.bodyColumns
    const home = await $.env.get('HOME')
    const selected = await read($, selectedAtom)

    // Two Buttons with one hotkey clash, so only the first drawn takes it.
    const taken = new Set<string>()
    const hot = (k: string) => {
      if (taken.has(k)) return undefined
      taken.add(k)
      return k
    }
    const button = (
      id: string,
      label: string,
      hotkey: string | undefined,
      onPress: () => unknown,
      primary = false,
    ) => (
      <Button
        key={id}
        label={label}
        hotkey={hotkey}
        plain
        variant={primary ? 'primary' : 'secondary'}
        dimColor={!primary}
        onPress={() => void onPress()}
      />
    )
    const setReview = (id: string, open: boolean) =>
      update($, reviewAtom, (m) => ({ ...m, [id]: open }))

    const header = (
      <Box justifyContent="space-between" marginBottom={1}>
        <Text bold color={ACCENT}>
          rvow
        </Text>
        {view?.branch && <Text dimColor>⎇ {view.branch}</Text>}
      </Box>
    )
    const TONE = { ok: ['✓', 'green'], fail: ['✗', 'red'], busy: ['…', 'yellow'] } as const
    const noticeBox = notice && (
      <Box flexDirection="column" marginBottom={1}>
        {notice.text.split('\n').map((line, i) => (
          <Text wrap="truncate-end">
            <Text color={TONE[notice.tone][1]}>{i === 0 ? TONE[notice.tone][0] : ' '}</Text> {line}
          </Text>
        ))}
      </Box>
    )

    const empty = (line: string, hint?: RenderChildren) => (
      <Box borderStyle="round" borderDimColor flexDirection="column" paddingX={1}>
        <Text>{line}</Text>
        {hint}
      </Box>
    )
    const startHint = (
      <Text wrap="truncate-end">
        <Text dimColor>Start a plan: </Text>
        <Text color={ACCENT}>{'rvow new "<title>"'}</Text>
      </Text>
    )

    if (!view) {
      return (
        <Box flexDirection="column">
          {header}
          {noticeBox}
          {empty('No rvow change on this branch.', startHint)}
        </Box>
      )
    }

    const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`

    const planCard = (p: RvowPlan) => {
      const why = p.ready && p.here ? null : blocker(p)
      const picked = selected.includes(p.id)
      const files = home && p.path.startsWith(`${home}/`) ? `~${p.path.slice(home.length)}` : p.path
      const body = review[p.id] ? (
        <Box flexDirection="column" marginTop={1}>
          {REVIEW.map((line, n) => (
            <Text bold={n === 0}>{line}</Text>
          ))}
          <Box gap={3} marginTop={1}>
            {button(
              `yes:${p.id}`,
              'Accept',
              hot('y'),
              async () => {
                await setReview(p.id, false)
                await act($, ['accept', p.id, '--yes'])
              },
              true,
            )}
            {button(`no:${p.id}`, 'Cancel', hot('n'), () => setReview(p.id, false))}
          </Box>
        </Box>
      ) : (
        <Box flexDirection="column">
          {!why && (
            <Text wrap="truncate-end">
              <Text color={ACCENT}>▸ </Text>
              Review the plan, then accept it.
            </Text>
          )}
          {view.questions
            .filter((q) => q.change === p.id)
            .map((q) => (
              <Text wrap="truncate-end">
                <Text color="yellow">? </Text>
                {q.question}
                {q.recommended && <Text dimColor> (recommended: {q.recommended})</Text>}
              </Text>
            ))}
          <Box gap={3} marginTop={1}>
            {button(
              `accept:${p.id}`,
              'Review & accept',
              hot('a'),
              () =>
                why
                  ? update($, noticeAtom, () => ({ tone: 'fail', text: `Not ready: ${why}.` }))
                  : setReview(p.id, true),
              !why,
            )}
            {!p.here &&
              button(
                `claim:${p.id}`,
                'Take to this branch',
                hot('t'),
                () => act($, ['claim', p.id]),
                true,
              )}
            {p.here &&
              p.questions > 0 &&
              button(
                `ask:${p.id}`,
                'Answer questions',
                hot('q'),
                async () => {
                  const text = await ask($)
                  await update($, noticeAtom, () => ({ tone: 'ok', text }))
                  await refresh($)
                },
                true,
              )}
          </Box>
          {why && (
            <Text dimColor wrap="truncate-end">
              to accept: {why}
            </Text>
          )}
          <Box gap={3} marginTop={1}>
            {button(`refine:${p.id}`, 'Refine with agent', hot('e'), () => refine($, [p]))}
            {button(`select:${p.id}`, picked ? 'Unselect' : 'Select', hot('s'), () =>
              update($, selectedAtom, (ids) =>
                ids.includes(p.id) ? ids.filter((x) => x !== p.id) : [...ids, p.id],
              ),
            )}
            {button(`refresh:${p.id}`, 'Refresh', hot('r'), () => refresh($))}
          </Box>
        </Box>
      )
      return (
        <Box borderStyle="round" borderColor="yellow" flexDirection="column" paddingX={1}>
          <Box justifyContent="space-between" gap={1}>
            <Text wrap="truncate-end">
              <Text color={picked ? ACCENT : undefined} dimColor={!picked}>
                {picked ? '◉' : '○'}
              </Text>{' '}
              <Text inverse bold color="yellow">
                {' PLAN '}
              </Text>{' '}
              <Text bold>{p.title}</Text>
            </Text>
            <Text dimColor>{p.id}</Text>
          </Box>
          {p.here ? (
            <Text color="yellow">● waiting for your accept</Text>
          ) : (
            <Text dimColor wrap="truncate-end">
              on {p.branch}
            </Text>
          )}
          <Text wrap="truncate-end">
            <Text dimColor>
              {plural(p.tasks, 'task')} · {plural(p.questions, 'open question')} · check:{' '}
            </Text>
            {p.issues > 0 ? (
              <Text color="red">{plural(p.issues, 'issue')}</Text>
            ) : p.ready ? (
              <Text color="green">ready</Text>
            ) : (
              <Text color="yellow">not ready</Text>
            )}
          </Text>
          <Text dimColor wrap="truncate-end">
            files: {files}
          </Text>
          {body}
        </Box>
      )
    }

    const changeCard = (c: RvowChange) => {
      const proved = c.tasks.filter((t) => t.state === 'proved').length
      const label = ` ${proved}/${c.tasks.length} proved`
      // The card's border and padding take 4 cells.
      const width = Math.max(4, Math.min(32, cols - 4 - label.length))
      const fill = c.tasks.length ? Math.round((width * proved) / c.tasks.length) : 0
      const next = view.next?.change === c.id ? view.next : null
      const amendButtons =
        listing === null
          ? [
              button(
                `amend:${c.id}`,
                'Review amendments',
                hot('c'),
                async () => {
                  const r = await rvow($, ['confirm'])
                  await update($, amendmentsAtom, () => r.text)
                },
                true,
              ),
            ]
          : [
              button(
                `amend-yes:${c.id}`,
                'Accept all',
                hot('y'),
                () => decide($, listing, 'accept'),
                true,
              ),
              button(`amend-no:${c.id}`, 'Reject all', hot('n'), () =>
                decide($, listing, 'reject'),
              ),
            ]
      return (
        <Box borderStyle="round" borderColor={ACCENT} flexDirection="column" paddingX={1}>
          <Box justifyContent="space-between" gap={1}>
            <Text bold wrap="truncate-end">
              {c.title}
            </Text>
            <Text dimColor>{c.id}</Text>
          </Box>
          <Text>
            <Text color="green">{'█'.repeat(fill)}</Text>
            <Text dimColor>{'░'.repeat(width - fill)}</Text>
            {label}
          </Text>
          <Box flexDirection="column" marginTop={1}>
            {c.tasks.map((t) => {
              const [mark, color] = MARKS[t.state] ?? ['?', undefined]
              return (
                <Text wrap="truncate-end">
                  <Text color={color} dimColor={!color}>
                    {mark}
                  </Text>{' '}
                  <Text dimColor>{t.id} </Text>
                  {t.title}
                  {t.reasons.length > 0 && <Text dimColor> ({t.reasons.join(', ')})</Text>}
                </Text>
              )
            })}
          </Box>
          {(c.drift.length > 0 || c.pending.length > 0) && (
            <Box
              borderStyle="round"
              borderColor="yellow"
              flexDirection="column"
              paddingX={1}
              marginTop={1}
            >
              <Text bold color="yellow">
                needs you
              </Text>
              {c.drift.map((d) => (
                <Text wrap="truncate-end">
                  <Text color={DRIFT[d.class]} dimColor={!DRIFT[d.class]}>
                    {d.class}
                  </Text>{' '}
                  {d.what}
                </Text>
              ))}
              {c.pending.length > 0 &&
                (listing === null
                  ? c.pending.map((p) => (
                      <Text wrap="truncate-end">
                        <Text color="yellow">amend</Text> <Text dimColor>{p.id}</Text> {p.reason}
                      </Text>
                    ))
                  : listing.split('\n').map((line) => (
                      <Text dimColor wrap="truncate-end">
                        {line}
                      </Text>
                    )))}
              {c.pending.length > 0 && (
                <Box gap={3} marginTop={1}>
                  {amendButtons}
                </Box>
              )}
            </Box>
          )}
          {next && (
            <Box
              borderStyle="round"
              borderDimColor
              flexDirection="column"
              paddingX={1}
              marginTop={1}
            >
              <Text wrap="truncate-end">
                <Text bold color={ACCENT}>
                  ▸ next
                </Text>{' '}
                <Text dimColor>{next.task}</Text> {next.title}
              </Text>
              <Text dimColor wrap="truncate-end">
                $ {next.verify}
              </Text>
            </Box>
          )}
        </Box>
      )
    }

    const tab = (await read($, tabAtom)) ?? firstTab(view)
    const picks = view.backlog.filter((p) => selected.includes(p.id))
    const tabs = [
      ['backlog', `Backlog (${view.backlog.length})`],
      ['active', `Active (${view.changes.length})`],
      ['archive', `Archive (${view.archive.length})`],
    ] as const
    // A plain Button takes no color, so a bar under the selected one marks it;
    // each draws as `k: label` with a gap of 3.
    const at = tabs.findIndex(([id]) => id === tab)
    const offset = tabs.slice(0, at).reduce((n, [, label]) => n + label.length + 6, 0)
    const tabBar = (
      <Box flexDirection="column" marginBottom={1}>
        <Box gap={3}>
          {tabs.map(([id, label], n) =>
            button(
              `tab:${id}`,
              label,
              String(n + 1),
              () => update($, tabAtom, () => id),
              id === tab,
            ),
          )}
        </Box>
        <Text bold color={ACCENT}>
          {' '.repeat(offset)}
          {'━'.repeat((tabs[at]?.[1].length ?? 0) + 3)}
        </Text>
      </Box>
    )
    const body =
      tab === 'backlog'
        ? view.backlog.length > 0
          ? [
              picks.length > 0 && (
                <Box key="refine-picked">
                  {button('refine-picked', `Refine ${picks.length} selected`, 'f', async () => {
                    await refine($, picks)
                    await update($, selectedAtom, () => [])
                  })}
                </Box>
              ),
              ...view.backlog.map(planCard),
            ]
          : empty('No plan in the backlog.', startHint)
        : tab === 'active'
          ? view.changes.length > 0
            ? view.changes.map(changeCard)
            : empty(
                'No active change on this branch.',
                <Text dimColor wrap="truncate-end">
                  Accept a plan on the Backlog tab first.
                </Text>,
              )
          : view.archive.length > 0
            ? view.archive.map((a) => {
                const [mark, color] = OUTCOME[a.outcome] ?? ['?', undefined]
                return (
                  <Text wrap="truncate-end">
                    <Text color={color}>{mark}</Text>{' '}
                    <Text dimColor>
                      {a.date} {a.id}
                    </Text>{' '}
                    {a.title}
                  </Text>
                )
              })
            : empty('Nothing archived yet.')

    return (
      <Box flexDirection="column">
        {header}
        {tabBar}
        {noticeBox}
        <Box flexDirection="column" gap={tab === 'archive' ? 0 : 1}>
          {body}
        </Box>
      </Box>
    )
  })

  on('ui.render', { component: 'AbovePrompt' }, async ($, e, next) => {
    const n = (await read($, viewAtom))?.needs_you
    const parts = n
      ? [
          n.plans > 0 && `${n.plans} plan${n.plans === 1 ? '' : 's'} to accept`,
          n.amendments > 0 && `${n.amendments} amendment${n.amendments === 1 ? '' : 's'}`,
          n.questions > 0 && `${n.questions} question${n.questions === 1 ? '' : 's'}`,
        ].filter(Boolean)
      : []
    if (e.props.hasSurvey || parts.length === 0) return next(e)
    const { Text } = $.ui.resolve(e)
    return (
      <Text wrap="truncate-end">
        <Text color="yellow">rvow · needs you: {parts.join(' · ')}</Text>{' '}
        <Text dimColor>/rvow-status</Text>
      </Text>
    )
  })
}
