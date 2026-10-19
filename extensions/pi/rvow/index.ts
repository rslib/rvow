import {
  ACTION_KEYS,
  archivable,
  BADGE,
  type Body,
  band,
  blocker,
  type Detail,
  DOCK_ENTRY,
  DOCK_MAX_ROWS,
  DOCK_MIN_COLS,
  DOCK_MIN_ROWS,
  type Dock,
  detailBody,
  dockKey,
  dockPanel,
  ENTRY,
  type Exit,
  empty,
  firstTab,
  fit,
  ID,
  type Item,
  isKind,
  isRecord,
  isTab,
  type Kind,
  later,
  MAX_WIDTH,
  NO_VIEW,
  type Notice,
  PANEL_ROWS,
  type Paint,
  PLAIN,
  type ScreenHit,
  START,
  spread,
  TABS,
  type Tab,
  TONE,
  tabBody,
  tabCounts,
  tabSpans,
} from "./formats.ts";
import { judgeArgs, judgeRun } from "./judge.ts";
import { decisionPrompt, failedPrompt, refinePrompt } from "./prompts.ts";
import { refreshLoop } from "./refresh.ts";
import {
  ACCEPT_MS,
  type AcceptMemoryStore,
  amendmentChanges,
  BIN,
  createAcceptMemory,
  createUserMade,
  cut,
  FAST_MS,
  GATE_MS,
  recordChanges,
  recordYolo,
  run,
  SLOW_MS,
  shortReview,
} from "./tools.ts";
import { askPendingAmendments, registerTypedTools, yoloArgs } from "./typed-tools.ts";
import type { Change, HookOutput, Plan, View } from "./types.ts";

const MAX_CONTINUES = 8;
// The context and settle handlers sit on the turn, so they get a short budget, not SLOW_MS.
const BOUND_MS = 5_000;

import type {
  ExtensionAPI,
  ExtensionContext,
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
} from "@earendil-works/pi-tui";

