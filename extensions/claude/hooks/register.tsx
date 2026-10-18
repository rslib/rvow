import type { EngineInterface, Register, RenderChildren } from 'claude-code'
import { atom, read, update } from 'claude-code'
import type { RvowChange, RvowDetail, RvowNotice, RvowPlan, RvowTab, RvowView } from '../types'
import { ACCEPT_MS, declinedAcceptReason, repeatAcceptResult, shortReview } from './accept'
import {
  ACCENT,
  ACCEPT,
  ACCEPT_DESCRIPTION,
  ACCEPT_SCHEMA,
  ARCHIVE,
  ARCHIVE_DESCRIPTION,
  ARCHIVE_SCHEMA,
  ASK,
  ASK_SCHEMA,
  type Asked,
  archiveBlockers,
  BADGE,
  BRANCH,
  blocker,
  CONFIRM,
  CONFIRM_DESCRIPTION,
  CONFIRM_SCHEMA,
  cut,
  DRIFT,
  decisionPrompt,
  EDIT_HINT,
  failedPrompt,
  failedText,
  firstTab,
  formatJudgeFindings,
  ID,
  isDecision,
  JUDGE,
  JUDGE_AGENT,
  JUDGE_AGENT_DESCRIPTION,
  JUDGE_DESCRIPTION,
  JUDGE_NO_FINDINGS,
  JUDGE_PROMPT_FALLBACK,
  JUDGE_SCHEMA,
  JUDGE_TOOLS,
  JudgeInbox,
  judgeArgs,
  judgeRecordArgv,
  judgeSpawnPrompt,
  type Listed,
  MARKS,
  MAX_LINES,
  OPEN,
  OUTCOME,
  PANE,
  parseJudgeAnswer,
  REVIEW,
  RULE,
  refinePrompt,
  splitListing,
  TABS,
  YOLO,
  YOLO_DESCRIPTION,
  YOLO_SCHEMA,
  YOLO_TIERS,
  yoloArgv,
} from './format'

type AcceptMemory = Record<string, { review: string; reason: string }>
const acceptMemoryAtom = atom({ plugin: 'rvow', key: 'accept-memory' } as const, {})
const viewAtom = atom({ plugin: 'rvow', key: 'view' } as const, null)
const reviewAtom = atom({ plugin: 'rvow', key: 'review' } as const, {})
const amendmentsAtom = atom({ plugin: 'rvow', key: 'amendments' } as const, null)
const noticeAtom = atom({ plugin: 'rvow', key: 'notice' } as const, null)
const tabAtom = atom({ plugin: 'rvow', key: 'tab' } as const, null)
const selectedAtom = atom({ plugin: 'rvow', key: 'selected' } as const, [])
const detailsAtom = atom({ plugin: 'rvow', key: 'details' } as const, [])

// `rvow install` writes the absolute path of the binary here.
const INSTALLED_BIN = '__RVOW_BIN__'
const BIN = INSTALLED_BIN.startsWith('__') ? 'rvow' : INSTALLED_BIN
const MAX_BLOCKS = 8
const CHECKED = /^(Bash|Edit|Write|MultiEdit|NotebookEdit|Read|Grep|Glob|mcp__.*)$/

type Event =
  | 'pre-tool-use'
  | 'post-tool-use'
  | 'session-start'
  | 'stop'
  | 'status'
  | 'view'
  | 'lease-bind'
  | 'lease-end'

let blocks = 0
let warned = false
const STDERR_LINES = 20
const FOLD_LINES = 8
/** The last brief sent to the model; a prompt carries the brief only when it changed. */
let sentBrief = ''
/** A tool call carries no session id, so the last prompt's id goes with `pre-tool-use`. */
let sessionId: string | undefined
/** The pending judge runs and the subagent answers on their way to them. */
const judgePending = new JudgeInbox()
const BRIEF_WAIT_MS = 5_000

/** Runs `work` after the current event returns, so the session never waits for rvow. */
function later($: EngineInterface, work: () => Promise<unknown>) {
  $.clock.after(0, () => {
    void work().catch(() => undefined)
  })
}

/** What `rvow hook claude <event>` prints; each event fills only some fields. */
type HookOutput = {
  status?: unknown
  reason?: unknown
  hookSpecificOutput?: {
    permissionDecision?: unknown
    permissionDecisionReason?: unknown
    additionalContext?: unknown
  }
  changes?: unknown
  plans?: RvowPlan[]
  backlog?: RvowPlan[]
  archive?: RvowView['archive']
}

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === 'object' && v !== null && !Array.isArray(v)

function isDetail(v: unknown): v is RvowDetail {
  return (
    isRecord(v) &&
    (v.kind === 'plan' || v.kind === 'change' || v.kind === 'task' || v.kind === 'archive') &&
    typeof v.id === 'string'
  )
}

