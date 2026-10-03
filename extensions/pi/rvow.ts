import { spawn } from "node:child_process";
import type {
  ExtensionAPI,
  ExtensionContext,
  ThemeBg,
  ThemeColor,
  ToolDefinition,
} from "@earendil-works/pi-coding-agent";
import {
  type KeyId,
  matchesKey,
  type TUI,
  type TuiMouseEvent,
  type TuiMouseEventResult,
  truncateToWidth,
  visibleWidth,
  wrapTextWithAnsi,
} from "@earendil-works/pi-tui";

const BIN = process.env.RVOW_BIN || "rvow";
const MAX_CONTINUES = 8;
const FAST_MS = 10_000;
const SLOW_MS = 60_000;

let warned = false;

function run(
  args: string[],
  cwd: string,
  stdin: string,
  timeoutMs: number,
): Promise<{ code: number; out: string } | undefined> {
  return new Promise((resolve) => {
    let out = "";
    let done = false;
    const finish = (v?: { code: number; out: string }) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      resolve(v);
    };
    const fail = () => {
      if (!warned) {
        warned = true;
        console.error(
          `rvow: cannot run '${BIN}'; its checks are off (set RVOW_BIN or put rvow on PATH)`,
        );
      }
      finish();
    };
    const timer = setTimeout(() => {
      child?.kill("SIGKILL");
      fail();
    }, timeoutMs);
    let child: ReturnType<typeof spawn> | undefined;
    try {
      child = spawn(BIN, args, { cwd, stdio: ["pipe", "pipe", "ignore"] });
    } catch {
      return fail();
    }
    child.on("error", fail);
    child.stdout?.on("data", (d: Buffer) => (out += d));
    child.on("close", (code) => finish({ code: code ?? 1, out }));
    child.stdin?.on("error", () => {});
    child.stdin?.end(stdin);
  });
}

type Task = { id: string; title: string; state: string; reasons?: string[] };
type Change = {
  id: string;
  title: string;
  tasks?: Task[];
  drift?: { class: string; what: string }[];
  pending?: { id: string; reason: string }[];
};
type Plan = {
  id: string;
  title: string;
  branch?: string;
  here?: boolean;
  ready?: boolean;
  tasks?: number;
  questions?: number;
  issues?: number;
  path?: string;
};
type Question = { change: string; question: string; recommended?: string | null };
type Archived = { id: string; title: string; outcome: string; date?: string };
type Next = { change?: string; task: string; title: string; verify?: string };
type NeedsYou = { amendments: number; questions: number; blocked: number; plans: number };
type View = {
  branch: string | null;
  changes: Change[];
  backlog: Plan[];
  archive: Archived[];
  questions: Question[];
  next: Next | null;
  needs_you: NeedsYou;
};
type HookOutput = Partial<Omit<View, "changes">> & {
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

async function hook(
  event: string,
  cwd: string,
  extra: object,
  timeoutMs: number,
  emptyMeansNone = false,
): Promise<HookOutput | undefined> {
  const r = await run(["hook", "claude", event], cwd, JSON.stringify({ cwd, ...extra }), timeoutMs);
  if (r?.code !== 0) return undefined;
  // rvow prints nothing for `view` in a project with no change and no archive at all.
  if (!r.out.trim()) return emptyMeansNone ? { changes: [] } : undefined;
  try {
    const parsed: unknown = JSON.parse(r.out);
    return typeof parsed === "object" && parsed !== null && !Array.isArray(parsed)
      ? parsed
      : undefined;
  } catch {
    return undefined;
  }
}

function normalize(out: HookOutput | undefined): View | undefined {
  if (!out || !Array.isArray(out.changes)) return undefined;
  return {
    branch: out.branch ?? null,
    changes: out.changes,
    backlog: out.backlog ?? out.plans ?? [],
    archive: out.archive ?? [],
    questions: out.questions ?? [],
    next: out.next ?? null,
    needs_you: { amendments: 0, questions: 0, blocked: 0, plans: 0, ...out.needs_you },
  };
}

const CLAUDE_NAMES: Record<string, string> = { bash: "Bash", edit: "Edit", write: "Write" };

async function brief(cwd: string): Promise<string | undefined> {
  const text = (await hook("session-start", cwd, {}, SLOW_MS))?.hookSpecificOutput
    ?.additionalContext;
  return typeof text === "string" && text ? text : undefined;
}

type Paint = {
  fg: (color: ThemeColor, text: string) => string;
  bold: (text: string) => string;
  inverse: (text: string) => string;
  bg: (color: ThemeBg, text: string) => string;
};
const PLAIN: Paint = { fg: (_c, t) => t, bold: (t) => t, inverse: (t) => t, bg: (_c, t) => t };

// These strings and glyphs mirror extensions/claude/hooks/format.ts; rvow.test.ts keeps them equal.
const MARKS: Record<string, [string, ThemeColor]> = {
  proved: ["✓", "success"],
  failed: ["✗", "error"],
  stale: ["~", "warning"],
  flaky: ["≈", "warning"],
  blocked: ["⊘", "accent"],
  missing: ["·", "dim"],
};
const OUTCOME: Record<string, [string, ThemeColor]> = {
  done: ["✓", "success"],
  dropped: ["✗", "error"],
  superseded: ["↷", "warning"],
};
const DRIFT_COLOR: Record<string, ThemeColor> = { weaken: "error", confirm: "warning" };
const ID = /\b(?:CHG|TSK|REQ|AMD)-[a-z0-9]+\b/g;
const MAX_LINES = 200;
const MAX_WIDTH = 100;
const PANEL_ROWS = 34;

export const REFINE_STEPS =
  "Follow `rvow guide proposal`, `rvow guide spec` and `rvow guide tasks`. Ask me about open points with the rvow_ask tool before changing scope. Then tighten the proposal and tasks so `rvow check` passes, and tell me what changed.";

export function refinePrompt(plans: { id: string; title: string }[]): string {
  const [p] = plans;
  if (plans.length === 1 && p) {
    return `Review and refine the rvow plan ${p.id} (${p.title}). Run \`rvow show ${p.id}\` and \`rvow check ${p.id}\`. ${REFINE_STEPS}`;
  }
  return [
    "Review and refine these rvow plans, one plan at a time, in this order:",
    ...plans.map((x) => `- ${x.id} (${x.title})`),
    `For each plan: run \`rvow show <id>\` and \`rvow check <id>\`. ${REFINE_STEPS}`,
  ].join("\n");
}

export function decisionPrompt(argv: string[], out: string): string | null {
  const [verb, id] = argv;
  if (verb === "archive") return `I archived ${id}. The change is closed.`;
  if (verb === "accept") {
    const next = out.split("\n").find((l) => l.startsWith("next:"));
    return `I accepted ${id}.${next ? ` ${next}.` : ""} Continue with it.`;
  }
  const ids = [...new Set(`${argv.join(" ")}\n${out}`.match(/AMD-[\w-]+/g) ?? [])].join(", ");
  if (argv.includes("--accept")) return `I accepted the plan edit ${ids}. Continue the tasks.`;
  if (argv.includes("--reject"))
    return `I rejected the plan edit ${ids}. Undo it, or amend it again with a different change.`;
  return null;
}

const later = (work: () => Promise<unknown>) => void work().catch(() => undefined);

function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}

function blocker(p: Plan): string {
  return !p.here
    ? "take it to this branch first"
    : (p.questions ?? 0) > 0
      ? "answer the open questions first"
      : (p.issues ?? 0) > 0
        ? "fix what rvow check reports"
        : "add at least one task with a verify command";
}

function middle(text: string, width: number): string {
  if (text.length <= width) return text;
  const keep = Math.max(2, width - 3);
  return `${text.slice(0, Math.ceil(keep / 2))}...${text.slice(-Math.floor(keep / 2))}`;
}

function cut(text: string, width: number): string {
  const line = text.replace(/\s+/g, " ").trim();
  return line.length <= width ? line : `${line.slice(0, Math.max(0, width - 3))}...`;
}

function band(v: View): string | undefined {
  const n = v.needs_you;
  const ready = v.backlog.find((p) => p.here === true && p.ready === true);
  const parts = [
    n.plans > 0 &&
      `${plural(n.plans, "plan")} to accept${ready ? `: /rvow-accept ${ready.id}` : ""}`,
    n.amendments > 0 && `${plural(n.amendments, "amendment")}: /rvow-confirm`,
    n.questions > 0 && plural(n.questions, "question"),
  ].filter(Boolean);
  return parts.length ? `rvow · needs you: ${parts.join(" · ")}` : undefined;
}

type Tab = "backlog" | "active" | "archive";
type Kind = "plan" | "change" | "task" | "archive";
type Detail = { kind: Kind; id: string; title: string; text: string | null };
type Notice = { tone: "ok" | "fail"; text: string };
type Item =
  | { kind: "plan"; plan: Plan }
  | { kind: "change"; change: Change }
  | { kind: "task"; change: Change; task: Task }
  | { kind: "archive"; entry: Archived };
type Exit =
  | { act: "close" }
  | { act: "accept" | "claim" | "archive" | "ask"; id: string }
  | { act: "amend" }
  | { act: "refine"; plans: Plan[] };
