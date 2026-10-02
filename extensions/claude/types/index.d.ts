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
}
export type RvowView = {
  branch: string | null
  changes: RvowChange[]
  plans: RvowPlan[]
  questions: { change: string; question: string; recommended: string | null }[]
  next: { change: string; task: string; title: string; verify: string; state: string } | null
  needs_you: { amendments: number; questions: number; blocked: number; plans: number }
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
    }
  }
}
