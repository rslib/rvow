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
    return `I accepted ${id}.${next ? ` ${next}.` : ''} Continue with it.`
  }
  const ids = [...new Set(`${argv.join(' ')}\n${out}`.match(/AMD-[\w-]+/g) ?? [])].join(', ')
  if (argv.includes('--accept')) return `I accepted the plan edit ${ids}. Continue the tasks.`
  if (argv.includes('--reject'))
    return `I rejected the plan edit ${ids}. Undo it, or amend it again with a different change.`
  return null
}

export const REFINE_STEPS =
  'Follow `rvow guide proposal`, `rvow guide spec` and `rvow guide tasks`. Ask me about open points with the rvow_ask tool before changing scope. Then tighten the proposal and tasks so `rvow check` passes, and tell me what changed.'

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