type ScreenHit = { row: number; x0: number; x1: number; key: string; item?: number };
type Hit = { line: number; x0: number; x1: number; key: string; item?: number };
type Body = { lines: string[]; spans: [number, number][]; items: Item[]; hits: Hit[] };

const TABS: Tab[] = ["backlog", "active", "archive"];
const KINDS: Kind[] = ["plan", "change", "task", "archive"];
const BADGE: Record<Kind, [string, ThemeColor]> = {
  plan: [" PLAN ", "warning"],
  change: [" CHANGE ", "accent"],
  task: [" TASK ", "accent"],
  archive: [" ARCHIVED ", "dim"],
};
const TONE: Record<Notice["tone"], [string, ThemeColor]> = {
  ok: ["✓", "success"],
  fail: ["✗", "error"],
};
const ENTRY = "rvow-pane";
const DOCK_ENTRY = "rvow-dock";

const isTab = (v: unknown): v is Tab => TABS.some((t) => t === v);
const isKind = (v: unknown): v is Kind => KINDS.some((k) => k === v);
const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

const fit = (s: string, n: number) => truncateToWidth(s, n, "...", true);
const spread = (left: string, right: string, n: number) =>
  `${truncateToWidth(left, Math.max(0, n - visibleWidth(right) - 1), "...", true)} ${right}`;

function card(rows: string[], w: number, color: ThemeColor, lit: boolean, th: Paint): string[] {
  const edge = (s: string) => th.fg(color, s);
  const gutter = lit ? th.fg("accent", "▌ ") : "  ";
  return [
    `${gutter}${edge(`╭${"─".repeat(w - 4)}╮`)}`,
    ...rows.map((r) => `${gutter}${edge("│")} ${fit(r, w - 6)} ${edge("│")}`),
    `${gutter}${edge(`╰${"─".repeat(w - 4)}╯`)}`,
  ];
}

type Button = { key: string; label: string; primary?: boolean };

function buttons(bs: Button[], n: number, on: boolean, th: Paint): string[] {
  const lines: string[] = [];
  let cur = "";
  let used = 0;
  for (const b of bs) {
    const width = b.key.length + 1 + b.label.length;
    if (cur && used + 3 + width > n) {
      lines.push(cur);
      cur = "";
    }
    used = cur ? used + 3 + width : width;
    const key = th.fg(on ? "accent" : "dim", th.bold(b.key));
    const label = on ? (b.primary ? th.bold(b.label) : b.label) : th.fg("dim", b.label);
    cur += `${cur ? "   " : ""}${key} ${label}`;
  }
  return cur ? [...lines, cur] : lines;
}

/** Where `buttons()` puts each label: line within its output and visible columns. */
function buttonRuns(bs: Button[], n: number): Hit[] {
  const runs: Hit[] = [];
  let line = 0;
  let used = 0;
  for (const b of bs) {
    const width = b.key.length + 1 + b.label.length;
    if (used > 0 && used + 3 + width > n) {
      line++;
      used = 0;
    }
    const x0 = used > 0 ? used + 3 : 0;
    runs.push({ line, x0, x1: x0 + width, key: b.key });
    used = x0 + width;
  }
  return runs;
}

function planButtons(p: Plan, inDetail: boolean, picked: boolean): Button[] {
  const ready = p.ready === true && p.here === true;
  return [
    { key: "a", label: "Review & accept", primary: ready },
    ...(p.here ? [] : [{ key: "t", label: "Take to this branch", primary: true }]),
    ...(p.here && (p.questions ?? 0) > 0
      ? [{ key: "A", label: "Answer questions", primary: true }]
      : []),
    ...(inDetail ? [] : [{ key: "d", label: "Details" }]),
    { key: "e", label: "Refine with agent" },
    { key: "s", label: picked ? "Unselect" : "Select" },
    { key: "r", label: "Refresh" },
  ];
}

const archivable = (c: Change) =>
  (c.tasks ?? []).length > 0 &&
  (c.tasks ?? []).every((t) => t.state === "proved") &&
  (c.pending ?? []).length === 0;

const HOME_DIR = process.env.HOME;

function planCard(
  v: View,
  p: Plan,
  picked: boolean,
  lit: boolean,
  w: number,
  th: Paint,
): { lines: string[]; hits: Hit[] } {
  const inner = w - 6;
  const files =
    HOME_DIR && p.path?.startsWith(`${HOME_DIR}/`) ? `~${p.path.slice(HOME_DIR.length)}` : p.path;
  const why = p.ready && p.here ? null : blocker(p);
  const check =
    (p.issues ?? 0) > 0
      ? th.fg("error", plural(p.issues ?? 0, "issue"))
      : p.ready
        ? th.fg("success", "ready")
        : th.fg("warning", "not ready");
  const rows = [
    spread(
      `${picked ? th.fg("accent", "◉") : th.fg("dim", "○")} ${th.inverse(th.fg("warning", th.bold(" PLAN ")))} ${th.bold(p.title)}`,
      th.fg("dim", p.id),
      inner,
    ),
    p.here ? th.fg("warning", "● waiting for your accept") : th.fg("dim", `on ${p.branch ?? "?"}`),
    `${th.fg("dim", `${plural(p.tasks ?? 0, "task")} · ${plural(p.questions ?? 0, "open question")} · check: `)}${check}`,
    ...(files ? [th.fg("dim", `files: ${middle(files, inner - 7)}`)] : []),
    ...v.questions
      .filter((q) => q.change === p.id)
      .map(
        (q) =>
          `${th.fg("warning", "?")} ${q.question}${q.recommended ? th.fg("dim", ` (recommended: ${q.recommended})`) : ""}`,
      ),
    why
      ? th.fg("dim", `to accept: ${why}`)
      : `${th.fg("accent", "▸")} Review the plan, then accept it.`,
  ];
  const bs = planButtons(p, false, picked);
  const at = rows.length;
  rows.push(...buttons(bs, inner, lit, th));
  const hits = buttonRuns(bs, inner).map((h) => ({
    ...h,
    line: 1 + at + h.line,
    x0: h.x0 + 4,
    x1: h.x1 + 4,
  }));
  return { lines: card(rows, w, "warning", lit, th), hits };
}

function changeCard(
  v: View,
  c: Change,
  focus: number,
  first: number,
  w: number,
  th: Paint,
): { lines: string[]; taskRows: number[]; hits: Hit[] } {
  const inner = w - 6;
  const tasks = c.tasks ?? [];
  const proved = tasks.filter((t) => t.state === "proved").length;
  const label = ` ${proved}/${tasks.length} proved`;
  const bar = Math.max(4, Math.min(32, inner - label.length));
  const fill = tasks.length ? Math.round((bar * proved) / tasks.length) : 0;
  const next = v.next && v.next.change === c.id ? v.next : null;
  const lit = focus >= first && focus <= first + tasks.length;
  const rows = [
    spread(th.bold(c.title), th.fg("dim", c.id), inner),
    `${th.fg("success", "█".repeat(fill))}${th.fg("dim", "░".repeat(bar - fill))}${label}`,
    "",
  ];
  const taskRows: number[] = [];
  for (const [i, t] of tasks.entries()) {
    const [mark, color] = MARKS[t.state] ?? ["?", "dim"];
    const reasons = t.reasons?.length ? ` (${t.reasons.join(", ")})` : "";
    const on = focus === first + 1 + i;
    taskRows.push(rows.length);
    const text = cut(`${t.id} ${t.title}`, inner - 4 - reasons.length);
    rows.push(
      `${on ? th.fg("accent", "›") : " "} ${th.fg(color, mark)} ${on ? th.bold(text) : text}${th.fg("dim", reasons)}`,
    );
  }
  if ((c.drift ?? []).length > 0 || (c.pending ?? []).length > 0) {
    rows.push("", th.fg("warning", th.bold("needs you")));
    for (const d of c.drift ?? [])
      rows.push(`${th.fg(DRIFT_COLOR[d.class] ?? "dim", d.class)} ${d.what}`);
    for (const a of c.pending ?? [])
      rows.push(`${th.fg("warning", "amend")} ${th.fg("dim", a.id)} ${a.reason}`);
  }
  if (next) {
    rows.push(
      "",
      `${th.fg("accent", th.bold("▸ next"))} ${th.fg("dim", next.task)} ${next.title}`,
      th.fg("dim", `$ ${next.verify ?? ""}`),
    );
  }
  const bs: Button[] = [
    ...(archivable(c) ? [{ key: "x", label: "Archive", primary: true }] : []),
    ...((c.pending ?? []).length > 0
      ? [{ key: "c", label: "Review amendments", primary: true }]
      : []),
    { key: "d", label: "Details" },
  ];
  const at = rows.length;
  rows.push(...buttons(bs, inner, lit, th));
  const hits = buttonRuns(bs, inner).map((h) => ({
    ...h,
    line: 1 + at + h.line,
    x0: h.x0 + 4,
    x1: h.x1 + 4,
  }));
  return { lines: card(rows, w, "accent", lit, th), taskRows, hits };
}

function empty(lines: string[], w: number, th: Paint): string[] {
  return card(lines, w, "borderMuted", false, th);
}

