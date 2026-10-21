export type Task = { id: string; title: string; state: string; reasons?: string[] };
export type Change = {
  id: string;
  title: string;
  tasks?: Task[];
  drift?: { class: string; what: string }[];
  pending?: { id: string; reason: string; items?: { class: string; what: string }[] }[];
};
export type Plan = {
  id: string;
  title: string;
  branch?: string;
  here?: boolean;
  ready?: boolean;
  tasks?: number;
  questions?: number;
  issues?: number;
  path?: string;
  /** The worktree that has `branch` checked out, or null when none has. */
  worktree?: string | null;
  /** Each task's timeout in seconds, 0 when it uses the configured default. */
  task_timeouts_secs?: number[];
  /** The configured default timeout in seconds. */
  default_timeout_secs?: number;
};
export type Question = { change: string; question: string; recommended?: string | null };
export type Archived = { id: string; title: string; outcome: string; date?: string };
export type Next = { change?: string; task: string; title: string; verify?: string };
export type NeedsYou = { amendments: number; questions: number; blocked: number; plans: number };
export type View = {
  branch: string | null;
  changes: Change[];
  /** Active changes of other branches that have a worktree. */
  elsewhere?: Change[];
  backlog: Plan[];
  archive: Archived[];
  questions: Question[];
  next: Next | null;
  needs_you: NeedsYou;
};
export type ToolOutput =
  | { kind: "view" | "status"; code: number; data: View | null }
  | { kind: "artifact-text"; code: number; id: string; text: string }
  | {
      kind: "proof";
      code: number;
      task: string;
      result: string;
      tests: number | null;
      duration_ms: number;
      event: string;
    }
  | { kind: "yolo"; code: number; tier: string; minutes?: number }
  | { kind: "decision"; code: number; text: string }
  | { kind: "failure"; code: number; error: string; stdout: string; cliCode?: string };

export type HookOutput = Partial<Omit<View, "changes">> & {
  bound?: unknown;
  changes?: Change[];
  plans?: Plan[];
  status?: unknown;
  reason?: unknown;
  hookSpecificOutput?: {
    permissionDecision?: unknown;
    permissionDecisionReason?: unknown;
    additionalContext?: unknown;
  };
};
