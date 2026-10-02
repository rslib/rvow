export type RvowTask = { id: string; title: string; state: string; reasons: string[] }
export type RvowChange = {
  id: string
  title: string
  stage: string
  tasks: RvowTask[]
  drift: { class: string; what: string }[]
  pending: { id: string; reason: string }[]
}
export type RvowPlan = {
  id: string
  title: string
  tasks: number
  questions: number
  issues: number
  ready: boolean
  /** The branch the plan is for. */
  branch: string
  /** True when `branch` is the current branch. */
  here: boolean
  /** The plan's folder. */
  path: string
}
export type RvowArchived = {
  id: string
  title: string
  outcome: 'done' | 'dropped' | 'superseded'
  /** YYYY-MM-DD */
  date: string
}
export type RvowView = {
  branch: string | null
  changes: RvowChange[]
  /** The plans on this branch. */
  plans: RvowPlan[]
  /** Every backlog plan of every branch, this branch first. */
  backlog: RvowPlan[]
  /** Newest first, at most 10. */
  archive: RvowArchived[]
  questions: { change: string; question: string; recommended: string | null }[]
  next: { change: string; task: string; title: string; verify: string; state: string } | null
  needs_you: { amendments: number; questions: number; blocked: number; plans: number }
}
export type RvowTab = 'backlog' | 'active' | 'archive'
/** A drill-down view drawn over the selected tab. */
export type RvowDetail = {
  kind: 'plan' | 'change' | 'task' | 'archive'
  id: string
  title: string
  /** `rvow show <id> --human`, plus a task's covered requirements; null while it runs. */
  text: string | null
}
/** The last thing a pane button ran, drawn under the header. */
export type RvowNotice = { tone: 'ok' | 'fail' | 'busy'; text: string }

declare module 'claude-code' {
  interface PluginState {
    rvow: {
      view: RvowView | null
      /** Plan ids whose card shows the review checklist. */
      review: Record<string, boolean>
      /** The `rvow confirm` listing while the amendments are under review. */
      amendments: string | null
      notice: RvowNotice | null
      /** The pane's selected tab; null picks it from the view. */
      tab: RvowTab | null
      /** Backlog plan ids picked for one refine prompt. */
      selected: string[]
      /** Open detail views, the drawn one last; empty draws the tab. */
      details: RvowDetail[]
    }
  }
}