const START = (th: Paint) =>
  `${th.fg("dim", "Start a plan: ")}${th.fg("accent", 'rvow new "<title>"')}`;

function tabBody(v: View, tab: Tab, picked: string[], focus: number, w: number, th: Paint): Body {
  const b: Body = { lines: [], spans: [], items: [], hits: [] };
  const push = (
    lines: string[],
    item: Item,
    hits: Hit[] = [],
    from = b.lines.length,
    to = from + lines.length,
  ) => {
    b.lines.push(...lines);
    b.hits.push(...hits.map((h) => ({ ...h, line: from + h.line, item: b.items.length })));
    b.items.push(item);
    b.spans.push([from, to]);
  };
  if (tab === "backlog") {
    if (v.backlog.length === 0) {
      b.lines = empty(["No plan in the backlog.", START(th)], w, th);
      return b;
    }
    const n = v.backlog.filter((p) => picked.includes(p.id)).length;
    if (n > 0) {
      const refine = [{ key: "f", label: `Refine ${n} selected`, primary: true }];
      b.hits.push(...buttonRuns(refine, w));
      b.lines.push(...buttons(refine, w, true, th), "");
    }
    for (const p of v.backlog) {
      const c = planCard(v, p, picked.includes(p.id), focus === b.items.length, w, th);
      push(c.lines, { kind: "plan", plan: p }, c.hits);
    }
  } else if (tab === "active") {
    if (v.changes.length === 0) {
      const there = v.branch ? ` on ${v.branch}` : "";
      b.lines = empty(
        [`Nothing active${there}.`, th.fg("dim", "Accept a plan on the Backlog tab first.")],
        w,
        th,
      );
      return b;
    }
    for (const c of v.changes) {
      const first = b.items.length;
      const start = b.lines.length;
      const card = changeCard(v, c, focus, first, w, th);
      b.lines.push(...card.lines);
      b.hits.push(...card.hits.map((h) => ({ ...h, line: start + h.line, item: first })));
      b.items.push({ kind: "change", change: c });
      b.spans.push([start, start + card.lines.length]);
      for (const [i, t] of (c.tasks ?? []).entries()) {
        const at = start + 1 + (card.taskRows[i] ?? 0);
        b.items.push({ kind: "task", change: c, task: t });
        b.spans.push([at, at + 1]);
      }
    }
  } else {
    if (v.archive.length === 0) {
      b.lines = empty(["Nothing archived yet."], w, th);
      return b;
    }
    for (const a of v.archive) {
      const [mark, color] = OUTCOME[a.outcome] ?? ["?", "dim"];
      const on = focus === b.items.length;
      const head = `${on ? th.fg("accent", "›") : " "} ${th.fg(color, mark)} ${th.fg("dim", `${a.date ?? ""} ${a.id}`)} `;
      push([fit(`${head}${on ? th.bold(a.title) : a.title}`, w)], { kind: "archive", entry: a });
    }
    b.lines.push("", th.fg("dim", "Press enter on an entry to read it."));
  }
  return b;
}

function styled(line: string, th: Paint): string {
  const ids = (s: string) => s.replace(ID, (m) => th.fg("dim", m));
  const heading = /^#+ (.*)$/.exec(line)?.[1] ?? (/^\S[^:]*:$/.test(line) ? line : null);
  if (heading !== null) return th.bold(ids(heading));
  const kv = /^(\s*)([a-z][a-z ]*):( .*)?$/.exec(line);
  if (kv?.[2] === "verify") return th.fg("dim", line);
  return kv ? `${kv[1]}${th.fg("dim", `${kv[2]}:`)}${ids(kv[3] ?? "")}` : ids(line);
}

function detailBody(
  v: View,
  d: Detail,
  picked: boolean,
  focus: number,
  w: number,
  th: Paint,
): Body {
  const b: Body = { lines: [], spans: [], items: [], hits: [] };
  const change = d.kind === "change" ? v.changes.find((c) => c.id === d.id) : undefined;
  if (d.text === null) {
    b.lines.push(th.fg("warning", `… loading rvow show ${d.id}`));
  } else {
    const all = d.text.split("\n").filter((l) => l !== d.id && l !== `title: ${d.title}`);
    for (const line of all.slice(0, MAX_LINES)) {
      const m = change ? /^\s+(TSK-[a-z0-9]+) (.*)$/.exec(line) : null;
      const task = change?.tasks?.find((t) => t.id === m?.[1]);
      if (change && m && task) {
        const [mark, color] = MARKS[task.state] ?? ["?", "dim"];
        const on = focus === b.items.length;
        const at = b.lines.length;
        b.items.push({ kind: "task", change, task });
        b.spans.push([at, at + 1]);
        b.lines.push(
          fit(
            `  ${on ? th.fg("accent", "›") : " "} ${th.fg(color, mark)} ${cut(`${m[1]} ${m[2]}`, w - 8)}`,
            w,
          ),
        );
      } else {
        b.lines.push(...wrapTextWithAnsi(styled(line, th), w));
      }
    }
    if (all.length > MAX_LINES)
      b.lines.push(
        th.fg("dim", `… ${all.length - MAX_LINES} more lines cut; run rvow show ${d.id}`),
      );
    if (change) b.lines.push("", th.fg("dim", "Press enter on a task to read its requirements."));
  }
  const plan = d.kind === "plan" ? v.backlog.find((p) => p.id === d.id) : undefined;
  const actions = plan
    ? planButtons(plan, true, picked)
    : change && archivable(change)
      ? [{ key: "x", label: "Archive", primary: true }]
      : [];
  if (actions.length) {
    b.lines.push("");
    b.hits.push(...buttonRuns(actions, w).map((h) => ({ ...h, line: b.lines.length + h.line })));
    b.lines.push(...buttons(actions, w, true, th));
  }
  return b;
}

const firstTab = (v: View): Tab =>
  v.changes.length > 0 ? "active" : v.backlog.length > 0 ? "backlog" : "archive";

type DockRow = { text: string; item?: Item };
type DockHit = {
  row: number;
  x0: number;
  x1: number;
  act: "tag" | { tab: Tab } | { item: number };
};
type Dock = { lines: string[]; items: Item[]; hits: DockHit[] };
type DockIn = {
  picked: string[];
  cursor: number;
  focus: boolean;
  notice?: Notice;
  width: number;
  cap: number;
  keys: [string, string][];
  loading?: boolean;
  tab: Tab;
};

const DOCK_KEY = "alt+r";
const NO_VIEW: View = {
  branch: null,
  changes: [],
  backlog: [],
  archive: [],
  questions: [],
  next: null,
  needs_you: { amendments: 0, questions: 0, blocked: 0, plans: 0 },
};
const ACTION_KEYS = "atesfxcA";
const DOCK_MIN_COLS = 50;
const DOCK_MIN_ROWS = 20;
const DOCK_MAX_ROWS = 10;

const needsText = (n: NeedsYou) =>
  [
    n.plans > 0 && plural(n.plans, "plan"),
    n.amendments > 0 && plural(n.amendments, "amendment"),
    n.questions > 0 && plural(n.questions, "question"),
    n.blocked > 0 && `${n.blocked} blocked`,
  ]
    .filter(Boolean)
    .join(" · ");

const tabLabels = (v: View) => [
  `1 Backlog ${v.backlog.length}`,
  `2 Active ${v.changes.length}`,
  `3 Archive ${v.archive.length}`,
];

/** Visible columns of each tab label in `tabCounts`, keyed by its digit. */
function tabSpans(v: View): { key: string; x0: number; x1: number }[] {
  let x = 0;
  return tabLabels(v).map((l, i) => {
    const span = { key: String(i + 1), x0: x, x1: x + l.length + 2 };
    x = span.x1 + 1;
    return span;
  });
}

function tabCounts(v: View, active: Tab | undefined, th: Paint): string {
  return tabLabels(v)
    .map((l, i) => {
      const text = ` ${l} `;
      return TABS[i] === active ? th.inverse(th.fg("accent", th.bold(text))) : th.fg("muted", text);
    })
    .join(" ");
}