/** Runs `rvow hook claude <event>`. Any failure allows, so a broken rvow never locks the session. */
async function hook(
  $: EngineInterface,
  event: Event,
  input: object,
): Promise<HookOutput | undefined> {
  const cwd = await $.session.cwd()
  try {
    const r = await $.process.run([BIN, 'hook', 'claude', event], {
      cwd,
      stdin: JSON.stringify({ cwd, ...input }),
      timeoutMs: event === 'pre-tool-use' ? 10_000 : 60_000,
    })
    const out = r.stdout.trim()
    const parsed: unknown = out ? JSON.parse(out) : undefined
    return isRecord(parsed) ? parsed : undefined
  } catch (err) {
    if (!warned) {
      warned = true
      $.ui.toast(
        `rvow could not run, so its checks are off: ${cut(String((err as Error)?.message ?? err), 80)}`,
      )
    }
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
  for (const d of await read($, detailsAtom)) {
    const r = await showText($, d)
    // Keeps the old text on a failed show; ids and kinds are unchanged, so the saved pane stays.
    if (r.tone === 'ok') {
      await update($, detailsAtom, (s) =>
        s.map((x) => (x.id === d.id && x.kind === d.kind ? { ...x, text: r.text } : x)),
      )
    }
  }
  await refreshStatus($)
}

/** Runs `rvow <argv>` with one bounded wait. */
async function rvow($: EngineInterface, argv: string[]): Promise<RvowNotice> {
  try {
    const r = await $.process.run([BIN, ...argv], {
      cwd: await $.session.cwd(),
      timeoutMs: ACCEPT_MS,
    })
    // stderr holds the live output of check runs; it shows only when there is no result.
    const text = r.stdout.trim() || r.stderr.trim().split('\n').slice(-STDERR_LINES).join('\n')
    return { tone: r.exitCode === 0 ? 'ok' : 'fail', text: text || 'no output' }
  } catch (err) {
    return { tone: 'fail', text: `cannot run ${BIN}: ${String(err)}` }
  }
}

/** Runs `rvow <argv>` for a tool and returns its words, so a failure reaches the agent in the tool result. */
async function tellOut($: EngineInterface, argv: string[]) {
  const cwd = await $.session.cwd()
  const ran = await $.process.run([BIN, ...argv], { cwd, timeoutMs: 600_000 })
  const out = ran.stdout.trim() || ran.stderr.trim()
  await refresh($)
  return { ok: ran.exitCode === 0, out }
}

async function waiting($: EngineInterface) {
  await refresh($)
  return (await read($, viewAtom))?.backlog.filter((x) => x.here && x.ready) ?? []
}

function list(ps: { id: string; title: string }[]) {
  return ps.length
    ? [`${ps.length} plans wait; name one:`, ...ps.map((p) => `${p.id} ${p.title}`)].join('\n')
    : 'no plan waits on this branch; see /rvow-status'
}

/** A pane press: runs rvow, keeps the outcome's short form as the notice, then refreshes. */
async function act($: EngineInterface, argv: string[]) {
  await update($, noticeAtom, () => ({ tone: 'busy', text: `running rvow ${argv.join(' ')}` }))
  const r = await rvow($, argv)
  await settle($, r)
  tellAgentOfFailure($, argv, r)
}

/** A decision that failed is not silent for the agent: it is told what failed and why. */
function tellAgentOfFailure($: EngineInterface, argv: string[], r: RvowNotice) {
  if (r.tone === 'fail' && isDecision(argv)) {
    later($, async () => queue($, failedPrompt(argv, r.text)))
  }
}

/** Keeps the short form of an outcome as the notice, then refreshes. */
async function settle($: EngineInterface, r: RvowNotice) {
  const lines = r.text.split('\n').filter((l) => l.trim())
  const text = r.tone === 'ok' ? (lines.at(-1) ?? '') : lines.slice(0, 3).join('\n')
  await update($, noticeAtom, () => ({ ...r, text }))
  await refresh($)
}

/** Writes `a`, then saves the open stack (no fetched text) and the expanded reviews for the next restart. */
async function save($: EngineInterface) {
  try {
    const details = (await read($, detailsAtom)).map(({ kind, id, title }) => ({ kind, id, title }))
    const flags = await read($, reviewAtom)
    const review = Object.keys(flags).filter((id) => flags[id])
    await $.store.set('pane', { details, review })
  } catch {}
}

const setDetails = ($: EngineInterface, fn: (s: RvowDetail[]) => RvowDetail[]): Promise<void> =>
  update($, detailsAtom, fn).then(() => save($))
const putReview = (
  $: EngineInterface,
  fn: (m: Record<string, boolean>) => Record<string, boolean>,
): Promise<void> => update($, reviewAtom, fn).then(() => save($))

/** Reopens the saved stack and expanded reviews that still exist in the view; the rest is dropped. */
async function restore($: EngineInterface) {
  try {
    const saved = await $.store.get('pane')
    const view = await read($, viewAtom)
    if (!view || !isRecord(saved)) return
    const alive = (d: RvowDetail) =>
      d.kind === 'plan'
        ? view.backlog.some((p) => p.id === d.id)
        : d.kind === 'change'
          ? view.changes.some((c) => c.id === d.id)
          : d.kind === 'archive'
            ? view.archive.some((a) => a.id === d.id)
            : view.changes.some((c) => c.tasks.some((t) => t.id === d.id))
    const open = (Array.isArray(saved.review) ? (saved.review as string[]) : []).filter((id) =>
      view.backlog.some((p) => p.id === id),
    )
    await update($, reviewAtom, () => Object.fromEntries(open.map((id) => [id, true])))
    for (const d of Array.isArray(saved.details) ? saved.details.filter(isDetail) : []) {
      if (alive(d)) await openDetail($, { kind: d.kind, id: d.id, title: String(d.title) })
    }
  } catch {}
}

/** `rvow show <id> --human`; a task also gets its covered requirements. */
async function showText($: EngineInterface, d: Omit<RvowDetail, 'text'>): Promise<RvowNotice> {
  const shown = await rvow($, ['show', d.id, '--human'])
  let text = shown.text
  if (d.kind === 'task') {
    const covers = /^\s*covers:\s*(.+)$/m.exec(text)?.[1]?.match(ID) ?? []
    for (const req of covers) {
      // The requirement's own heading replaces the task's one-line mention of it.
      text = text
        .split('\n')
        .filter((l) => !l.startsWith(`${req} `))
        .join('\n')
      text += `\n\n${(await rvow($, ['show', req, '--human'])).text}`
    }
  }
  return { tone: shown.tone, text }
}

/** Pushes a detail view and fills it with its show text. */
async function openDetail($: EngineInterface, d: Omit<RvowDetail, 'text'>) {
  await setDetails($, (s) => [...s, { ...d, text: null }])
  const { text } = await showText($, d)
  await update($, detailsAtom, (s) =>
    s.map((x) => (x.id === d.id && x.text === null ? { ...x, text } : x)),
  )
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
  await update($, noticeAtom, () => ({ tone: 'busy', text: `running rvow confirm --${verb}` }))
  const r = await rvow($, ['confirm', `--${verb}`, '--all', digest])
  // A stale digest makes rvow print the current list: show it so the person reads it again.
  if (r.tone === 'fail' && /^digest \S+$/m.test(r.text)) {
    await update($, amendmentsAtom, () => r.text)
    await update($, noticeAtom, () => ({
      tone: 'fail',
      text: 'the pending amendments changed; read the new list',
    }))
    return
  }
  await settle($, r)
  tellAgentOfFailure($, ['confirm', `--${verb}`, '--all', digest], r)
}

const EACH_LABEL = { accept: 'Accept', reject: 'Reject' } as const

/**
 * Asks about each pending amendment in turn and runs `decide` for each accept or reject.
 * A later, or a dialog the person closed, leaves that amendment pending.
 */
async function confirmEach(
  $: EngineInterface,
  items: Listed[],
  first: 'accept' | 'reject',
  decide: (item: Listed, verb: 'accept' | 'reject') => Promise<string>,
): Promise<string[]> {
  const second = first === 'accept' ? 'reject' : 'accept'
  const options = [EACH_LABEL[first], EACH_LABEL[second], 'Later']
  const lines: string[] = []
  for (const [i, item] of items.entries()) {
    const answer = await $.ui
      .ask(`${item.text}\n\n${i + 1} of ${items.length}: accept, reject, or decide later?`, options)
      .catch(() => 'Later')
    const verb = answer === 'Accept' ? 'accept' : answer === 'Reject' ? 'reject' : null
    lines.push(verb ? await decide(item, verb) : `${item.id} is still pending.`)
  }
  return lines
}

/** `/rvow-confirm` with no argument or `all`: list the pending plan edits and ask once, or once for each. */
async function confirmDialog($: EngineInterface, origin: { kind: string }) {
  if (origin.kind !== 'composer')
    return { text: '/rvow-confirm runs only when you type it yourself.' }
  const listing = await rvow($, ['confirm'])
  await refresh($)
  if (listing.tone === 'fail') return { text: listing.text }
  const digest = /^digest (\S+)$/m.exec(listing.text)?.[1]
  if (!digest) return { text: listing.text }
  const items = splitListing(listing.text)
  if (items.length > 1) {
    const prompts: string[] = []
    const lines = await confirmEach($, items, 'accept', async (item, verb) => {
      const argv = ['confirm', item.id, `--${verb}`]
      const done = await rvow($, argv)
      const prompt = done.tone === 'ok' ? decisionPrompt(argv, done.text) : null
      if (prompt) prompts.push(prompt)
      tellAgentOfFailure($, argv, done)
      return done.text
    })
    await refresh($)
    // The engine refuses a submit made while this hook still holds the command.
    if (prompts.length > 0) later($, async () => queue($, prompts.join(' ')))
    return { text: lines.join('\n') }
  }
  const answer = await $.ui
    .ask(`${listing.text}\n\nAccept all, reject all, or decide later?`, [
      'Accept all',
      'Reject all',
      'Later',
    ])
    .catch(() => 'Later')
  if (answer !== 'Accept all' && answer !== 'Reject all') {
    return { text: 'The plan edits are still pending.' }
  }
  const argv = ['confirm', answer === 'Accept all' ? '--accept' : '--reject', '--all', digest]
  const done = await rvow($, argv)
  await refresh($)
  const prompt = done.tone === 'ok' ? decisionPrompt(argv, done.text) : null
  // The engine refuses a submit made while this hook still holds the command.
  if (prompt) later($, async () => queue($, prompt))
  tellAgentOfFailure($, argv, done)
  return { text: done.text }
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
  const prompt = r.tone === 'ok' ? decisionPrompt(argv, r.text) : null
  // The engine refuses a submit made while this hook still holds the command.
  if (prompt) later($, async () => queue($, prompt))
  tellAgentOfFailure($, argv, r)
  return { text: r.text }
}

/** Queues a prompt as the agent's next turn, as the user. */
function queue($: EngineInterface, text: string) {
  void $.prompt
    .submit({ text, asUser: true })
    .catch((err: unknown) =>
      update($, noticeAtom, () => ({ tone: 'fail', text: `cannot queue: ${String(err)}` })),
    )
}

/** Queues the refine prompt as the agent's next turn. */
async function refine($: EngineInterface, plans: RvowPlan[]) {
  queue($, refinePrompt(plans))
  await update($, noticeAtom, () => ({ tone: 'ok', text: 'queued for the agent' }))
}

/** Registers the read-only judge agent type with the shared judge prompt. */
async function registerJudgeAgent($: EngineInterface) {
  const guide = await rvow($, ['guide', 'judge'])
  const prompt = guide.tone === 'ok' && guide.text.trim() ? guide.text : JUDGE_PROMPT_FALLBACK
  try {
    await $.agent.register({
      name: 'judge',
      description: JUDGE_AGENT_DESCRIPTION,
      prompt,
      tools: [...JUDGE_TOOLS],
    })
  } catch {
    // A broken registration never locks the session; /rvow-judge reports it per run.
  }
}

/** The active changes of this branch, newest effort first. */
async function judgeChanges($: EngineInterface): Promise<{ id: string; title: string }[]> {
  const out = await hook($, 'view', {})
  const changes = Array.isArray(out?.changes) ? out.changes : []
  return changes
    .filter(
      (c): c is { id: string; title: string } =>
        isRecord(c) && typeof c.id === 'string' && typeof c.title === 'string',
    )
    .map((c) => ({ id: c.id, title: c.title }))
}

/**
 * Runs the judge for one change: builds the packet, spawns the read-only judge
 * agent, and records its findings with the model the subagent ran on. The mod
 * records, never the agent: the hook denies an agent's own `--judge` call.
 */
async function judgeRun($: EngineInterface, argsText: string): Promise<{ text: string }> {
  // Counted from the start, not from the spawn: an answer can arrive before
  // the spawn returns, and it is kept only while a run is pending.
  judgePending.begin()
  try {
    return await judgeOnce($, argsText)
  } finally {
    judgePending.end()
  }
}

async function judgeOnce($: EngineInterface, argsText: string): Promise<{ text: string }> {
  // The type is registered at session start; this covers a session that
  // started before the registration landed, and is a no-op replace after.
  await registerJudgeAgent($)
  const { change, model } = judgeArgs(argsText)
  let id = change
  if (!id) {
    const active = await judgeChanges($)
    if (active.length === 0) return { text: 'no active change on this branch; see /rvow-status' }
    const [only] = active
    if (!only || active.length > 1) {
      return {
        text: [
          `${active.length} changes are active; name one:`,
          ...active.map((c) => `${c.id} ${c.title}`),
        ].join('\n'),
      }
    }
    id = only.id
  }
  const packet = await rvow($, ['review', id, '--packet', '--human'])
  if (packet.tone !== 'ok') return { text: packet.text }
  const identity = /^identity: (\S+)/m.exec(packet.text)?.[1]
  if (!identity) return { text: `cannot read the packet of ${id}` }
  const body = packet.text.split('\n').slice(1).join('\n')
  const guide = await rvow($, ['guide', 'judge', '--human'])
  const prompt =
    guide.tone === 'ok' && guide.text.trim()
      ? judgeSpawnPrompt(guide.text, identity, body)
      : judgeSpawnPrompt(JUDGE_PROMPT_FALLBACK, identity, body)
  let started: { model: string; agentId?: string } | { deny: string }
  try {
    started = await $.agent.spawn({
      prompt,
      subagentType: JUDGE_AGENT,
      ...(model ? { model } : {}),
    })
  } catch (err) {
    return { text: `cannot start the judge: ${String(err)}` }
  }
  if ('deny' in started) return { text: started.deny }
  // The subagent's answer arrives as its turn.complete, whatever its reason,
  // so this wait always ends.
  const answer = await judgePending.answer(started.agentId)
  const parsed = answer ? parseJudgeAnswer(answer, identity) : null
  if (!parsed) return { text: JUDGE_NO_FINDINGS }
  const cwd = await $.session.cwd()
  let recorded: { exitCode: number; stdout: string; stderr: string }
  try {
    recorded = await $.process.run([BIN, ...judgeRecordArgv(id, started.model)], {
      cwd,
      stdin: parsed.text,
      timeoutMs: 600_000,
    })
  } catch (err) {
    return { text: `cannot record the judge: ${String(err)}` }
  }
  if (recorded.exitCode !== 0) {
    return { text: recorded.stdout.trim() || recorded.stderr.trim() || 'cannot record the judge' }
  }
  await refresh($)
  return { text: formatJudgeFindings(id, started.model, parsed.findings) }
}

/** One interview round: `$.ui.ask` per question, `rvow answer` per answer. */
async function ask($: EngineInterface, change?: string): Promise<string> {
  const cwd = await $.session.cwd()
  const run = (argv: string[]) => $.process.run([BIN, ...argv], { cwd, timeoutMs: 60_000 })
  let round: { questions?: Asked[]; remaining?: number }
  try {
    round = JSON.parse((await run(['clarify', ...(change ? [change] : []), '--json'])).stdout)
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
    if (r?.exitCode !== 0) return [...done, `cannot record ${q.ref}; ${stay()}`].join('\n')
    done.push(`${q.ref} ${q.question} -> ${choice}`)
  }
  return [...done, `${round.remaining ?? 0} questions remain.`, EDIT_HINT].join('\n')
}

export const register: Register = (on) => {
  // A compaction that stands folds the brief into a summary, so the next prompt carries it again.
  // A skipped one, a subagent's own and a `precompute` install nothing and leave the brief alone.
  on('session.compact', async (_$, e, next) => {
    const out = await next(e)
    if (out.messages && !e.agentId && e.trigger !== 'precompute') sentBrief = ''
    return out
  })

  on('session.start', async ($, e, next) => {
    // A new session (startup, clear, resume) has no brief yet, no warning shown, no blocks in a row.
    sentBrief = ''
    warned = false
    blocks = 0
    const started = await next(e)
    await $.command.register({
      name: 'rvow-accept',
      description: 'Record the vow for a plan: <CHG-id>',
    })
    await $.command.register({
      name: 'rvow-review',
      description: 'Show an rvow plan and the accept checklist, without running anything: <CHG-id>',
    })
    await $.command.register({
      name: 'rvow-confirm',
      description:
        'List pending rvow amendments, or: accept|reject <AMD-id>..., accept --all <digest>',
    })
    await $.command.register({
      name: 'rvow-archive',
      description: 'Archive an rvow change whose tasks are all proved: <CHG-id>',
    })
    await $.command.register({
      name: 'rvow-yolo',
      description:
        'Relax the hooks for a while: audit|auto|bypass [30m|2h], off, or nothing to see it',
    })
    await $.command.register({
      name: 'rvow-status',
      description: 'Show the rvow state of this branch in a pane',
    })
    await $.command.register({
      name: 'rvow-judge',
      description: 'Run the spec conformance judge for a change: [CHG-id] [model]',
    })
    await $.tool.register({
      name: 'rvow_ask',
      description:
        'Use this tool for the open questions of the rvow plan on this branch, one round of at most 3, and record the answers. Pass change to ask the open questions of that change, whatever branch the session is on. Ask in chat only when it is missing.',
      inputSchema: ASK_SCHEMA,
    })
    await $.tool.register({
      name: 'rvow_accept',
      description: ACCEPT_DESCRIPTION,
      inputSchema: ACCEPT_SCHEMA,
    })
    await $.tool.register({
      name: 'rvow_archive',
      description: ARCHIVE_DESCRIPTION,
      inputSchema: ARCHIVE_SCHEMA,
    })
    await $.tool.register({
      name: 'rvow_confirm',
      description: CONFIRM_DESCRIPTION,
      inputSchema: CONFIRM_SCHEMA,
    })
    await $.tool.register({
      name: 'rvow_yolo',
      description: YOLO_DESCRIPTION,
      inputSchema: YOLO_SCHEMA,
    })
    await $.tool.register({
      name: 'rvow_judge',
      description: JUDGE_DESCRIPTION,
      inputSchema: JUDGE_SCHEMA,
    })
    later($, () => registerJudgeAgent($))
    later($, () => refresh($))
    return started
  })

  // The judge type is spawned by the mod itself, never offered to the model.
  on('agent.offer', { agent: JUDGE_AGENT }, async () => ({ isOffered: false }))

  // The agent may ask for a tier. Only the user's answer in the dialog switches one on, and the tool
  // runs the CLI itself, since the hook refuses an agent that runs `rvow yolo` in a shell.
  on('tool.call', { tool: YOLO }, async ($, e) => {
    const { tool, tool_use_id, agentId, consent, ...input } = e as Record<string, unknown>
    const argv = yoloArgv(input)
    if (typeof argv === 'string') return { result: argv }
    const tier = argv[1]
    if (tier && tier !== 'off') {
      const length = argv[3] ?? '2h'
      const question = `The agent asks to turn on yolo ${tier} for ${length}: ${YOLO_TIERS[tier as keyof typeof YOLO_TIERS]}. The gate still checks at push. Allow it?`
      const answer = await $.ui.ask(question, ['Allow', 'Deny']).catch(() => 'Deny')
      if (answer !== 'Allow') return { result: 'The user declined, so yolo stays as it was.' }
    }
    const cwd = await $.session.cwd()
    const ran = await $.process.run([BIN, ...argv], { cwd, timeoutMs: 60_000 })
    await refresh($)
    return { result: ran.stdout || ran.stderr }
  })

  // These tools are the agent's way to the user's decisions. The user answers in a dialog, the tool runs
  // the CLI itself, and the result carries the failure text, so the agent is never left guessing.
  on('tool.call', { tool: ACCEPT }, async ($, e) => {
    const change = (e as Record<string, unknown>).change
    if (typeof change !== 'string' || !/^CHG-[a-z0-9]+$/.test(change)) {
      return { result: 'rvow: change must be a complete CHG id' }
    }
    const review = await tellOut($, ['review', change])
    if (!review.ok) return { result: failedText(['review', change], review.out) }
    // A repeat while the plan is unchanged asks nothing: the earlier reason stands.
    const remembered = ((await read($, acceptMemoryAtom)) as unknown as AcceptMemory)[change]
    if (remembered && remembered.review === review.out)
      return { result: repeatAcceptResult(remembered.reason, change) }
    const answer = await $.ui
      .ask(`${shortReview(review.out, change)}\n\nAccept ${change}?`, ['Accept', 'Not now'])
      .catch(() => 'Not now')
    if (answer !== 'Accept') {
      const reason = declinedAcceptReason(change)
      await update($, acceptMemoryAtom, (m) => ({
        ...((m as unknown as AcceptMemory) ?? {}),
        [change]: { review: review.out, reason },
      }))
      return { result: repeatAcceptResult(reason, change) }
    }
    const done = await rvow($, ['accept', change, '--yes'])
    if (done.tone === 'ok') {
      await update($, acceptMemoryAtom, (m) => {
        const next = { ...((m as unknown as AcceptMemory) ?? {}) }
        delete next[change]
        return next
      })
      return { result: done.text }
    }
    const reason = failedText(['accept', change], done.text)
    await update($, acceptMemoryAtom, (m) => ({
      ...((m as unknown as AcceptMemory) ?? {}),
      [change]: { review: review.out, reason },
    }))
    return { result: reason }
  })

  on('tool.call', { tool: ARCHIVE }, async ($, e) => {
    const change = (e as Record<string, unknown>).change
    if (typeof change !== 'string' || !/^CHG-[a-z0-9]+$/.test(change)) {
      return { result: 'rvow: change must be a complete CHG id' }
    }
    const seen = await tellOut($, ['hook', 'claude', 'view'])
    if (!seen.ok) return { result: failedText(['hook', 'claude', 'view'], seen.out) }
    let view: {
      changes?: { id: string; tasks: { id: string; state: string }[]; pending: unknown[] }[]
    }
    try {
      view = JSON.parse(seen.out)
    } catch {
      return { result: 'rvow: cannot read the state of the branch' }
    }
    const blocked = archiveBlockers(
      view.changes?.find((c) => c.id === change),
      change,
    )
    if (blocked) return { result: blocked }
    const answer = await $.ui
      .ask(
        `Every task of ${change} is proved or stale, and archive re-proves the stale ones. Archive it? The gate trusts an archived change and never reruns its tasks.`,
        ['Archive', 'Not yet'],
      )
      .catch(() => 'Not yet')
    if (answer !== 'Archive')
      return {
        result: `The user did not archive ${change}; it stays active. The user can run /rvow-archive ${change} to archive it.`,
      }
    const done = await tellOut($, ['archive', change])
    return { result: done.ok ? done.out : failedText(['archive', change], done.out) }
  })

  on('tool.call', { tool: CONFIRM }, async ($, e) => {
    const decision = (e as Record<string, unknown>).decision
    if (decision !== undefined && decision !== 'accept' && decision !== 'reject') {
      return { result: 'rvow: decision must be accept or reject' }
    }
    const named = (e as Record<string, unknown>).id
    if (named !== undefined && (typeof named !== 'string' || !/^AMD-[a-z0-9]+$/.test(named))) {
      return { result: 'rvow: id must be an AMD- id' }
    }
    const listing = await tellOut($, ['confirm'])
    if (!listing.ok) return { result: failedText(['confirm'], listing.out) }
    if (named !== undefined) {
      const item = splitListing(listing.out).find((i) => i.id === named)
      if (!item) return { result: `${named} is not pending.` }
      if (decision === undefined) return { result: item.text }
      const answer = await $.ui
        .ask(`${item.text}\n\n${decision === 'accept' ? 'Accept' : 'Reject'} ${named}?`, [
          'Yes',
          'No',
        ])
        .catch(() => 'No')
      if (answer !== 'Yes') {
        return {
          result: `The user did not decide ${named}, so it is still pending. The user can run /rvow-confirm to decide it.`,
        }
      }
      const argv = ['confirm', named, `--${decision}`]
      const done = await tellOut($, argv)
      return { result: done.ok ? done.out : failedText(argv, done.out) }
    }
    if (decision === undefined) return { result: listing.out }
    const items = splitListing(listing.out)
    if (items.length > 1) {
      const lines = await confirmEach($, items, decision, async (item, verb) => {
        const argv = ['confirm', item.id, `--${verb}`]
        const done = await tellOut($, argv)
        return done.ok ? done.out : failedText(argv, done.out)
      })
      return { result: lines.join('\n') }
    }
    const digest = /^digest (\S+)$/m.exec(listing.out)?.[1]
    if (!digest) return { result: listing.out || 'There are no pending plan edits.' }
    const answer = await $.ui
      .ask(`${listing.out}\n\n${decision === 'accept' ? 'Accept' : 'Reject'} all of these?`, [
        'Yes',
        'No',
      ])
      .catch(() => 'No')
    if (answer !== 'Yes')
      return {
        result:
          'The user did not decide, so the plan edits are still pending. The user can run /rvow-confirm to decide them.',
      }
    const argv = ['confirm', `--${decision}`, '--all', digest]
    const done = await tellOut($, argv)
    return { result: done.ok ? done.out : failedText(argv, done.out) }
  })

  on('tool.call', { tool: ASK }, async ($, e) => {
    const change = (e as Record<string, unknown>).change
    if (change !== undefined && (typeof change !== 'string' || !/^CHG-[a-z0-9]+$/.test(change))) {
      return { result: 'rvow: change must be a CHG- id' }
    }
    const result = await ask($, change)
    await refresh($)
    return { result }
  })

  // The agent's way to the judge: the mod builds the packet, spawns the
  // read-only judge agent and records its findings itself, since the hook
  // denies an agent's own `rvow review --judge` call.
  on('tool.call', { tool: JUDGE }, async ($, e) => {
    const input = e as Record<string, unknown>
    const change = typeof input.change === 'string' ? input.change : ''
    const model = typeof input.model === 'string' ? input.model : ''
    const { text } = await judgeRun($, [change, model].filter(Boolean).join(' '))
    return { result: text }
  })

  on('tool.call', async ($, e, next) => {
    if (!CHECKED.test(e.tool)) return next(e)
    const { tool, tool_use_id, agentId, consent, ...args } = e as Record<string, unknown>
    const out = await hook($, 'pre-tool-use', {
      tool_name: tool,
      tool_input: args,
      session_id: sessionId,
      agent_id: typeof agentId === 'string' ? agentId : undefined,
      tool_use_id,
    })
    const decision = out?.hookSpecificOutput
    if (decision?.permissionDecision === 'deny') {
      return { deny: String(decision.permissionDecisionReason) }
    }
    // After repeated denials rvow asks the user once; no answer leaves the call denied.
    if (decision?.permissionDecision === 'ask') {
      const reason = String(decision.permissionDecisionReason)
      const answer = await $.ui.ask(reason, ['Allow', 'Deny']).catch(() => 'Deny')
      if (answer !== 'Allow') return { deny: reason }
    }
    // After the call runs or fails, report its outcome so rvow records the
    // blobs the call changed. A denied call never ran, so it is not reported.
    let result: Awaited<ReturnType<typeof next>>
    let post: HookOutput | undefined
    try {
      result = await next(e)
    } finally {
      post = await hook($, 'post-tool-use', {
        session_id: sessionId,
        agent_id: typeof agentId === 'string' ? agentId : undefined,
        tool_use_id,
      })
      if (tool === 'Bash' && /\brvow\b/.test(String(args.command ?? ''))) await refresh($)
    }
    // A notice that rvow kept for this call goes to the model after the tool's result.
    const notice = post?.hookSpecificOutput?.additionalContext
    if (typeof notice !== 'string' || !notice || result.deny !== undefined) return result
    return { ...result, context: [...(result.context ?? []), notice] }
  })

  // A subagent whose prompt starts with the `lease:` line of `rvow delegate` is bound to that lease, so
  // the hook can judge its edits by the leased task. Other prompts never start rvow.
  on('agent.spawn', async ($, e, next) => {
    const started = await next(e)
    if (started.agentId && /^\s*lease:\s*TSK-/m.test(e.prompt)) {
      await hook($, 'lease-bind', { agent_id: started.agentId, prompt: e.prompt })
    }
    return started
  })

  on('classic.SubagentStop', async ($, e, next) => {
    const result = await next(e)
    await hook($, 'lease-end', { agent_id: e.agent_id })
    return result
  })

  // The brief goes with each prompt whose brief changed, not with session start: startup never waits
  // for rvow, and the model learns of an accept, a confirm or an archive the user ran.
  on('classic.UserPromptSubmit', async ($, e, next) => {
    sessionId = e.session_id
    const result = await next(e)
    const out = await Promise.race([hook($, 'session-start', {}), $.clock.sleep(BRIEF_WAIT_MS)])
    const text = out?.hookSpecificOutput?.additionalContext
    if (typeof text !== 'string' || !text || text === sentBrief) return result
    sentBrief = text
    return { ...result, additionalContext: [...(result.additionalContext ?? []), text] }
  })

  // Claude Code caps settings Stop hooks at 8 blocks in a row; this keeps the
  // same cap for the mod's own block.
  on('classic.Stop', async ($, e, next) => {
    const result = await next(e)
    // The CLI gives each reason once per session and lets subagents stop, so it needs these fields.
    const who = {
      session_id: e.session_id,
      agent_id: e.agent_id,
      stop_hook_active: e.stop_hook_active,
    }
    const reason = (await hook($, 'stop', who))?.reason
    if (typeof reason !== 'string' || !reason || blocks >= MAX_BLOCKS) {
      blocks = 0
      return result
    }
    blocks += 1
    return { ...result, block: reason }
  })

  on('turn.complete', async ($, e, next) => {
    const done = await next(e)
    judgePending.deliver(e.agentId, e.answer)
    await refresh($)
    return done
  })

  on('command.run', { command: 'rvow-confirm' }, async ($, e) => {
    const [verb, ...rest] = e.args.trim().split(/\s+/).filter(Boolean)
    if (verb === 'accept' || verb === 'reject') {
      return userOnly($, e.origin, 'rvow-confirm', ['confirm', `--${verb}`, ...rest])
    }
    if (verb !== undefined && verb !== 'all') {
      return userOnly($, e.origin, 'rvow-confirm', ['confirm', verb, ...rest])
    }
    return confirmDialog($, e.origin)
  })

  on('command.run', { command: 'rvow-accept' }, async ($, e) => {
    const id = e.args.trim().split(/\s+/)[0]
    if (!id) {
      const ps = await waiting($)
      const [p] = ps
      if (!p || ps.length > 1) return { text: list(ps) }
      return {
        text: `one plan waits: ${p.id} ${p.title}; read it with /rvow-review, then run /rvow-accept ${p.id}`,
      }
    }
    return userOnly($, e.origin, 'rvow-accept', ['accept', id])
  })

  on('command.run', { command: 'rvow-review' }, async ($, e) => {
    let typed = e.args.trim().split(/\s+/)[0]
    if (!typed) {
      const ps = await waiting($)
      const [p] = ps
      if (!p || ps.length > 1) return { text: list(ps) }
      typed = p.id
    }
    await refresh($)
    const p = (await read($, viewAtom))?.backlog.find((x) => x.here && x.id.startsWith(typed))
    if (!p) return { text: `${typed} is not a backlog plan on this branch; see /rvow-status` }
    await setDetails($, () => [])
    await update($, tabAtom, () => 'backlog')
    await $.store.set('tab', 'backlog').catch(() => {})
    await openDetail($, { kind: 'plan', id: p.id, title: p.title })
    await putReview($, (m) => ({ ...m, [p.id]: true }))
    await $.ui.open({ ...OPEN, focus: true, closeOnEscape: true })
    return {}
  })

  on('command.run', { command: 'rvow-yolo' }, async ($, e) => {
    const [tier, length] = e.args.trim().split(/\s+/).filter(Boolean)
    const argv = yoloArgv({ ...(tier ? { tier } : {}), ...(length ? { for: length } : {}) })
    if (typeof argv === 'string') return { text: `${argv}; try /rvow-yolo auto 30m` }
    return userOnly($, e.origin, 'rvow-yolo', argv)
  })

  on('command.run', { command: 'rvow-archive' }, async ($, e) => {
    const id = e.args.trim()
    if (!id) return { text: 'usage: /rvow-archive <CHG-id>' }
    return userOnly($, e.origin, 'rvow-archive', ['archive', id])
  })

  on('command.run', { command: 'rvow-judge' }, async ($, e) => {
    if (e.origin.kind !== 'composer')
      return { text: '/rvow-judge runs only when you type it yourself.' }
    return judgeRun($, e.args)
  })

  on('command.run', { command: 'rvow-status' }, async ($) => {
    const saved = await $.store.get('tab').catch(() => undefined)
    await update($, tabAtom, () => (TABS.includes(saved as RvowTab) ? (saved as RvowTab) : null))
    await update($, detailsAtom, () => [])
    await $.ui.open(OPEN)
    await refresh($)
    if (await read($, viewAtom)) await restore($)
    else {
      await update($, noticeAtom, () => ({
        tone: 'fail',
        text: 'rvow did not answer; run /rvow-status again',
      }))
    }
    return {}
  })

  on('ui.render', { component: 'Pane', requestId: PANE }, async ($, e) => {
    const { Box, Text, Button } = $.ui.resolve(e)
    const view = await read($, viewAtom)
    const review = await read($, reviewAtom)
    const listing = await read($, amendmentsAtom)
    const notice = await read($, noticeAtom)
    const cols = Math.max(20, e.props.bodyColumns)
    const home = await $.env.get('HOME')
    const selected = await read($, selectedAtom)
    const details = await read($, detailsAtom)

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
    const setReview = (id: string, open: boolean) => putReview($, (m) => ({ ...m, [id]: open }))

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

    if (!view && !notice) return <Text dimColor>loading rvow...</Text>
    if (!view) {
      return (
        <Box flexDirection="column">
          {noticeBox}
          {empty('No rvow change on this branch.', startHint)}
        </Box>
      )
    }

    const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`

    const row = (...kids: RenderChildren[]) => (
      <Box columnGap={3} flexWrap="wrap" marginTop={1}>
        {kids}
      </Box>
    )

    /** The plan's actions: on its card, and under its detail view without `d`. */
    const planBody = (p: RvowPlan, inDetail: boolean) => {
      const why = p.ready && p.here ? null : blocker(p)
      const picked = selected.includes(p.id)
      return review[p.id] ? (
        <Box flexDirection="column" marginTop={1}>
          {REVIEW.map((line, n) => (
            <Text bold={n === 0}>{line}</Text>
          ))}
          {row(
            button(
              `yes:${p.id}`,
              'Accept',
              hot('y'),
              async () => {
                await setReview(p.id, false)
                await act($, ['accept', p.id, '--yes'])
              },
              true,
            ),
            button(`no:${p.id}`, 'Cancel', hot('n'), () => setReview(p.id, false)),
          )}
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
          {row(
            button(
              `accept:${p.id}`,
              'Review & accept',
              hot('a'),
              () =>
                why
                  ? update($, noticeAtom, () => ({ tone: 'fail', text: `Not ready: ${why}.` }))
                  : setReview(p.id, true),
              !why,
            ),
            !p.here &&
              button(
                `claim:${p.id}`,
                'Take to this branch',
                hot('t'),
                () => act($, ['claim', p.id]),
                true,
              ),
            p.here &&
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
              ),
          )}
          {why && (
            <Text dimColor wrap="truncate-end">
              to accept: {why}
            </Text>
          )}
          {row(
            !inDetail &&
              button(`details:${p.id}`, 'Details', hot('d'), () =>
                openDetail($, { kind: 'plan', id: p.id, title: p.title }),
              ),
            button(`refine:${p.id}`, 'Refine with agent', hot('e'), () => refine($, [p])),
            button(`select:${p.id}`, picked ? 'Unselect' : 'Select', hot('s'), () =>
              update($, selectedAtom, (ids) =>
                ids.includes(p.id) ? ids.filter((x) => x !== p.id) : [...ids, p.id],
              ),
            ),
            button(`refresh:${p.id}`, 'Refresh', hot('r'), () => refresh($)),
          )}
        </Box>
      )
    }

    const planCard = (p: RvowPlan) => {
      const picked = selected.includes(p.id)
      const files = home && p.path.startsWith(`${home}/`) ? `~${p.path.slice(home.length)}` : p.path
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
          <Text>
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
          <Text dimColor wrap="truncate-middle">
            files: {files}
          </Text>
          {planBody(p, false)}
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
              const reasons = t.reasons.length > 0 ? ` (${t.reasons.join(', ')})` : ''
              return (
                <Box key={`task-line:${t.id}`} gap={1}>
                  <Text color={color} dimColor={!color}>
                    {mark}
                  </Text>
                  <Button
                    key={`task:${t.id}`}
                    plain
                    label={cut(`${t.id} ${t.title}`, cols - 6 - reasons.length)}
                    onPress={() => openDetail($, { kind: 'task', id: t.id, title: t.title })}
                  />
                  {reasons && (
                    <Text dimColor wrap="truncate-end">
                      {reasons}
                    </Text>
                  )}
                </Box>
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
              {c.pending.length > 0 && row(...amendButtons)}
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
          {row(
            ...(c.tasks.length > 0 &&
            c.tasks.every((t) => t.state === 'proved') &&
            c.pending.length === 0
              ? [
                  button(
                    `archive:${c.id}`,
                    'Archive',
                    hot('x'),
                    () => act($, ['archive', c.id]),
                    true,
                  ),
                ]
              : []),
            button(`details:${c.id}`, 'Details', hot('d'), () =>
              openDetail($, { kind: 'change', id: c.id, title: c.title }),
            ),
          )}
        </Box>
      )
    }

    const ids = (text: string) =>
      text
        .split(/(\b(?:CHG|TSK|REQ|AMD)-[a-z0-9]+\b)/)
        .map((part, i) => (i % 2 === 1 ? <Text dimColor>{part}</Text> : part))
    /** One line of `rvow show --human`: headings bold, keys and ids dim, a verify command all dim. */
    const styled = (line: string) => {
      const heading = /^#+ (.*)$/.exec(line)?.[1] ?? (/^\S[^:]*:$/.test(line) ? line : null)
      if (heading !== null) return <Text bold>{ids(heading)}</Text>
      const kv = /^(\s*)([a-z][a-z ]*):( .*)?$/.exec(line)
      if (kv?.[2] === 'verify') return <Text dimColor>{line}</Text>
      return (
        <Text>{kv ? [kv[1], <Text dimColor>{kv[2]}:</Text>, ...ids(kv[3] ?? '')] : ids(line)}</Text>
      )
    }
    const detailView = (d: RvowDetail) => {
      const [badge, color] = BADGE[d.kind]
      const plan = d.kind === 'plan' ? view.backlog.find((p) => p.id === d.id) : undefined
      const states = view.changes.find((c) => c.id === d.id)?.tasks ?? []
      // The header already shows the id and the title.
      const lines = (d.text?.split('\n') ?? []).filter(
        (l) => l !== d.id && l !== `title: ${d.title}`,
      )
      const shown = lines.slice(0, MAX_LINES)
      const taskLine = (line: string) => {
        const m = d.kind === 'change' ? /^\s+(TSK-[a-z0-9]+) (.*)$/.exec(line) : null
        if (!m) return styled(line)
        const [, id = '', title = ''] = m
        const [mark, mc] = MARKS[states.find((t) => t.id === id)?.state ?? 'missing'] ?? [
          '?',
          undefined,
        ]
        return (
          <Box key={`task-line:${id}`} gap={1} paddingLeft={2}>
            <Text color={mc} dimColor={!mc}>
              {mark}
            </Text>
            <Button
              key={`task:${id}`}
              plain
              label={cut(`${id} ${title}`, cols - 4)}
              onPress={() => openDetail($, { kind: 'task', id, title })}
            />
          </Box>
        )
      }
      // An open review shows its Accept and Cancel before the text, so no scrolling.
      const actions = plan && (
        <Box flexDirection="column" marginTop={1}>
          {planBody(plan, true)}
        </Box>
      )
      const reviewing = plan && review[plan.id]
      return (
        <Box flexDirection="column">
          {button('back', 'Back', hot('b'), () => setDetails($, (s) => s.slice(0, -1)))}
          <Box justifyContent="space-between" gap={1} marginTop={1}>
            <Text wrap="truncate-end">
              <Text inverse bold color={color}>
                {badge}
              </Text>{' '}
              <Text bold>{d.title}</Text>
            </Text>
            <Text dimColor>{d.id}</Text>
          </Box>
          <Box marginBottom={1}>
            <Text dimColor>{RULE.repeat(cols)}</Text>
          </Box>
          {reviewing && actions}
          {d.text === null ? (
            <Text color="yellow" wrap="truncate-end">
              … loading rvow show {d.id}
            </Text>
          ) : (
            shown.map(taskLine)
          )}
          {lines.length > shown.length && (
            <Text dimColor wrap="truncate-end">
              … {lines.length - shown.length} more lines cut; run rvow show {d.id}
            </Text>
          )}
          {d.kind === 'change' && d.text !== null && (
            <Box marginTop={1}>
              <Text dimColor wrap="truncate-end">
                Press a task to read its requirements.
              </Text>
            </Box>
          )}
          {!reviewing && actions}
        </Box>
      )
    }

    const detail = details.at(-1)
    const tab = (await read($, tabAtom)) ?? firstTab(view)
    const picks = view.backlog.filter((p) => selected.includes(p.id))
    const tabs = [
      ['backlog', `Backlog ${view.backlog.length}`],
      ['active', `Active ${view.changes.length}`],
      ['archive', `Archive ${view.archive.length}`],
    ] as const
    // Each tab draws as `[ label ]`, one cell apart.
    const tabsWidth = tabs.reduce((n, [, label]) => n + label.length + 5, -1)
    const branch = view.branch ? `${BRANCH} ${view.branch}` : ''
    const tabBar = (
      <Box flexDirection="column" marginBottom={1}>
        <Box justifyContent="space-between">
          <Box columnGap={1} flexWrap="wrap">
            {tabs.map(([id, label], n) => (
              <Button
                key={`tab:${id}`}
                label={label}
                hotkey={String(n + 1)}
                variant={id === tab ? 'primary' : undefined}
                onPress={async () => {
                  await setDetails($, () => [])
                  await update($, tabAtom, () => id)
                  await $.store.set('tab', id).catch(() => {})
                }}
              />
            ))}
          </Box>
          {branch && tabsWidth + 2 + branch.length <= cols && <Text dimColor>{branch}</Text>}
        </Box>
        <Text dimColor>{RULE.repeat(cols)}</Text>
      </Box>
    )
    // Only what is drawn takes hotkeys, so the tab's cards are built only without a detail.
    const body = detail
      ? detailView(detail)
      : tab === 'backlog'
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
            ? [
                ...view.archive.map((a) => {
                  const [mark, color] = OUTCOME[a.outcome] ?? ['?', undefined]
                  return (
                    <Box key={`archive:${a.id}`} gap={1}>
                      <Text color={color}>{mark}</Text>
                      <Text dimColor>
                        {a.date} {a.id}
                      </Text>
                      <Button
                        key={`open:${a.id}`}
                        plain
                        label={cut(a.title, cols - a.date.length - a.id.length - 4)}
                        onPress={() => openDetail($, { kind: 'archive', id: a.id, title: a.title })}
                      />
                    </Box>
                  )
                }),
                <Box key="archive-hint" marginTop={1}>
                  <Text dimColor wrap="truncate-end">
                    Press an entry to read it.
                  </Text>
                </Box>,
              ]
            : empty('Nothing archived yet.')

    return (
      <Box flexDirection="column">
        {tabBar}
        {noticeBox}
        <Box flexDirection="column" gap={tab === 'archive' || detail ? 0 : 1}>
          {body}
        </Box>
      </Box>
    )
  })

  // The row shows the head of a long rvow output; the model reads the stored text in full.
  on('ui.render', { component: 'CommandOutput' }, async ($, e, next) => {
    const lines = e.props.text.split('\n')
    if (!e.props.command.startsWith('rvow-') || lines.length <= FOLD_LINES) return next(e)
    const { Box, Text } = $.ui.resolve(e)
    return (
      <Box flexDirection="column">
        <Text>{lines.slice(0, FOLD_LINES).join('\n')}</Text>
        <Text
          dimColor
        >{`... ${lines.length - FOLD_LINES} more lines; the model has them all`}</Text>
      </Box>
    )
  })

  on('ui.render', { component: 'AbovePrompt' }, async ($, e, next) => {
    const view = await read($, viewAtom)
    const n = view?.needs_you
    const ready = view?.backlog.find((p) => p.here === true && p.ready === true)
    const parts = n
      ? [
          n.plans > 0 &&
            `${n.plans} plan${n.plans === 1 ? '' : 's'} to accept${ready ? `: /rvow-accept ${ready.id}` : ''}`,
          n.amendments > 0 &&
            `${n.amendments} amendment${n.amendments === 1 ? '' : 's'}: /rvow-confirm`,
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
