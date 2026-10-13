/** The accept budget of one plan, read from the hook view rvow prints.
 *  `timeouts` holds each task's `timeout:` in seconds, 0 when the task uses
 *  the configured default. Pure code only: every function that takes `$`
 *  stays in register.tsx. */

export type PlanBudget = {
  /** The number of baseline checks accept will run. */
  tasks: number
  /** Each task's timeout in seconds; 0 means the configured default. */
  timeouts: number[]
  /** The configured default timeout in seconds. */
  defaultSecs: number
}

type ViewPlan = {
  id?: unknown
  tasks?: unknown
  task_timeouts_secs?: unknown
  default_timeout_secs?: unknown
}

/** Finds one plan's baseline count and timeout budget in the hook view JSON. */
export function planBudget(view: unknown, change: string): PlanBudget | null {
  if (!view || typeof view !== 'object') return null
  const { plans, backlog } = view as { plans?: unknown; backlog?: unknown }
  const entries = Array.isArray(plans) ? plans : Array.isArray(backlog) ? backlog : []
  const plan = entries.find(
    (p): p is ViewPlan => !!p && typeof p === 'object' && (p as ViewPlan).id === change,
  )
  if (!plan) return null
  const timeouts = Array.isArray(plan.task_timeouts_secs)
    ? plan.task_timeouts_secs.filter((n): n is number => typeof n === 'number' && n >= 0)
    : []
  const defaultSecs = typeof plan.default_timeout_secs === 'number' ? plan.default_timeout_secs : 0
  const tasks = typeof plan.tasks === 'number' ? plan.tasks : timeouts.length
  return { tasks, timeouts, defaultSecs }
}

/** The most checklist lines the accept dialog shows before it cuts the rest. */
export const ACCEPT_DIALOG_LINES = 10

/** The dialog's share of `review`: the first lines in full, or the first
 *  10 lines and one line that names `rvow review <id>` and the cut count. */
export function shortReview(review: string, change: string): string {
  const lines = review.split('\n')
  if (lines.at(-1) === '') lines.pop()
  if (lines.length <= ACCEPT_DIALOG_LINES) return review
  const cut = lines.length - ACCEPT_DIALOG_LINES
  return [
    ...lines.slice(0, ACCEPT_DIALOG_LINES),
    `... ${cut} more lines: run \`rvow review ${change}\``,
  ].join('\n')
}

/** What the agent hears when the user does not accept: the plan stays, and the slash command starts it. */
export function declinedAcceptReason(change: string): string {
  return `The user did not accept ${change}; it stays a plan.`
}

/** A repeat accept while the plan is unchanged: the old reason, and the slash command the user can run. */
export function repeatAcceptResult(reason: string, change: string): string {
  return `${reason} The user can run \`/rvow-accept ${change}\` to start it.`
}

/** The notice shown before accept starts, so the user knows what will run. */
export function baselineNotice(count: number): string {
  return `rvow accept: ${count} baseline checks; this can take a while`
}

/** The accept timeout: the sum of the plan's task timeouts, each task without
 *  one counting the configured default, and never below the fixed wait. */
export function acceptTimeoutMs(budget: PlanBudget | null, floorMs = 600_000): number {
  if (!budget || budget.tasks <= 0) return floorMs
  let total = 0
  for (let i = 0; i < budget.tasks; i++) {
    const secs = budget.timeouts[i] ?? 0
    total += secs > 0 ? secs : budget.defaultSecs
  }
  return Math.max(floorMs, total * 1000)
}