function dockRows(v: View, o: DockIn, th: Paint): DockRow[] {
  const rows: DockRow[] = [];
  const dim = (t: string) => th.fg("dim", t);
  if (o.tab === "active") {
    for (const c of v.changes) {
      const tasks = c.tasks ?? [];
      const proved = tasks.filter((t) => t.state === "proved").length;
      const fill = tasks.length ? Math.round((8 * proved) / tasks.length) : 0;
      const pending = (c.pending ?? []).length;
      rows.push({
        text: `${th.fg("accent", "▸")} ${th.bold(c.title)} ${th.fg("success", "█".repeat(fill))}${dim("░".repeat(8 - fill))} ${proved}/${tasks.length}${pending ? th.fg("warning", ` · ${plural(pending, "amendment")}`) : ""}`,
        item: { kind: "change", change: c },
      });
      for (const t of tasks) {
        const [mark, color] = MARKS[t.state] ?? ["?", "dim"];
        rows.push({
          text: `  ${th.fg(color, mark)} ${cut(`${t.id} ${t.title}`, o.width - 10)}`,
          item: { kind: "task", change: c, task: t },
        });
      }
    }
    if (v.changes.length === 0)
      rows.push({ text: dim(`Nothing active${v.branch ? ` on ${v.branch}` : ""}.`) });
    if (v.next)
      rows.push({
        text: `${th.fg("accent", "▸ next")} ${dim(v.next.task)} ${v.next.title}`,
      });
  } else if (o.tab === "backlog") {
    for (const p of [...v.backlog.filter((x) => x.here), ...v.backlog.filter((x) => !x.here)]) {
      const mark = o.picked.includes(p.id) ? th.fg("accent", "◉") : dim("○");
      const where = dim(p.here ? p.id : `${p.id} on ${p.branch ?? "?"}`);
      rows.push({
        text: `${mark} ${th.fg(p.here ? "warning" : "dim", "PLAN")} ${p.title} ${where}`,
        item: { kind: "plan", plan: p },
      });
    }
    if (v.backlog.length === 0) rows.push({ text: dim("No plan in the backlog.") });
  } else {
    for (const a of v.archive) {
      const [mark, color] = OUTCOME[a.outcome] ?? ["?", "dim"];
      rows.push({
        text: `${th.fg(color, mark)} ${dim(`${a.date ?? ""} ${a.id} ${a.outcome}`)} ${a.title}`,
        item: { kind: "archive", entry: a },
      });
    }
    if (v.archive.length === 0) rows.push({ text: dim("Nothing archived yet.") });
  }
  if (o.notice) {
    const [glyph, color] = TONE[o.notice.tone];
    rows.push({ text: `${th.fg(color, glyph)} ${o.notice.text.split("\n")[0] ?? ""}` });
  }
  return rows;
}

function dockPanel(v: View, o: DockIn, th: Paint): Dock {
  const rows = o.loading ? [{ text: th.fg("warning", "loading rvow...") }] : dockRows(v, o, th);
  const needs = needsText(v.needs_you);
  const items = rows.flatMap((r) => (r.item ? [r.item] : []));
  const at = Math.max(0, Math.min(o.cursor, items.length - 1));
  const budget = Math.max(1, o.cap - (o.loading ? 2 : 3));
  const first = rows.findIndex((r) => r.item === items[at]);
  const start = Math.max(0, Math.min(first - Math.floor(budget / 2), rows.length - budget));
  const w = o.width;
  const edge = (c: string) => th.fg(o.focus ? "accent" : "borderMuted", c);
  const tag = o.focus
    ? th.inverse(th.bold(th.fg("accent", " focus ")))
    : th.fg("dim", `${DOCK_KEY} focus`);
  const head = ` rvow${v.branch ? ` ⎇ ${v.branch}` : ""}${needs ? ` · needs you: ${needs}` : ""} `;
  const title = truncateToWidth(head, Math.max(1, w - visibleWidth(tag) - 5), "...");
  const fill = Math.max(1, w - 3 - visibleWidth(title) - visibleWidth(tag));
  const row = (text: string) => `${edge("│")} ${fit(text, w - 4)} ${edge("│")}`;
  const marker =
    rows.length > budget
      ? ` ${start + 1}-${Math.min(rows.length, start + budget)}/${rows.length} `
      : "";
  const hintRoom = Math.max(0, w - 2 - visibleWidth(marker));
  const hint = o.focus
    ? truncateToWidth(` ${o.keys.map(([k, l]) => `${k} ${l}`).join("  ")} `, hintRoom, "")
    : "";
  const gap = Math.max(0, hintRoom - visibleWidth(hint));
  const lines = [
    `${edge("╭")}${th.bold(title)}${edge("─".repeat(fill))}${tag}${edge("─╮")}`,
    ...(o.loading ? [] : [row(tabCounts(v, o.tab, th))]),
    ...rows.slice(start, start + budget).map((r, i) => {
      const on = o.focus && r.item !== undefined && r.item === items[at] && start + i === first;
      return row(`${on ? th.fg("accent", "›") : " "} ${r.text}`);
    }),
    `${edge("╰")}${th.fg("dim", hint)}${edge("─".repeat(gap))}${th.fg("dim", marker)}${edge("╯")}`,
  ];
  const hits: DockHit[] = [
    {
      row: 0,
      x0: 1 + visibleWidth(title) + fill,
      x1: 1 + visibleWidth(title) + fill + visibleWidth(tag),
      act: "tag",
    },
  ];
  if (!o.loading) {
    for (const [i, sp] of tabSpans(v).entries())
      hits.push({ row: 1, x0: 2 + sp.x0, x1: 2 + sp.x1, act: { tab: TABS[i] ?? "active" } });
  }
  for (const [i, r] of rows.slice(start, start + budget).entries()) {
    const item = r.item ? items.indexOf(r.item) : -1;
    if (item >= 0) hits.push({ row: (o.loading ? 1 : 2) + i, x0: 2, x1: w - 2, act: { item } });
  }
  return { lines, items, hits };
}

type Sel = { plan?: Plan; change?: Change; v?: View };

type Option = { label: string; description?: string };
type Ask = { ref: string; question: string; header?: string; options: Option[] };

const OTHER = "Other (type your own answer)";
const EDIT_HINT = "Edit the text each answer changes.";

function asked(a: Ask): string {
  const opts = a.options.map((o) => `  - ${o.label}${o.description ? `: ${o.description}` : ""}`);
  return [`${a.ref} ${a.header ? `[${a.header}] ` : ""}${a.question}`, ...opts].join("\n");
}

async function ask(ctx: ExtensionContext): Promise<string> {
  const r = await run(["clarify", "--json"], ctx.cwd, "", SLOW_MS);
  if (r?.code !== 0) return `cannot read the open questions from '${BIN}'`;
  let round: { questions?: Ask[]; remaining?: number };
  try {
    round = JSON.parse(r.out);
  } catch {
    return "cannot read the open questions: rvow printed no JSON";
  }
  const qs = round.questions ?? [];
  if (qs.length === 0) return "no open questions";
  if (!ctx.hasUI) {
    return [
      "No dialog here. Ask the user these in chat, then record each answer with",
      '`rvow answer <ref> "<choice>" --via chat`.',
      "",
      ...qs.map(asked),
      "",
      `${round.remaining ?? 0} more after this round. ${EDIT_HINT}`,
    ].join("\n");
  }
  const done: string[] = [];
  const open = (from: number) => qs.slice(from).map((q) => q.ref);
  for (const [i, q] of qs.entries()) {
    const shown = new Map(
      q.options.map((o) => [o.description ? `${o.label} - ${o.description}` : o.label, o.label]),
    );
    const title = `${q.header ? `${q.header}: ` : ""}${q.question}`;
    let choice = await ctx.ui.select(title, [...shown.keys(), OTHER]);
    if (choice === OTHER) choice = await ctx.ui.input(title, "your answer");
    else if (choice !== undefined) choice = shown.get(choice);
    if (!choice?.trim())
      return [...done, `cancelled; still open: ${open(i).join(", ")}`].join("\n");
    const a = await run(["answer", q.ref, choice, "--via", "dialog"], ctx.cwd, "", SLOW_MS);
    if (a?.code !== 0)
      return [...done, `cannot record ${q.ref}; still open: ${open(i).join(", ")}`].join("\n");
    done.push(`${q.ref} ${q.question} -> ${choice}`);
  }
  return [...done, `${round.remaining ?? 0} questions remain.`, EDIT_HINT].join("\n");
}

async function showText(cwd: string, d: Pick<Detail, "kind" | "id">) {
  const shown = await run(["show", d.id, "--human"], cwd, "", SLOW_MS);
  if (!shown) return { ok: false, text: `cannot run '${BIN}'` };
  let text = shown.out.trim();
  if (d.kind === "task") {
    const covers = /^\s*covers:\s*(.+)$/m.exec(text)?.[1]?.match(ID) ?? [];
    for (const req of covers) {
      text = text
        .split("\n")
        .filter((l) => !l.startsWith(`${req} `))
        .join("\n");
      const r = await run(["show", req, "--human"], cwd, "", SLOW_MS);
      if (r) text += `\n\n${r.out.trim()}`;
    }
  }
  return { ok: shown.code === 0, text: text || "no output" };
}

async function show(ctx: ExtensionContext, lines: (p: Paint, width: number) => string[]) {
  if (ctx.mode !== "tui") return ctx.ui.notify(lines(PLAIN, 80).join("\n"), "info");
  await ctx.ui.custom<void>((tui, theme, _kb, done) => ({
    invalidate() {},
    render: (width: number) => {
      const w = Math.min(width, 80);
      return ["", ...lines(theme, w), "", theme.fg("dim", "esc to close")].map((l) =>
        truncateToWidth(l, w),
      );
    },
    handleInput(data: string) {
      if (matchesKey(data, "escape") || matchesKey(data, "enter") || matchesKey(data, "q")) done();
      tui.requestRender();
    },
  }));
}

type State = {
  view?: View;
  tab?: Tab;
  stack: Detail[];
  selected: string[];
  cursor: Record<string, number>;
  top: Record<string, number>;
  follow: boolean;
  notice?: Notice;
  loaded: boolean;
  loading: boolean;
  saved: string;
  dock: boolean;
  dockFocus: boolean;
  dockCursor: number;
  modal: boolean;
};