async function hook(
  event: string,
  cwd: string,
  extra: object,
  timeoutMs: number,
  emptyMeansNone = false,
  signal?: AbortSignal,
  onFail?: (why: string) => void,
): Promise<HookOutput | undefined> {
  const r = await run(
    ["hook", "claude", event],
    cwd,
    JSON.stringify({ cwd, ...extra }),
    timeoutMs,
    signal,
  );
  if (!r) return void onFail?.(`cannot run '${BIN}'`);
  if (r.code !== 0) return void onFail?.(`'${BIN}' failed or timed out (exit ${r.code})`);
  // rvow prints nothing for `view` in a project with no change and no archive at all.
  if (!r.out.trim()) return emptyMeansNone ? { changes: [] } : undefined;
  try {
    const parsed: unknown = JSON.parse(r.out);
    if (typeof parsed === "object" && parsed !== null && !Array.isArray(parsed)) return parsed;
  } catch {}
  onFail?.(`'${BIN}' printed invalid JSON`);
  return undefined;
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
// Tools that cannot write: a failed pre-tool-use hook still lets these through.
const READ_ONLY = new Set(["read", "grep", "find", "ls", "rvow_view"]);

async function brief(
  cwd: string,
  signal?: AbortSignal,
  timeoutMs = SLOW_MS,
  onFail?: (why: string) => void,
  extra: { session_id?: string; source?: "startup" | "compact" } = {},
): Promise<string | undefined> {
  const text = (await hook("session-start", cwd, extra, timeoutMs, false, signal, onFail))
    ?.hookSpecificOutput?.additionalContext;
  return typeof text === "string" && text ? text : undefined;
}

type Sel = { plan?: Plan; change?: Change; v?: View };

type Option = { label: string; description?: string };
type Ask = { ref: string; question: string; header?: string; options: Option[] };

const OTHER = "Other (type your own answer)";
const EDIT_HINT = "Edit the text each answer changes.";

const ASK_CAP = 200;

function asked(a: Ask): string {
  const opts = a.options.map(
    (o) => `  - ${cut(o.label, ASK_CAP)}${o.description ? `: ${cut(o.description, ASK_CAP)}` : ""}`,
  );
  return [`${a.ref} ${a.header ? `[${a.header}] ` : ""}${cut(a.question, ASK_CAP)}`, ...opts].join(
    "\n",
  );
}

async function ask(ctx: ExtensionContext, change?: string): Promise<string> {
  const r = await run(["clarify", ...(change ? [change] : []), "--json"], ctx.cwd, "", SLOW_MS);
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
    done.push(`${q.ref} ${cut(q.question, ASK_CAP)} -> ${choice}`);
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
  // A judge child session sets RVOW_JUDGE=1: it registers no command, tool or
  // hook, so the judge runs with read-only tools and no rvow extension.
  if (process.env.RVOW_JUDGE === "1") return;
  let toolsRegistered = false;
  // The accept tool's memory of failed and declined reviews, read here so a
  // repeat failure after /rvow-accept sends no followUp the agent already has.
  let acceptMemory: AcceptMemoryStore = createAcceptMemory();
  let epoch = 0;
  let background = new AbortController();
  let stopRefresh = () => {};
  let refreshing: Promise<void> | undefined;
  let resend = false;
  let continues = 0;
  let offShown = false;
  // Why the last pre-tool-use call failed, until a later call works.
  let hookFailed: string | undefined;
  // What the user did in this session; the agent's own tool calls never add to it.
  const userMade = createUserMade();
  // Fail-open stays: a broken rvow never locks the agent out, but the user is told once per session.
  const enforcementOff = (ctx: ExtensionContext) => (why: string) => {
    if (offShown) return;
    offShown = true;
    if (ctx.hasUI) ctx.ui.notify(`rvow enforcement is off: ${why}`, "warning");
  };
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

  function refresh(ctx: ExtensionContext): Promise<void> {
    if (refreshing) return refreshing;
    const pending = updateView(ctx).finally(() => {
      if (refreshing === pending) refreshing = undefined;
    });
    refreshing = pending;
    return pending;
  }

  async function updateView(ctx: ExtensionContext) {
    const generation = epoch;
    const [view, status] = await Promise.all([
      hook("view", ctx.cwd, {}, SLOW_MS, true, background.signal),
      hook("status", ctx.cwd, {}, SLOW_MS, false, background.signal),
    ]);
    if (generation !== epoch) return;
    const v = normalize(view);
    S.view = v ?? S.view;
    S.selected = S.selected.filter((id) => S.view?.backlog.some((p) => p.id === id));
    await Promise.all(
      S.stack.map(async (d) => {
        const r = await showText(ctx.cwd, d);
        if (r.ok) d.text = r.text;
      }),
    );
    if (!ctx.hasUI || generation !== epoch) return;
    dockRedraw();
    ctx.ui.setStatus(
      "rvow",
      hookFailed
        ? `rvow hook failed: ${hookFailed}`
        : typeof status?.status === "string"
          ? status.status
          : undefined,
    );
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
    else settle(await decide(ctx, [exit.act, exit.id], exit.act === "archive" ? GATE_MS : SLOW_MS));
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
    if (!ctx.hasUI || ctx.mode !== "tui") return;
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
      dockRedraw();
      if (ctx.isIdle?.() === false) return undefined;
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

  // A subagent runs in a session of its own that loads this extension, so it can bind itself: when a
  // prompt starts with the `lease:` line of `rvow delegate`, the hook then judges its edits and its
  // `rvow prove` by the leased task, as it does for a bound Claude subagent. A later `lease:` line binds
  // the next lease once the old one ended; while the old one is live the hook refuses with a reason.
  let leaseAgent: string | undefined;
  pi.on("before_agent_start", async (event, ctx) => {
    if (event.messageType === "custom" || !/^\s*lease:\s*TSK-/m.test(event.prompt)) return;
    const id = ctx.sessionManager?.getSessionId?.();
    if (!id) return;
    const out = await hook(
      "lease-bind",
      ctx.cwd,
      { agent_id: id, prompt: event.prompt },
      FAST_MS,
      false,
      undefined,
      enforcementOff(ctx),
    );
    if (out?.bound === true) leaseAgent = id;
    else if (typeof out?.reason === "string" && ctx.hasUI) ctx.ui.notify(out.reason, "warning");
  });

  // The hook's verdict on one call: the reason to block it, or undefined to allow it. A hook that fails
  // blocks a tool that can write, and lets a read-only one through, until a later call works.
  async function gate(
    ctx: ExtensionContext,
    toolName: string,
    input: Record<string, unknown>,
    toolUseId?: string,
  ): Promise<string | undefined> {
    let failed: string | undefined;
    const out = (
      await hook(
        "pre-tool-use",
        ctx.cwd,
        {
          tool_name: CLAUDE_NAMES[toolName] ?? toolName,
          tool_input: input,
          session_id: ctx.sessionManager?.getSessionId?.(),
          agent_id: leaseAgent,
          tool_use_id: toolUseId,
          user_made: userMade,
        },
        FAST_MS,
        false,
        undefined,
        (why) => {
          failed = why;
        },
      )
    )?.hookSpecificOutput;
    if (failed !== undefined) {
      const first = hookFailed === undefined;
      hookFailed = failed;
      if (ctx.hasUI) {
        ctx.ui.setStatus("rvow", `rvow hook failed: ${failed}`);
        if (first) ctx.ui.notify(`rvow hook failed: ${failed}`, "warning");
      }
      if (READ_ONLY.has(toolName)) return undefined;
      return `rvow: the hook failed, so tools that write stay denied until it works: ${failed}. If rvow itself is broken, run \`rvow install\` again`;
    }
    if (hookFailed !== undefined) {
      hookFailed = undefined;
      if (ctx.hasUI) ctx.ui.setStatus("rvow", undefined);
    }
    const reason = String(out?.permissionDecisionReason ?? "rvow: denied");
    if (out?.permissionDecision === "deny") return reason;
    // After repeated denials rvow asks the user once; without a person to ask, the call stays denied.
    if (out?.permissionDecision === "ask") {
      const allowed = ctx.hasUI && (await ctx.ui.confirm("rvow", reason).catch(() => false));
      if (!allowed) return reason;
    }
    return undefined;
  }

  pi.on("tool_call", async (event, ctx) => {
    const input: Record<string, unknown> = { ...(event.input as Record<string, unknown>) };
    if (typeof input.path === "string" && input.file_path === undefined)
      input.file_path = input.path;
    const reason = await gate(ctx, event.toolName, input, event.toolCallId);
    if (reason !== undefined) return { block: true, reason };
  });

  // These handlers only update the status line, so pi never waits for a slow rvow.
  pi.on("session_start", (_event, ctx) => {
    if (!toolsRegistered) {
      acceptMemory = registerTypedTools(pi, run, {
        made: userMade,
        guard: (ctx, command) => gate(ctx, "bash", { command }),
      });
      toolsRegistered = true;
    }
    epoch++;
    continues = 0;
    offShown = false;
    hookFailed = undefined;
    if (ctx.hasUI && dockKey().invalid) ctx.ui.notify(dockKey().invalid ?? "", "warning");
    background.abort();
    background = new AbortController();
    stopRefresh();
    refreshing = undefined;
    S.view = undefined;
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
    unlisten?.();
    unlisten = undefined;
    if (ctx.hasUI && ctx.mode === "tui") {
      unlisten = ctx.ui.onTerminalInput(dockInput);
      syncDock(ctx);
    }
    later(async () => {
      const generation = epoch;
      const text = await brief(ctx.cwd, background.signal, undefined, undefined, {
        session_id: ctx.sessionManager?.getSessionId?.(),
        source: "startup",
      });
      if (text && generation === epoch)
        pi.sendMessage({ customType: "rvow", content: text, display: false });
    });
    later(() => refresh(ctx));
    if (ctx.hasUI && ctx.mode === "tui")
      stopRefresh = refreshLoop(() => (S.dock ? refresh(ctx) : Promise.resolve()));
  });

  pi.on("session_shutdown", async (_event, ctx) => {
    epoch++;
    background.abort();
    stopRefresh();
    unlisten?.();
    unlisten = undefined;
    uiCtx = undefined;
    dockTui = undefined;
    S.dockFocus = false;
    redraw = () => {};
    if (ctx.hasUI) {
      ctx.ui.setWidget("rvow-dock", undefined);
      ctx.ui.setWidget("rvow", undefined);
      ctx.ui.setStatus("rvow", undefined);
    }
    if (leaseAgent) {
      const id = leaseAgent;
      leaseAgent = undefined;
      await hook("lease-end", ctx.cwd, { agent_id: id }, FAST_MS);
    }
  });

  pi.on("agent_settled", (_event, ctx) => {
    later(() => refresh(ctx));
  });

  pi.on("tool_result", async (event, ctx) => {
    // A read changes no blob, so its outcome is not reported: that skips one rvow start per read.
    if (READ_ONLY.has(event.toolName)) return;
    // After the call returns, report its outcome so rvow records the blobs the
    // call changed, with the same call id that pre-tool-use carried.
    const out = await hook(
      "post-tool-use",
      ctx.cwd,
      {
        session_id: ctx.sessionManager?.getSessionId?.(),
        agent_id: leaseAgent,
        tool_use_id: event.toolCallId,
      },
      FAST_MS,
      false,
      undefined,
      enforcementOff(ctx),
    );
    const cmd = (event.input as Record<string, unknown>).command;
    if (event.toolName === "bash" && typeof cmd === "string" && /\brvow\b/.test(cmd))
      later(() => refresh(ctx));
    // A notice that rvow kept for this call goes to the model with the call's result.
    const notice = out?.hookSpecificOutput?.additionalContext;
    if (typeof notice !== "string" || !notice) return;
    return { content: [...event.content, { type: "text" as const, text: notice }] };
  });

  pi.on("session_compact", () => {
    resend = true;
  });

  pi.on("context", async (event, ctx) => {
    if (!resend) return;
    resend = false;
    const text = await brief(ctx.cwd, undefined, BOUND_MS, enforcementOff(ctx), {
      session_id: ctx.sessionManager?.getSessionId?.(),
      source: "compact",
    });
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
    // The CLI gives each reason once per session, so it needs the session id.
    const who = { session_id: ctx.sessionManager?.getSessionId?.(), agent_id: leaseAgent };
    const reason = (
      await hook("stop", ctx.cwd, who, BOUND_MS, false, undefined, enforcementOff(ctx))
    )?.reason;
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
      if (!ctx.hasUI || ctx.mode !== "tui")
        return ctx.ui.notify("The rvow dock needs a terminal UI.", "warning");
      if (arg && !["on", "off", "toggle"].includes(arg))
        return ctx.ui.notify("usage: /rvow-dock [on|off]", "info");
      const on = arg === "on" || (arg !== "off" && !S.dock);
      setDock(ctx, on);
      const why = on ? dockWhyNot() : undefined;
      ctx.ui.notify(
        !on
          ? "rvow dock off"
          : (why ??
              `rvow dock on: shows plans, changes and the next task; ${dockKey().key} focuses it`),
        why ? "warning" : "info",
      );
    },
  });

  pi.registerShortcut(dockKey().key, {
    description: "Focus or leave the rvow dock",
    handler: (ctx) => toggleFocus(ctx),
  });

  async function confirmAmendments(ctx: ExtensionContext) {
    const list = await run(["confirm", "--full"], ctx.cwd, "", SLOW_MS);
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
      if (r?.code === 0) {
        decided[verb].push(id);
        if (accept) recordChanges(userMade, amendmentChanges(list.out, [id]));
      } else if (r)
        pi.sendUserMessage(failedPrompt(["confirm", id, `--${verb}`], r.out), {
          deliverAs: "followUp",
        });
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
    body: (ctx: ExtensionContext, arg: string, all: string) => Promise<void>,
  ) {
    pi.registerCommand(name, {
      description,
      handler: async (args, ctx) => {
        if (!ctx.hasUI) {
          ctx.ui.notify(`Run \`rvow ${name.slice(5)}\` in a terminal.`, "warning");
          return;
        }
        await body(ctx, args.trim().split(/\s+/)[0] ?? "", args.trim());
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

  async function rvow(ctx: ExtensionContext, argv: string[], timeoutMs = SLOW_MS) {
    const r = await run(argv, ctx.cwd, "", timeoutMs);
    if (!r) ctx.ui.notify(`Cannot run '${BIN}'.`, "error");
    return r;
  }

  async function decide(
    ctx: ExtensionContext,
    argv: string[],
    timeoutMs = SLOW_MS,
    failedFollowUp = true,
  ) {
    ctx.ui.setStatus("rvow", `rvow ${argv.join(" ")} ...`);
    const r = await rvow(ctx, argv, timeoutMs);
    if (r) {
      ctx.ui.notify(
        r.out.trim() || `rvow ${argv.join(" ")}: done`,
        r.code === 0 ? "info" : "error",
      );
      const prompt =
        r.code === 0
          ? decisionPrompt(argv, r.out)
          : argv[0] === "yolo"
            ? null
            : failedPrompt(argv, r.out);
      // A repeat the memory caught tells the agent nothing new, so it sends no followUp.
      if (prompt && (r.code === 0 || failedFollowUp))
        pi.sendUserMessage(prompt, { deliverAs: "followUp" });
    }
    await refresh(ctx);
    return r;
  }

  async function acceptPlan(ctx: ExtensionContext, id: string) {
    // The user typed the accept of a change that is already accepted: it is the approval of an accept
    // the hook did not see, so record it and ask nothing.
    await refresh(ctx);
    if (S.view?.changes.some((c) => c.id === id)) {
      recordChanges(userMade, [id]);
      ctx.ui.notify(`${id} is already accepted; your approval is recorded.`, "info");
      return;
    }
    const r = await rvow(ctx, ["review", id]);
    if (!r) return;
    if (r.code !== 0) {
      ctx.ui.notify(r.out.trim(), "error");
      pi.sendUserMessage(failedPrompt(["review", id], r.out), { deliverAs: "followUp" });
      return r;
    }
    // A repeat the memory caught still runs and still asks: the user typed it. Its failure sends no
    // followUp, since the agent already holds the reason from the accept tool.
    const repeat = acceptMemory.remembered(id, r.out) !== null;
    if (!(await ctx.ui.confirm(`rvow ${id}`, `${shortReview(r.out, id).trim()}\n\nAccept ${id}?`)))
      return;
    const done = await decide(ctx, ["accept", id], ACCEPT_MS, !repeat);
    if (done?.code === 0) recordChanges(userMade, [id]);
    return done;
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

  userCommand(
    "rvow-yolo",
    "Relax the hooks for a while: audit|auto|bypass [30m|2h], off, or nothing to see it",
    async (ctx, _first, all) => {
      const [tier, length] = all.split(/\s+/).filter(Boolean);
      let argv: string[];
      try {
        argv = yoloArgs({ ...(tier ? { tier } : {}), ...(length ? { for: length } : {}) }).filter(
          (word) => word !== "--toon",
        );
      } catch (error) {
        return ctx.ui.notify(`${(error as Error).message}; try /rvow-yolo auto 30m`, "info");
      }
      const at = Date.now();
      const done = await decide(ctx, argv);
      if (done?.code === 0 && tier && tier !== "off") recordYolo(userMade, tier, at, length);
    },
  );

  userCommand("rvow-archive", "Archive a finished rvow change", async (ctx, arg) => {
    if (!arg) return ctx.ui.notify("usage: /rvow-archive <CHG-id>", "info");
    const answer = await askPendingAmendments(
      arg,
      async () => {
        const r = await rvow(ctx, ["confirm", "--full"]);
        return { ok: r?.code === 0, text: r?.out ?? "" };
      },
      (message) => ctx.ui.confirm(`rvow ${arg}`, message),
      `Archive ${arg}?`,
    );
    // An unreadable list accepts nothing: the archive below still refuses what is not ready.
    if (answer.kind === "no" || answer.kind === "changing")
      return ctx.ui.notify(
        `${arg} stays active and its plan edits stay pending; run /rvow-archive ${arg} to try again.`,
        "info",
      );
    if (answer.kind === "yes") {
      const done = await rvow(ctx, ["confirm", ...answer.ids, "--accept"]);
      if (done?.code !== 0) {
        if (done) ctx.ui.notify(done.out.trim(), "error");
        return;
      }
      recordChanges(userMade, [arg]);
    }
    await decide(ctx, ["archive", arg], GATE_MS);
  });

  userCommand(
    "rvow-judge",
    "Run the spec conformance judge for a change: [CHG-id] [model]",
    async (ctx, _first, all) => {
      const { change, model } = judgeArgs(all);
      ctx.ui.setStatus("rvow", "rvow judge ...");
      const { text } = await judgeRun(ctx.cwd, {
        ...(change ? { change } : {}),
        ...(model ? { model } : {}),
      });
      ctx.ui.notify(text, "info");
      await refresh(ctx);
    },
  );

  pi.registerTool({
    name: "rvow_judge",
    label: "rvow judge",
    description:
      "Run the spec conformance judge for a change: it reviews the changed code against the plan in a clean read-only session with no rvow extension, and records its findings. The extension runs the judge and records, never the agent.",
    parameters: {
      type: "object",
      properties: {
        change: { type: "string" },
        model: { type: "string" },
      },
      additionalProperties: false,
    } as ToolDefinition["parameters"],
    async execute(_id, params, _signal, _update, ctx) {
      const p = (params ?? {}) as { change?: unknown; model?: unknown };
      const change = typeof p.change === "string" ? p.change : undefined;
      const model = typeof p.model === "string" ? p.model : undefined;
      const { text } = await judgeRun(ctx.cwd, {
        ...(change ? { change } : {}),
        ...(model ? { model } : {}),
        forModel: true,
      });
      await refresh(ctx);
      return { content: [{ type: "text", text }], details: {} };
    },
  });

  pi.registerTool({
    name: "rvow_ask",
    label: "rvow ask",
    description:
      "Use this tool for the open questions of the rvow plan on this branch, one round of at most 3, and record the answers. Pass change to ask the open questions of that change, whatever branch the session is on. Ask in chat only when it is missing.",
    parameters: {
      type: "object",
      properties: { change: { type: "string", pattern: "^CHG-[a-z0-9]+$" } },
    } as ToolDefinition["parameters"],
    async execute(_id, params, _signal, _update, ctx) {
      const change = (params as { change?: unknown } | undefined)?.change;
      if (change !== undefined && (typeof change !== "string" || !/^CHG-[a-z0-9]+$/.test(change)))
        throw new Error("rvow: change must be a CHG id");
      return { content: [{ type: "text", text: await ask(ctx, change) }], details: {} };
    },
  });
}
