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
