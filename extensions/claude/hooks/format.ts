import type { RvowDetail, RvowPlan, RvowTab, RvowView } from '../types'

export const ACCENT = 'cyan'
export const RULE = '─'
export const BRANCH = '⎇'
export const BADGE: Record<RvowDetail['kind'], [string, string | undefined]> = {
  plan: [' PLAN ', 'yellow'],
  change: [' CHANGE ', ACCENT],
  task: [' TASK ', ACCENT],
  archive: [' ARCHIVED ', undefined],
}
export const ID = /\b(?:CHG|TSK|REQ|AMD)-[a-z0-9]+\b/g
// The pane body scrolls; this cap only keeps a huge `rvow show` from drawing thousands of rows.
export const MAX_LINES = 200
// The checklist `rvow accept` prints (crates/rvow/src/main.rs REVIEW).
export const REVIEW = [
  'before you accept, check:',
  '  1. Does each requirement say what the system does, not how?',
  "  2. Does each task's check fail today for the reason the task fixes?",
  '  3. Is anything in Out of scope that you need now?',
  '  4. Is every Assumption one you agree with?',
  '  5. Is the change small enough to review its diff in one sitting?',
]
// Drawing only: the state names and drift classes are rvow's own.
export const MARKS: Record<string, [string, string | undefined]> = {
  proved: ['✓', 'green'],
  failed: ['✗', 'red'],
  stale: ['~', 'yellow'],
  flaky: ['≈', 'yellow'],
  blocked: ['⊘', 'magenta'],
  missing: ['·', undefined],
}
export const DRIFT: Record<string, string | undefined> = { weaken: 'red', confirm: 'yellow' }
export const OUTCOME = {
  done: ['✓', 'green'],
  dropped: ['✗', 'red'],
  superseded: ['↷', 'yellow'],
} as const

export const firstTab = (v: RvowView): RvowTab =>
  v.changes.length > 0 ? 'active' : v.backlog.length > 0 ? 'backlog' : 'archive'

/** Why a plan cannot be accepted here, in the order the author should fix it. */
export const blocker = (p: RvowPlan) =>
  !p.here
    ? 'take it to this branch first'
    : p.questions > 0
      ? 'answer the open questions first'
      : p.issues > 0
        ? 'fix what rvow check reports'
        : 'add at least one task with a verify command'

/** `text` on one line, cut to `width` characters with `...`. */
export const cut = (text: string, width: number) => {
  const line = text.replace(/\s+/g, ' ').trim()
  return line.length <= width ? line : `${line.slice(0, Math.max(0, width - 3))}...`
}

/** What the agent hears after the person decided, or null when argv decided nothing. */
export function decisionPrompt(argv: string[], out: string): string | null {
  const [verb, id] = argv
  if (verb === 'archive') return `I archived ${id}. The change is closed.`
  if (verb === 'accept') {
    const next = out.split('\n').find((l) => l.startsWith('next:'))
    const trimmed = next?.endsWith('.') ? next.slice(0, -1) : next
    return `I accepted ${id}.${trimmed ? ` ${trimmed}.` : ''} Continue with it.`
  }
  if (verb === 'yolo') {
    if (id === 'off') return 'I switched yolo off. The usual rules apply again.'
    if (id && id in YOLO_TIERS) {
      return `I switched on yolo ${id}: ${YOLO_TIERS[id as keyof typeof YOLO_TIERS]}. It ends by itself, and the gate still checks at push.`
    }
    return null
  }
  const ids = [...new Set(`${argv.join(' ')}\n${out}`.match(/AMD-[\w-]+/g) ?? [])].join(', ')
  if (argv.includes('--accept')) return `I accepted the plan edit ${ids}. Continue the tasks.`
  if (argv.includes('--reject'))
    return `I rejected the plan edit ${ids}. Undo it, or amend it again with a different edit.`
  return null
}

/** A command that only the user runs and that decides something, so a failure of it must reach the agent. */
export function isDecision(argv: string[]): boolean {
  const [verb] = argv
  return (
    verb === 'accept' ||
    verb === 'archive' ||
    (verb === 'confirm' && (argv.includes('--accept') || argv.includes('--reject')))
  )
}
const FAILED_LINES = 12

/** What the agent is told when a decision the user made could not be carried out. */
export function failedPrompt(argv: string[], out: string): string {
  const lines = out.split('\n').filter((l) => l.trim())
  return [
    `I tried \`rvow ${argv.join(' ')}\` and it failed:`,
    ...lines.slice(0, FAILED_LINES),
    'Fix what it names, then ask me to run it again.',
  ].join('\n')
}

export const REFINE_STEPS =
  'Follow `rvow guide proposal`, `rvow guide spec` and `rvow guide tasks`. Ask me about open points with the rvow_ask tool before changing scope. Then cut anything the guides rule out of the proposal and tasks so `rvow check` passes, and tell me what changed.'

