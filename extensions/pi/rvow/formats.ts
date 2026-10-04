import type { ThemeBg, ThemeColor } from "@earendil-works/pi-coding-agent";
import { truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import type { Archived, Change, NeedsYou, Plan, Task, View } from "./types.ts";
export type Paint = {
  fg: (color: ThemeColor, text: string) => string;
  bold: (text: string) => string;
  inverse: (text: string) => string;
  bg: (color: ThemeBg, text: string) => string;
};
export const PLAIN: Paint = {
  fg: (_c, t) => t,
  bold: (t) => t,
  inverse: (t) => t,
  bg: (_c, t) => t,
};

// These strings and glyphs mirror extensions/claude/hooks/format.ts; rvow.test.ts keeps them equal.
export const MARKS: Record<string, [string, ThemeColor]> = {
  proved: ["✓", "success"],
  failed: ["✗", "error"],
  stale: ["~", "warning"],
  flaky: ["≈", "warning"],
  blocked: ["⊘", "accent"],
  missing: ["·", "dim"],
};
export const OUTCOME: Record<string, [string, ThemeColor]> = {
  done: ["✓", "success"],
  dropped: ["✗", "error"],
  superseded: ["↷", "warning"],
};
export const DRIFT_COLOR: Record<string, ThemeColor> = { weaken: "error", confirm: "warning" };
export const ID = /\b(?:CHG|TSK|REQ|AMD)-[a-z0-9]+\b/g;
export const MAX_LINES = 200;
export const MAX_WIDTH = 100;
export const PANEL_ROWS = 34;

export const later = (work: () => Promise<unknown>) => void work().catch(() => undefined);

export function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}

export function blocker(p: Plan): string {
  return !p.here
    ? "take it to this branch first"
    : (p.questions ?? 0) > 0
      ? "answer the open questions first"
      : (p.issues ?? 0) > 0
        ? "fix what rvow check reports"
        : "add at least one task with a verify command";
}

export function middle(text: string, width: number): string {
  if (text.length <= width) return text;
  const keep = Math.max(2, width - 3);
  return `${text.slice(0, Math.ceil(keep / 2))}...${text.slice(-Math.floor(keep / 2))}`;
}

export function cut(text: string, width: number): string {
  const line = text.replace(/\s+/g, " ").trim();
  return line.length <= width ? line : `${line.slice(0, Math.max(0, width - 3))}...`;
}

