export type RvowTask = { id: string; title: string; state: string; reasons: string[] }
export type RvowChange = {
  id: string
  title: string
  stage: string
  tasks: RvowTask[]
  drift: { class: string; what: string }[]
  pending: { id: string; reason: string }[]
}
export type RvowView = {
  branch: string | null
  changes: RvowChange[]
  plans: { id: string; title: string }[]
  questions: { change: string; question: string; recommended: string | null }[]
  next: { change: string; task: string; title: string; verify: string; state: string } | null
  needs_you: { amendments: number; questions: number; blocked: number; plans: number }
}

declare module 'claude-code' {
  interface PluginState {
    rvow: { view: RvowView | null }
  }
}