/** The prompt that asks the agent to refine one plan, or several one at a time. */
export function refinePrompt(plans: RvowPlan[]): string {
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

export const ASK = 'mcp__rvow__rvow_ask'
export const YOLO = 'mcp__rvow__rvow_yolo'
export const ACCEPT = 'mcp__rvow__rvow_accept'
export const CONFIRM = 'mcp__rvow__rvow_confirm'

export const ACCEPT_DESCRIPTION =
  'Ask the user to accept a ready rvow plan. The user reads the review text and the checklist in a dialog and answers; the plan is accepted only when they say yes. The result tells you what happened, and when it failed it says why. Use this instead of asking the user to type /rvow-accept, and run `rvow check` first so the plan is ready.'
export const CONFIRM_DESCRIPTION =
  'Show the pending rvow plan edits (amendments), or ask the user to accept or reject all of them. With no decision it only lists them. With a decision the user reads the list in a dialog and answers; nothing changes unless they say yes. The result tells you what happened, and when it failed it says why.'
export const ACCEPT_SCHEMA = {
  type: 'object',
  required: ['change'],
  properties: { change: { type: 'string', pattern: '^CHG-[a-z0-9]+$' } },
  additionalProperties: false,
}
export const CONFIRM_SCHEMA = {
  type: 'object',
  properties: { decision: { type: 'string', enum: ['accept', 'reject'] } },
  additionalProperties: false,
}

/** What a failed command tells the agent inside a tool result: the command and its own words. */
export function failedText(argv: string[], out: string): string {
  return `rvow ${argv.join(' ')} failed:\n${out
    .split('\n')
    .filter((l) => l.trim())
    .slice(0, 12)
    .join('\n')}`
}

/** What each yolo tier changes, as the user reads it in the dialog and the model reads it in the tool text. */
export const YOLO_TIERS = {
  audit: 'edits go through before the plan is accepted or outside a task, and are only counted',
  auto: 'also lets the agent run rvow accept and rvow confirm',
  bypass: 'every hook only audits and a stop is never held back',
} as const

export const YOLO_DESCRIPTION = [
  "Show or change rvow's yolo mode, which relaxes the hooks for a while. Call it with no tier to see what is on.",
  'A tier needs the user to allow it in a dialog, and the user may refuse.',
  `audit: ${YOLO_TIERS.audit}.`,
  `auto: ${YOLO_TIERS.auto}.`,
  `bypass: ${YOLO_TIERS.bypass}.`,
  'off ends it at once and needs no dialog. A tier ends by itself after 2 hours, or after `for` (such as 30m or 4h, at most 24h).',
  'The gate still checks everything at push, and tampering with the ledger, the enforcement files or the hidden checks stays denied below bypass.',
  'Do not use it to get around one denial; run rvow block with a reason instead.',
].join(' ')

export const YOLO_SCHEMA = {
  type: 'object',
  properties: {
    tier: { type: 'string', enum: ['audit', 'auto', 'bypass', 'off'] },
    for: { type: 'string', pattern: '^[1-9][0-9]*[smh]$' },
  },
  additionalProperties: false,
}

/** The `rvow yolo` arguments for a tool call, or the reason that the call is refused. */
export function yoloArgv(input: Record<string, unknown>): string[] | string {
  const unknown = Object.keys(input).filter((k) => k !== 'tier' && k !== 'for')
  if (unknown.length > 0)
    return `rvow: unknown input ${unknown.join(', ')}; only tier and for are allowed`
  const { tier, for: length } = input
  if (tier === undefined) return length === undefined ? ['yolo'] : 'rvow: for needs a tier'
  if (typeof tier !== 'string' || !['audit', 'auto', 'bypass', 'off'].includes(tier)) {
    return 'rvow: tier must be audit, auto, bypass or off'
  }
  if (length === undefined) return ['yolo', tier]
  if (tier === 'off') return 'rvow: off takes no length'
  if (typeof length !== 'string' || !/^[1-9][0-9]*[smh]$/.test(length)) {
    return 'rvow: for must look like 30m or 2h'
  }
  return ['yolo', tier, '--for', length]
}
export const EDIT_HINT = 'Edit the text each answer changes.'

export type Asked = {
  ref: string
  question: string
  header?: string
  options: { label: string; description?: string }[]
}

export const PANE = 'rvow'
// No focus, so the pane never takes the keys from the prompt. No size, so the dock keeps the width the
// diff panel has. No closeOnEscape: Esc hands the keys back and the pane stays; the dock keeps both as tabs.
export const OPEN = { id: PANE, title: 'rvow' } as const
export const TABS = ['backlog', 'active', 'archive']