export default function (pi: ExtensionAPI) {
  let resend = false;
  let continues = 0;
  const S: State = {
    stack: [],
    selected: [],
    cursor: {},
    top: {},
    follow: true,
    loaded: false,
    loading: false,
    saved: "",
    dock: false,
    dockFocus: false,
    dockCursor: 0,
    modal: false,
  };
  let redraw = () => {};

  async function refresh(ctx: ExtensionContext) {
    const [view, status] = await Promise.all([
      hook("view", ctx.cwd, {}, SLOW_MS, true),
      hook("status", ctx.cwd, {}, SLOW_MS),
    ]);
    const v = normalize(view);
    S.view = v ?? S.view;
    S.selected = S.selected.filter((id) => S.view?.backlog.some((p) => p.id === id));
    await Promise.all(
      S.stack.map(async (d) => {
        const r = await showText(ctx.cwd, d);
        if (r.ok) d.text = r.text;
      }),
    );
    if (!ctx.hasUI) return;
    dockRedraw();
    ctx.ui.setStatus("rvow", typeof status?.status === "string" ? status.status : undefined);
    const line = v && band(v);
    ctx.ui.setWidget(
      "rvow",
      line
        ? (tui, theme) => ({
            invalidate() {},
            render: (width: number) =>
              dockShown(tui, theme, width)
                ? []
                : [
                    truncateToWidth(
                      `${theme.fg("warning", line)} ${theme.fg("dim", "/rvow-status")}`,
                      width,
                    ),
                  ],
          })
        : undefined,
    );
  }

  function savePane() {
    const state = JSON.stringify({
      tab: S.tab,
      details: S.stack.map(({ kind, id, title }) => ({ kind, id, title })),
    });
    if (state === S.saved) return;
    S.saved = state;
    pi.appendEntry(ENTRY, JSON.parse(state));
  }

  async function restore(ctx: ExtensionContext) {
    const v = S.view;
    const last = ctx.sessionManager
      .getEntries()
      .filter((e) => e.type === "custom" && e.customType === ENTRY)
      .at(-1);
    const data = last?.type === "custom" ? last.data : undefined;
    if (!v || !isRecord(data)) return;
    if (isTab(data.tab)) S.tab = data.tab;
    const alive = (d: Detail) =>
      d.kind === "plan"
        ? v.backlog.some((p) => p.id === d.id)
        : d.kind === "change"
          ? v.changes.some((c) => c.id === d.id)
          : d.kind === "archive"
            ? v.archive.some((a) => a.id === d.id)
            : v.changes.some((c) => c.tasks?.some((t) => t.id === d.id));
    const stack: Detail[] = [];
    for (const d of Array.isArray(data.details) ? data.details : []) {
      if (!isRecord(d) || !isKind(d.kind) || typeof d.id !== "string") continue;
      const frame: Detail = { kind: d.kind, id: d.id, title: String(d.title ?? ""), text: null };
      if (alive(frame)) stack.push(frame);
    }
    S.stack = stack;
    await Promise.all(
      stack.map(async (d) => {
        d.text = (await showText(ctx.cwd, d)).text;
      }),
    );
  }

  function settle(r: { code: number; out: string } | undefined) {
    if (!r) return;
    const lines = r.out.split("\n").filter((l) => l.trim());
    const text = r.code === 0 ? (lines.at(-1) ?? "") : lines.slice(0, 3).join("\n");
    S.notice = { tone: r.code === 0 ? "ok" : "fail", text: text || "no output" };
  }

  // The action hotkeys shared by the pane and the dock: undefined when `data` is no action key,
  // {} when it was handled in place, {exit} when the action leaves the view.
  function actionKey(data: string, { plan, change, v }: Sel): { exit?: Exit } | undefined {
    const is = (k: KeyId) => matchesKey(data, k);
    if (is("a") && plan) {
      if (plan.ready && plan.here) return { exit: { act: "accept", id: plan.id } };
      S.notice = { tone: "fail", text: `Not ready: ${blocker(plan)}.` };
      return {};
    }
    if (is("t") && plan && !plan.here) return { exit: { act: "claim", id: plan.id } };
    if (data === "A" && plan?.here && (plan.questions ?? 0) > 0)
      return { exit: { act: "ask", id: plan.id } };
    if (is("e") && plan) return { exit: { act: "refine", plans: [plan] } };
    if (is("s") && plan) {
      S.selected = S.selected.includes(plan.id)
        ? S.selected.filter((x) => x !== plan.id)
        : [...S.selected, plan.id];
      return {};
    }
    if (is("f") && v && S.selected.length > 0)
      return { exit: { act: "refine", plans: v.backlog.filter((p) => S.selected.includes(p.id)) } };
    if (is("x") && change && archivable(change)) return { exit: { act: "archive", id: change.id } };
    if (is("c") && change && (change.pending ?? []).length > 0) return { exit: { act: "amend" } };
    return undefined;
  }

  function layout(tui: TUI, th: Paint, wide: number) {
    const pw = Math.max(24, Math.min(wide, MAX_WIDTH));
    const w = pw - 4;
    const v = S.view;
    const d = S.stack.at(-1);
    const tab = S.tab ?? (v ? firstTab(v) : "backlog");
    const key = d ? `d${S.stack.length}` : tab;
    const make = (focus: number): Body =>
      !v
        ? {
            lines: empty(
              S.loading ? ["loading rvow..."] : ["No rvow change on this branch.", START(th)],
              w,
              th,
            ),
            spans: [],
            items: [],
            hits: [],
          }
        : d
          ? detailBody(v, d, S.selected.includes(d.id), focus, w, th)
          : tabBody(v, tab, S.selected, focus, w, th);
    const probe = make(0);
    const focus = Math.max(0, Math.min(S.cursor[key] ?? 0, probe.items.length - 1));
    const body = focus === 0 ? probe : make(focus);

    const head: string[] = [];
    let title = "";
    if (v) {
      const bar = tabCounts(v, tab, th);
      const branch = v.branch ? th.fg("dim", `⎇ ${v.branch}`) : "";
      title =
        branch && visibleWidth(bar) + 2 + visibleWidth(branch) <= w ? spread(bar, branch, w) : bar;
    }
    if (d) {
      const [badge, color] = BADGE[d.kind];
      head.push(
        th.fg("accent", th.bold("b")) + th.fg("dim", " Back"),
        spread(
          `${th.inverse(th.fg(color, th.bold(badge)))} ${th.bold(d.title)}`,
          th.fg("dim", d.id),
          w,
        ),
        th.fg("dim", "─".repeat(w)),
      );
    }
    if (S.notice) {
      const [glyph, color] = TONE[S.notice.tone];
      for (const [i, l] of S.notice.text.split("\n").entries())
        head.push(`${th.fg(color, i === 0 ? glyph : " ")} ${l}`);
      head.push("");
    }

    const item = body.items[focus];
    const plan =
      item?.kind === "plan"
        ? item.plan
        : d?.kind === "plan"
          ? v?.backlog.find((p) => p.id === d.id)
          : undefined;
    const change =
      item?.kind === "change" || item?.kind === "task"
        ? item.change
        : d?.kind === "change"
          ? v?.changes.find((c) => c.id === d.id)
          : undefined;
    const keys: [string, string][] = [
      ["1-3", "tabs"],
      ["j/k", body.items.length ? "move" : "scroll"],
    ];
    if (d) keys.unshift(["b", "back"]);
    if (item) keys.push(["enter", "open"]);
    if (plan) keys.push(["a", "accept"], ["e", "refine"], ["s", "select"]);
    if (plan && !plan.here) keys.push(["t", "take"]);
    if (change && archivable(change)) keys.push(["x", "archive"]);
    keys.push(["r", "refresh"], ["esc", "close"]);
    const text = (ks: [string, string][]) => ks.map(([k, l]) => `${k} ${l}`).join("  ");
    while (keys.length > 2 && text(keys).length > w - 9) keys.splice(keys.length - 3, 1);
    const hint = keys.map(([k, l]) => `${th.fg("accent", k)} ${th.fg("dim", l)}`).join("  ");

    const panelH = Math.max(12, Math.min(PANEL_ROWS, Math.floor(tui.terminal.rows * 0.9)));
    const bodyH = Math.max(3, panelH - 6 - head.length);
    const total = body.lines.length;
    let top = Math.min(S.top[key] ?? 0, Math.max(0, total - bodyH));
    const span = body.spans[focus];
    if (S.follow && span) {
      if (span[0] < top) top = span[0];
      else if (span[1] > top + bodyH) top = Math.min(span[0], span[1] - bodyH);
    }
    top = Math.max(0, Math.min(top, Math.max(0, total - bodyH)));
    S.top[key] = top;
    const more =
      total > bodyH ? th.fg("dim", `  ${top + 1}-${Math.min(total, top + bodyH)}/${total}`) : "";
    const shown = body.lines.slice(top, top + bodyH);
    const left = " ".repeat(Math.floor(Math.max(0, wide - pw) / 2));
    const ox = left.length + 2;
    const bodyRow0 = 3 + head.length;
    const hits: ScreenHit[] = [];
    if (v)
      for (const sp of tabSpans(v))
        hits.push({ row: 1, x0: ox + sp.x0, x1: ox + sp.x1, key: sp.key });
    if (d) hits.push({ row: 3, x0: ox, x1: ox + "b Back".length, key: "b" });
    const escAt = text(keys.slice(0, -1)).length + (keys.length > 1 ? 2 : 0);
    hits.push({
      row: bodyRow0 + bodyH + 1,
      x0: ox + escAt,
      x1: ox + escAt + "esc close".length,
      key: "\u001b",
    });
    for (const h of body.hits) {
      if (h.line >= top && h.line < top + bodyH)
        hits.push({
          row: bodyRow0 + h.line - top,
          x0: ox + h.x0,
          x1: ox + h.x1,
          key: h.key,
          item: h.item,
        });
    }
    const edge = (c: string) => th.fg("border", c);
    const row = (inner: string) =>
      th.bg("toolPendingBg", `${edge("│")} ${fit(inner, w)} ${edge("│")}`);
    const rule = (l: string, r: string) =>
      th.bg("toolPendingBg", edge(`${l}${"─".repeat(pw - 2)}${r}`));
    const inside = [
      rule("╭", "╮"),
      row(title),
      rule("├", "┤"),
      ...head.map(row),
      ...shown.map(row),
      ...Array.from({ length: bodyH - shown.length }, () => row("")),
      rule("├", "┤"),
      row(`${hint}${more}`),
      rule("╰", "╯"),
    ];
    return {
      hits,
      bodyRow0,
      top,
      lines: inside.map((l) => truncateToWidth(`${left}${l}`, wide, "", true)),
      body,
      focus,
      key,
      plan,
      change,
      item,
      bodyH,
      tab,
      d,
      v,
    };
  }

  function component(ctx: ExtensionContext, tui: TUI, th: Paint, done: (e: Exit) => void) {
    redraw = () => tui.requestRender();
    let wide = MAX_WIDTH;

    async function open(d: Pick<Detail, "kind" | "id" | "title">) {
      const frame: Detail = { ...d, text: null };
      S.stack.push(frame);
      S.notice = undefined;
      S.follow = true;
      redraw();
      const r = await showText(ctx.cwd, d);
      frame.text = r.text;
      redraw();
    }

    async function reload() {
      await refresh(ctx);
      if (!S.view) S.notice = { tone: "fail", text: "rvow did not answer; press r to retry" };
      redraw();
    }

    function openItem(item: Item | undefined) {
      if (item?.kind === "plan")
        void open({ kind: "plan", id: item.plan.id, title: item.plan.title });
      else if (item?.kind === "change")
        void open({ kind: "change", id: item.change.id, title: item.change.title });
      else if (item?.kind === "task")
        void open({ kind: "task", id: item.task.id, title: item.task.title });
      else if (item?.kind === "archive")
        void open({ kind: "archive", id: item.entry.id, title: item.entry.title });
    }

    function handleInput(data: string) {
      const l = layout(tui, th, wide);
      const is = (k: KeyId) => matchesKey(data, k);
      const { plan, change, item, d, v } = l;
      const move = (n: number) => {
        const next = Math.max(0, Math.min(l.focus + n, l.body.items.length - 1));
        if (next === l.focus) S.top[l.key] = Math.max(0, (S.top[l.key] ?? 0) + n * 3);
        S.cursor[l.key] = next;
        S.follow = next !== l.focus || S.follow;
      };
      const scroll = (n: number) => {
        S.top[l.key] = Math.max(0, (S.top[l.key] ?? 0) + n);
        S.follow = false;
      };
      const tabTo = (t: Tab) => {
        S.notice = undefined;
        S.stack = [];
        S.tab = t;
        S.follow = true;
        S.dockCursor = 0;
        dockRedraw();
      };
      const finish = (e: Exit) => done(e);

      if (is("escape") || is("q") || is("ctrl+c")) return finish({ act: "close" });
      if (d && (is("b") || is("backspace"))) {
        S.stack.pop();
        S.notice = undefined;
        delete S.cursor[l.key];
        delete S.top[l.key];
        S.follow = true;
      } else if (is("up") || is("k")) move(-1);
      else if (is("down") || is("j")) move(1);
      else if (is("pageUp")) scroll(-Math.max(1, l.bodyH - 1));
      else if (is("pageDown")) scroll(Math.max(1, l.bodyH - 1));
      else if (is("1")) tabTo("backlog");
      else if (is("2")) tabTo("active");
      else if (is("3")) tabTo("archive");
      else if (!d && (is("tab") || is("right")))
        tabTo(TABS[(TABS.indexOf(l.tab) + 1) % TABS.length] ?? "backlog");
      else if (!d && (is("shift+tab") || is("left")))
        tabTo(TABS[(TABS.indexOf(l.tab) + TABS.length - 1) % TABS.length] ?? "backlog");
      else if (is("enter") || is("d")) openItem(item);
      else if (is("r")) void reload();
      else {
        const r = actionKey(data, { plan, change, v });
        if (!r) return;
        if (r.exit) return finish(r.exit);
      }
      tui.requestRender();
    }

    function handleMouse(e: TuiMouseEvent): TuiMouseEventResult | undefined {
      const l = layout(tui, th, wide);
      if (e.type === "wheel") {
        S.top[l.key] = Math.max(0, (S.top[l.key] ?? 0) + Math.sign(e.wheelDelta ?? 0) * 3);
        S.follow = false;
        return { handled: true };
      }
      if (e.button !== "left") return undefined;
      const hit = l.hits.find((h) => h.row === e.y && e.x >= h.x0 && e.x < h.x1);
      const line = e.y - l.bodyRow0 + l.top;
      let at = -1;
      if (e.y >= l.bodyRow0 && e.y < l.bodyRow0 + l.bodyH) {
        for (const [i, [a, b]] of l.body.spans.entries()) {
          const [pa, pb] = l.body.spans[at] ?? [0, Number.POSITIVE_INFINITY];
          if (line >= a && line < b && b - a <= pb - pa) at = i;
        }
      }
      if (!hit && at < 0) return undefined;
      if (e.type !== "click") return { handled: true, render: false };
      if (hit) {
        if (hit.item !== undefined) S.cursor[l.key] = hit.item;
        handleInput(hit.key);
      } else {
        S.cursor[l.key] = at;
        const kind = l.body.items[at]?.kind;
        if (kind === "task" || kind === "archive" || (e.clickCount ?? 1) >= 2) handleInput("\r");
      }
      return { handled: true };
    }

    return {
      invalidate() {},
      render: (width: number) => {
        wide = width;
        return layout(tui, th, width).lines;
      },
      handleInput,
      handleMouse,
    };
  }

  async function pane(ctx: ExtensionContext) {
    S.loading = true;
    // The pane opens at once; rvow runs in the background and the pane redraws when it answers.
    void refresh(ctx)
      .then(async () => {
        if (S.loaded) return;
        S.loaded = true;
        await restore(ctx);
      })
      .finally(() => {
        S.loading = false;
        if (!S.view)
          S.notice = { tone: "fail", text: `rvow did not answer; is '${BIN}' installed?` };
        redraw();
      });
    for (;;) {
      const exit = await ctx.ui.custom<Exit | undefined>(
        (tui, theme, _kb, done) => component(ctx, tui, theme, done),
        { overlay: true, overlayOptions: { width: "100%", anchor: "center" } },
      );
      savePane();
      if (!exit || exit.act === "close") return;
      if (!(await perform(ctx, exit))) return;
    }
  }

  async function perform(ctx: ExtensionContext, exit: Exclude<Exit, { act: "close" }>) {
    if (exit.act === "refine") {
      pi.sendUserMessage(refinePrompt(exit.plans), { deliverAs: "followUp" });
      S.selected = [];
      ctx.ui.notify("Refine prompt sent to the agent.", "info");
      return false;
    }
    if (exit.act === "amend") await confirmAmendments(ctx);
    else if (exit.act === "ask") {
      S.notice = { tone: "ok", text: await ask(ctx) };
      await refresh(ctx);
    } else if (exit.act === "accept") settle(await acceptPlan(ctx, exit.id));
    else settle(await decide(ctx, [exit.act, exit.id]));
    return true;
  }

  let dockTui: TUI | undefined;
  let uiCtx: ExtensionContext | undefined;
  let unlisten: (() => void) | undefined;
  const dockRedraw = () => dockTui?.requestRender();

  function dockNow(tui: TUI, th: Paint, width: number): Dock | undefined {
    const rows = tui.terminal.rows;
    if (!S.dock || width < DOCK_MIN_COLS || rows < DOCK_MIN_ROWS) return undefined;
    const view = S.view ?? NO_VIEW;
    const tab = S.tab ?? firstTab(view);
    const items = dockItems(view, tab);
    const item = items[Math.max(0, Math.min(S.dockCursor, items.length - 1))];
    const plan = item?.kind === "plan" ? item.plan : undefined;
    const change = item?.kind === "change" || item?.kind === "task" ? item.change : undefined;
    const keys: [string, string][] = [
      ["1-3", "tabs"],
      ["j/k", "move"],
    ];
    if (item) keys.push(["enter", "open"]);
    if (plan) keys.push(["a", "accept"], ["e", "refine"], ["s", "select"]);
    if (plan && !plan.here) keys.push(["t", "take"]);
    if (S.selected.length > 0) keys.push(["f", `refine ${S.selected.length}`]);
    if (change && archivable(change)) keys.push(["x", "archive"]);
    if (change && (change.pending ?? []).length > 0) keys.push(["c", "amend"]);
    keys.push(["esc", "editor"]);
    while (keys.length > 2 && keys.map(([k, l]) => `${k} ${l}`).join("  ").length > width - 6)
      keys.splice(keys.length - 2, 1);
    return dockPanel(
      view,
      {
        loading: !S.view,
        tab,
        picked: S.selected,
        cursor: S.dockCursor,
        focus: S.dockFocus,
        notice: S.notice,
        width,
        cap: Math.min(DOCK_MAX_ROWS, Math.floor(rows * 0.3)),
        keys,
      },
      th,
    );
  }

  const dockItems = (v: View, tab: Tab): Item[] =>
    dockPanel(v, { picked: [], cursor: 0, focus: false, width: 80, cap: 99, keys: [], tab }, PLAIN)
      .items;

  function dockShown(tui: TUI, th: Paint, width: number): boolean {
    return (dockNow(tui, th, width)?.lines.length ?? 0) > 0;
  }

  function syncDock(ctx: ExtensionContext) {
    if (!ctx.hasUI) return;
    ctx.ui.setWidget(
      "rvow-dock",
      S.dock
        ? (tui, theme) => {
            dockTui = tui;
            let last: Dock | undefined;
            return {
              invalidate() {},
              handleMouse: (e: TuiMouseEvent) => dockMouse(e, last),
              render: (width: number) => {
                last = dockNow(tui, theme, width);
                return (last?.lines ?? []).map((l) => truncateToWidth(l, width, "", true));
              },
              dispose() {
                dockTui = undefined;
              },
            };
          }
        : undefined,
      { placement: "aboveEditor" },
    );
  }

  function setDock(ctx: ExtensionContext, on: boolean) {
    S.dock = on;
    if (!on) S.dockFocus = false;
    uiCtx = ctx;
    syncDock(ctx);
    pi.appendEntry(DOCK_ENTRY, { on });
    if (on && !S.view) later(() => refresh(ctx));
  }

  function dockWhyNot(): string | undefined {
    if (!S.dock) return "the rvow dock is off; run /rvow-dock on";
    const { columns, rows } = dockTui?.terminal ?? { columns: DOCK_MIN_COLS, rows: DOCK_MIN_ROWS };
    if (columns < DOCK_MIN_COLS || rows < DOCK_MIN_ROWS)
      return `terminal too small for the rvow dock (needs ${DOCK_MIN_COLS} columns by ${DOCK_MIN_ROWS} rows, this one is ${columns}x${rows})`;
    return S.view ? undefined : "rvow did not answer yet";
  }

  function toggleFocus(ctx: ExtensionContext) {
    uiCtx = ctx;
    const why = S.dockFocus ? undefined : dockWhyNot();
    if (why) return ctx.ui.notify(why, "info");
    S.dockFocus = !S.dockFocus;
    dockRedraw();
  }

  async function modal(work: () => Promise<unknown>) {
    S.modal = true;
    try {
      await work();
    } finally {
      S.modal = false;
      dockRedraw();
    }
  }

  function openAt(ctx: ExtensionContext, item: Item) {
    const [kind, id, title, tab]: [Kind, string, string, Tab] =
      item.kind === "plan"
        ? ["plan", item.plan.id, item.plan.title, "backlog"]
        : item.kind === "change"
          ? ["change", item.change.id, item.change.title, "active"]
          : item.kind === "task"
            ? ["task", item.task.id, item.task.title, "active"]
            : ["archive", item.entry.id, item.entry.title, "archive"];
    const frame: Detail = { kind, id, title, text: null };
    S.loaded = true;
    S.notice = undefined;
    S.stack = [frame];
    S.tab = tab;
    S.follow = true;
    void showText(ctx.cwd, frame).then((r) => {
      frame.text = r.text;
      redraw();
    });
    return pane(ctx);
  }

  // Runs before pi's focused component. While the dock has focus it eats its own keys and
  // passes every other key to the editor, so a draft in the editor is never touched.
  function dockInput(data: string): { consume: true } | undefined {
    const ctx = uiCtx;
    if (!S.dockFocus || S.modal || !S.view || !ctx) return undefined;
    const is = (k: KeyId) => matchesKey(data, k);
    const tab = S.tab ?? firstTab(S.view);
    const items = dockItems(S.view, tab);
    const at = Math.max(0, Math.min(S.dockCursor, items.length - 1));
    const item = items[at];
    const eat = () => {
      dockRedraw();
      return { consume: true } as const;
    };
    const goTab = (t: Tab) => {
      S.tab = t;
      S.stack = [];
      S.dockCursor = 0;
      S.notice = undefined;
      savePane();
      return eat();
    };
    const page = Math.max(1, (dockTui?.terminal.rows ?? 30) * 0.3 - 4);
    const step = (to: number) => {
      S.dockCursor = Math.max(0, Math.min(items.length - 1, to));
      return eat();
    };
    if (is("escape")) {
      S.dockFocus = false;
      return eat();
    }
    if (is("up") || is("k")) return step(at - 1);
    if (is("down") || is("j")) return step(at + 1);
    if (is("pageUp")) return step(at - Math.floor(page));
    if (is("pageDown")) return step(at + Math.floor(page));
    if (is("1")) return goTab("backlog");
    if (is("2")) return goTab("active");
    if (is("3")) return goTab("archive");
    if (is("tab") || is("right")) return goTab(TABS[(TABS.indexOf(tab) + 1) % TABS.length] ?? tab);
    if (is("shift+tab") || is("left"))
      return goTab(TABS[(TABS.indexOf(tab) + TABS.length - 1) % TABS.length] ?? tab);
    if (is("enter") || is("d")) {
      if (item) void modal(() => openAt(ctx, item));
      return eat();
    }
    if (is("r")) {
      void refresh(ctx).then(dockRedraw);
      return eat();
    }
    const plan = item?.kind === "plan" ? item.plan : undefined;
    const change = item?.kind === "change" || item?.kind === "task" ? item.change : undefined;
    S.notice = undefined;
    const r = actionKey(data, { plan, change, v: S.view });
    if (!r && ACTION_KEYS.includes(data) && data.length === 1) {
      S.notice = { tone: "fail", text: "Nothing to act on here." };
      return eat();
    }
    if (!r) return undefined;
    const exit = r.exit;
    if (exit && exit.act !== "close") void modal(() => perform(ctx, exit));
    return eat();
  }

  // Fullscreen mode routes clicks and the wheel here with coordinates local to the dock. Every
  // action goes through dockInput, so a click does what the key does and never touches the editor.
  function dockMouse(e: TuiMouseEvent, dock: Dock | undefined): TuiMouseEventResult | undefined {
    if (!dock || S.modal || !S.view) return undefined;
    if (e.type === "wheel") {
      const items = dockItems(S.view, S.tab ?? firstTab(S.view));
      const to = S.dockCursor + Math.sign(e.wheelDelta ?? 0);
      S.dockCursor = Math.max(0, Math.min(items.length - 1, to));
      return { handled: true };
    }
    if (e.button !== "left") return undefined;
    const hit = dock.hits.find((h) => h.row === e.y && e.x >= h.x0 && e.x < h.x1);
    if (!hit) return undefined;
    if (e.type !== "click") return { handled: true, render: false };
    if (hit.act === "tag") S.dockFocus = !S.dockFocus;
    else if ("tab" in hit.act) {
      S.dockFocus = true;
      dockInput(String(TABS.indexOf(hit.act.tab) + 1));
    } else {
      const was = S.dockFocus && S.dockCursor === hit.act.item;
      S.dockFocus = true;
      S.dockCursor = hit.act.item;
      if (was || (e.clickCount ?? 1) >= 2) dockInput("\r");
    }
    return { handled: true };
  }

  pi.on("tool_call", async (event, ctx) => {
    const input: Record<string, unknown> = { ...(event.input as Record<string, unknown>) };
    if (typeof input.path === "string" && input.file_path === undefined)
      input.file_path = input.path;
    const name = CLAUDE_NAMES[event.toolName] ?? event.toolName;
    const out = (
      await hook("pre-tool-use", ctx.cwd, { tool_name: name, tool_input: input }, FAST_MS)
    )?.hookSpecificOutput;
    if (out?.permissionDecision === "deny") {
      return { block: true, reason: String(out.permissionDecisionReason ?? "rvow: denied") };
    }
  });

  // These handlers only update the status line, so pi never waits for a slow rvow.
  pi.on("session_start", (_event, ctx) => {
    S.loaded = false;
    S.stack = [];
    S.saved = "";
    uiCtx = ctx;
    const saved = ctx.sessionManager
      .getEntries()
      .filter((e) => e.type === "custom" && e.customType === DOCK_ENTRY)
      .at(-1);
    S.dock = saved?.type === "custom" && isRecord(saved.data) && saved.data.on === true;
    S.dockFocus = false;
    const savedPane = ctx.sessionManager
      .getEntries()
      .filter((e) => e.type === "custom" && e.customType === ENTRY)
      .at(-1);
    const savedTab =
      savedPane?.type === "custom" && isRecord(savedPane.data) ? savedPane.data.tab : undefined;
    S.tab = isTab(savedTab) ? savedTab : undefined;
    if (ctx.hasUI) {
      unlisten?.();
      unlisten = ctx.ui.onTerminalInput(dockInput);
      syncDock(ctx);
    }
    later(async () => {
      const text = await brief(ctx.cwd);
      if (text) pi.sendMessage({ customType: "rvow", content: text, display: false });
    });
    later(() => refresh(ctx));
  });

  pi.on("agent_settled", (_event, ctx) => {
    later(() => refresh(ctx));
  });

  pi.on("tool_result", (event, ctx) => {
    const cmd = (event.input as Record<string, unknown>).command;
    if (event.toolName === "bash" && typeof cmd === "string" && /\brvow\b/.test(cmd))
      later(() => refresh(ctx));
  });

  pi.on("session_compact", () => {
    resend = true;
  });

  pi.on("context", async (event, ctx) => {
    if (!resend) return;
    resend = false;
    const text = await brief(ctx.cwd);
    if (!text) return;
    const msg = {
      role: "custom" as const,
      customType: "rvow",
      content: text,
      display: false,
      timestamp: Date.now(),
    };
    return { messages: [...event.messages, msg] };
  });

  pi.on("agent_before_settle", async (event, ctx) => {
    const reason = (await hook("stop", ctx.cwd, {}, SLOW_MS))?.reason;
    if (typeof reason !== "string" || !reason) {
      continues = 0;
      return;
    }
    if (continues >= MAX_CONTINUES) return;
    continues++;
    const entry = {
      type: "custom_message" as const,
      customType: "rvow",
      content: reason,
      display: false,
    };
    return { entries: [...event.entries, entry], continue: true };
  });

  pi.registerCommand("rvow-status", {
    description: "Show the rvow view of this branch",
    handler: async (_args, ctx) => {
      if (ctx.mode === "tui") return pane(ctx);
      await refresh(ctx);
      const v = S.view;
      if (!v) return ctx.ui.notify(`rvow did not answer; is '${BIN}' installed?`, "warning");
      const tabs = TABS.map((t) => [t, ...tabBody(v, t, [], -1, 80, PLAIN).lines].join("\n"));
      ctx.ui.notify(tabs.join("\n\n"), "info");
    },
  });

  pi.registerCommand("rvow-dock", {
    description: "Show or hide the rvow dock above the editor: on, off or toggle",
    handler: async (args, ctx) => {
      const arg = args.trim();
      if (!ctx.hasUI) return ctx.ui.notify("The rvow dock needs a terminal UI.", "warning");
      if (arg && !["on", "off", "toggle"].includes(arg))
        return ctx.ui.notify("usage: /rvow-dock [on|off]", "info");
      const on = arg === "on" || (arg !== "off" && !S.dock);
      setDock(ctx, on);
      const why = on ? dockWhyNot() : undefined;
      ctx.ui.notify(
        !on
          ? "rvow dock off"
          : (why ?? `rvow dock on: shows plans, changes and the next task; ${DOCK_KEY} focuses it`),
        why ? "warning" : "info",
      );
    },
  });

  pi.registerShortcut(DOCK_KEY, {
    description: "Focus or leave the rvow dock",
    handler: (ctx) => toggleFocus(ctx),
  });

  async function confirmAmendments(ctx: ExtensionContext) {
    const list = await run(["confirm"], ctx.cwd, "", SLOW_MS);
    if (!list) return ctx.ui.notify(`Cannot run '${BIN}'.`, "error");
    const ids = list.out
      .split("\n")
      .filter((l) => l.startsWith("AMD-"))
      .map((l) => l.split(/\s/)[0] ?? "");
    if (list.code !== 0 || ids.length === 0) {
      return ctx.ui.notify(
        list.out.trim() || "No pending amendments.",
        list.code === 0 ? "info" : "error",
      );
    }
    const decided: Record<"accept" | "reject", string[]> = { accept: [], reject: [] };
    for (const id of ids) {
      const accept = await ctx.ui.confirm(`rvow ${id}`, `${list.out.trim()}\n\nAccept ${id}?`);
      const verb = accept ? "accept" : "reject";
      const r = await run(["confirm", id, `--${verb}`], ctx.cwd, "", SLOW_MS);
      if (r?.code === 0) decided[verb].push(id);
      ctx.ui.notify(
        r?.out.trim() || `${id}: ${accept ? "accepted" : "rejected"}`,
        r?.code === 0 ? "info" : "error",
      );
    }
    for (const verb of ["accept", "reject"] as const) {
      const prompt = decided[verb].length
        ? decisionPrompt(["confirm", `--${verb}`, ...decided[verb]], "")
        : null;
      if (prompt) pi.sendUserMessage(prompt, { deliverAs: "followUp" });
    }
    await refresh(ctx);
  }

  pi.registerCommand("rvow-confirm", {
    description: "Review and decide pending rvow amendments",
    handler: async (_args, ctx) => {
      if (!ctx.hasUI) {
        ctx.ui.notify("Run `rvow confirm` in a terminal to decide amendments.", "warning");
        return;
      }
      await confirmAmendments(ctx);
    },
  });

  // These decide for the user, so they exist only as commands a person types, and need a dialog.
  function userCommand(
    name: string,
    description: string,
    body: (ctx: ExtensionContext, arg: string) => Promise<void>,
  ) {
    pi.registerCommand(name, {
      description,
      handler: async (args, ctx) => {
        if (!ctx.hasUI) {
          ctx.ui.notify(`Run \`rvow ${name.slice(5)}\` in a terminal.`, "warning");
          return;
        }
        await body(ctx, args.trim().split(/\s+/)[0] ?? "");
      },
    });
  }

  async function pick(ctx: ExtensionContext, arg: string): Promise<string | undefined> {
    if (arg) return arg;
    await refresh(ctx);
    const ps = (S.view?.backlog ?? []).filter((x) => x.here && x.ready);
    const [only] = ps;
    if (!only) {
      ctx.ui.notify("no plan waits on this branch; see /rvow-status", "info");
      return;
    }
    if (ps.length === 1) return only.id;
    const shown = new Map(ps.map((x) => [`${x.id} ${x.title}`, x.id]));
    const choice = await ctx.ui.select("Which plan?", [...shown.keys()]);
    return choice === undefined ? undefined : shown.get(choice);
  }

  async function rvow(ctx: ExtensionContext, argv: string[]) {
    const r = await run(argv, ctx.cwd, "", SLOW_MS);
    if (!r) ctx.ui.notify(`Cannot run '${BIN}'.`, "error");
    return r;
  }

  async function decide(ctx: ExtensionContext, argv: string[]) {
    ctx.ui.setStatus("rvow", `rvow ${argv.join(" ")} ...`);
    const r = await rvow(ctx, argv);
    if (r) {
      ctx.ui.notify(
        r.out.trim() || `rvow ${argv.join(" ")}: done`,
        r.code === 0 ? "info" : "error",
      );
      const prompt = r.code === 0 ? decisionPrompt(argv, r.out) : null;
      if (prompt) pi.sendUserMessage(prompt, { deliverAs: "followUp" });
    }
    await refresh(ctx);
    return r;
  }

  async function acceptPlan(ctx: ExtensionContext, id: string) {
    const r = await rvow(ctx, ["review", id]);
    if (!r) return;
    if (r.code !== 0) {
      ctx.ui.notify(r.out.trim(), "error");
      return r;
    }
    if (!(await ctx.ui.confirm(`rvow ${id}`, `${r.out.trim()}\n\nAccept ${id}?`))) return;
    return decide(ctx, ["accept", id]);
  }

  userCommand(
    "rvow-review",
    "Read a waiting rvow plan and the accept checklist",
    async (ctx, arg) => {
      const id = await pick(ctx, arg);
      const r = id && (await rvow(ctx, ["review", id]));
      if (!r) return;
      if (r.code !== 0) return ctx.ui.notify(r.out.trim(), "error");
      await show(ctx, () => r.out.trim().split("\n"));
    },
  );

  userCommand("rvow-accept", "Accept a waiting rvow plan after reading it", async (ctx, arg) => {
    const id = await pick(ctx, arg);
    if (id) await acceptPlan(ctx, id);
  });

  userCommand("rvow-archive", "Archive a finished rvow change", async (ctx, arg) => {
    if (!arg) return ctx.ui.notify("usage: /rvow-archive <CHG-id>", "info");
    await decide(ctx, ["archive", arg]);
  });

  pi.registerTool({
    name: "rvow_ask",
    label: "rvow ask",
    description:
      "Ask the user the open questions of the rvow plan on this branch, one round of at most 4, and record the answers. Use it instead of asking in chat.",
    parameters: { type: "object", properties: {} } as ToolDefinition["parameters"],
    async execute(_id, _params, _signal, _update, ctx) {
      return { content: [{ type: "text", text: await ask(ctx) }], details: {} };
    },
  });
}