export function band(v: View): string | undefined {
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

export type Tab = "backlog" | "active" | "archive";
export type Kind = "plan" | "change" | "task" | "archive";
export type Detail = { kind: Kind; id: string; title: string; text: string | null };
export type Notice = { tone: "ok" | "fail"; text: string };
export type Item =
  | { kind: "plan"; plan: Plan }
  | { kind: "change"; change: Change }
  | { kind: "task"; change: Change; task: Task }
  | { kind: "archive"; entry: Archived };
export type Exit =
  | { act: "close" }
  | { act: "accept" | "claim" | "archive" | "ask"; id: string }
  | { act: "amend" }
  | { act: "refine"; plans: Plan[] };
export type ScreenHit = { row: number; x0: number; x1: number; key: string; item?: number };
export type Hit = { line: number; x0: number; x1: number; key: string; item?: number };
export type Body = { lines: string[]; spans: [number, number][]; items: Item[]; hits: Hit[] };

export const TABS: Tab[] = ["backlog", "active", "archive"];
export const KINDS: Kind[] = ["plan", "change", "task", "archive"];
export const BADGE: Record<Kind, [string, ThemeColor]> = {
  plan: [" PLAN ", "warning"],
  change: [" CHANGE ", "accent"],
  task: [" TASK ", "accent"],
  archive: [" ARCHIVED ", "dim"],
};
export const TONE: Record<Notice["tone"], [string, ThemeColor]> = {
  ok: ["✓", "success"],
  fail: ["✗", "error"],
};
export const ENTRY = "rvow-pane";
export const DOCK_ENTRY = "rvow-dock";

export const isTab = (v: unknown): v is Tab => TABS.some((t) => t === v);
export const isKind = (v: unknown): v is Kind => KINDS.some((k) => k === v);
export const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

export const fit = (s: string, n: number) => truncateToWidth(s, n, "...", true);
export const spread = (left: string, right: string, n: number) =>
  `${truncateToWidth(left, Math.max(0, n - visibleWidth(right) - 1), "...", true)} ${right}`;

export function card(
  rows: string[],
  w: number,
  color: ThemeColor,
  lit: boolean,
  th: Paint,
): string[] {
  const edge = (s: string) => th.fg(color, s);
  const gutter = lit ? th.fg("accent", "▌ ") : "  ";
  return [
    `${gutter}${edge(`╭${"─".repeat(w - 4)}╮`)}`,
    ...rows.map((r) => `${gutter}${edge("│")} ${fit(r, w - 6)} ${edge("│")}`),
    `${gutter}${edge(`╰${"─".repeat(w - 4)}╯`)}`,
  ];
}

export type Button = { key: string; label: string; primary?: boolean };

export function buttons(bs: Button[], n: number, on: boolean, th: Paint): string[] {
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
export function buttonRuns(bs: Button[], n: number): Hit[] {
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

export function planButtons(p: Plan, inDetail: boolean, picked: boolean): Button[] {
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

export const archivable = (c: Change) =>
  (c.tasks ?? []).length > 0 &&
  (c.tasks ?? []).every((t) => t.state === "proved") &&
  (c.pending ?? []).length === 0;

export const HOME_DIR = process.env.HOME;

export function planCard(
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

export function changeCard(
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

export function empty(lines: string[], w: number, th: Paint): string[] {
  return card(lines, w, "borderMuted", false, th);
}

export const START = (th: Paint) =>
  `${th.fg("dim", "Start a plan: ")}${th.fg("accent", 'rvow new "<title>"')}`;

export function tabBody(
  v: View,
  tab: Tab,
  picked: string[],
  focus: number,
  w: number,
  th: Paint,
): Body {
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

export function styled(line: string, th: Paint): string {
  const ids = (s: string) => s.replace(ID, (m) => th.fg("dim", m));
  const heading = /^#+ (.*)$/.exec(line)?.[1] ?? (/^\S[^:]*:$/.test(line) ? line : null);
  if (heading !== null) return th.bold(ids(heading));
  const kv = /^(\s*)([a-z][a-z ]*):( .*)?$/.exec(line);
  if (kv?.[2] === "verify") return th.fg("dim", line);
  return kv ? `${kv[1]}${th.fg("dim", `${kv[2]}:`)}${ids(kv[3] ?? "")}` : ids(line);
}

export function detailBody(
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

export const firstTab = (v: View): Tab =>
  v.changes.length > 0 ? "active" : v.backlog.length > 0 ? "backlog" : "archive";

export type DockRow = { text: string; item?: Item };
export type DockHit = {
  row: number;
  x0: number;
  x1: number;
  act: "tag" | { tab: Tab } | { item: number };
};
export type Dock = { lines: string[]; items: Item[]; hits: DockHit[] };
export type DockIn = {
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

export function focusShortcut(value = "alt+r"): import("@earendil-works/pi-tui").KeyId {
  if (
    !/^(?:(?:ctrl|shift|alt|super)\+)*(?:[a-z0-9]|escape|enter|tab|space|backspace|delete|insert|home|end|pageUp|pageDown|up|down|left|right|f[1-9]|f1[0-2])$/.test(
      value,
    )
  )
    throw new Error("rvow: RVOW_FOCUS_SHORTCUT must be a pi key chord such as alt+r");
  return value as import("@earendil-works/pi-tui").KeyId;
}

type FocusKey = import("@earendil-works/pi-tui").KeyId;
type FocusChoice = { key: FocusKey; invalid?: string };

export function pickFocusKey(value: string | undefined): FocusChoice {
  try {
    return { key: focusShortcut(value || "alt+r") };
  } catch {
    return {
      key: "alt+r",
      invalid: `rvow: RVOW_FOCUS_SHORTCUT '${value}' is not a pi key chord such as alt+r; using alt+r`,
    };
  }
}

let focusChoice: FocusChoice | undefined;

/** Read on first use, not at load: a bad value must not stop the extension from loading. */
export function dockKey(): FocusChoice {
  focusChoice ??= pickFocusKey(process.env.RVOW_FOCUS_SHORTCUT);
  return focusChoice;
}
export const NO_VIEW: View = {
  branch: null,
  changes: [],
  backlog: [],
  archive: [],
  questions: [],
  next: null,
  needs_you: { amendments: 0, questions: 0, blocked: 0, plans: 0 },
};
export const ACTION_KEYS = "atesfxcA";
export const DOCK_MIN_COLS = 50;
export const DOCK_MIN_ROWS = 20;
export const DOCK_MAX_ROWS = 10;

export const needsText = (n: NeedsYou) =>
  [
    n.plans > 0 && plural(n.plans, "plan"),
    n.amendments > 0 && plural(n.amendments, "amendment"),
    n.questions > 0 && plural(n.questions, "question"),
    n.blocked > 0 && `${n.blocked} blocked`,
  ]
    .filter(Boolean)
    .join(" · ");

export const tabLabels = (v: View) => [
  `1 Backlog ${v.backlog.length}`,
  `2 Active ${v.changes.length}`,
  `3 Archive ${v.archive.length}`,
];

/** Visible columns of each tab label in `tabCounts`, keyed by its digit. */
export function tabSpans(v: View): { key: string; x0: number; x1: number }[] {
  let x = 0;
  return tabLabels(v).map((l, i) => {
    const span = { key: String(i + 1), x0: x, x1: x + l.length + 2 };
    x = span.x1 + 1;
    return span;
  });
}

export function tabCounts(v: View, active: Tab | undefined, th: Paint): string {
  return tabLabels(v)
    .map((l, i) => {
      const text = ` ${l} `;
      return TABS[i] === active ? th.inverse(th.fg("accent", th.bold(text))) : th.fg("muted", text);
    })
    .join(" ");
}

export function dockRows(v: View, o: DockIn, th: Paint): DockRow[] {
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

export function dockPanel(v: View, o: DockIn, th: Paint): Dock {
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
    : th.fg("dim", `${dockKey().key} focus`);
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
