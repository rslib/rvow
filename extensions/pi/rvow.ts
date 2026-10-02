import { spawn } from "node:child_process";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { matchesKey, truncateToWidth } from "@earendil-works/pi-tui";

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

async function hook(event: string, cwd: string, extra: object, timeoutMs: number): Promise<any> {
  const r = await run(["hook", "claude", event], cwd, JSON.stringify({ cwd, ...extra }), timeoutMs);
  if (!r || r.code !== 0 || !r.out.trim()) return undefined;
  try {
    return JSON.parse(r.out);
  } catch {
    return undefined;
  }
}

const CLAUDE_NAMES: Record<string, string> = { bash: "Bash", edit: "Edit", write: "Write" };

async function brief(cwd: string): Promise<string | undefined> {
  const text = (await hook("session-start", cwd, {}, SLOW_MS))?.hookSpecificOutput
    ?.additionalContext;
  return typeof text === "string" && text ? text : undefined;
}

type Paint = { fg: (color: any, text: string) => string; bold: (text: string) => string };
const PLAIN: Paint = { fg: (_c, t) => t, bold: (t) => t };

const MARKS: Record<string, [string, string]> = {
  proved: ["✓", "success"],
  failed: ["✗", "error"],
  stale: ["~", "warning"],
  flaky: ["≈", "warning"],
  blocked: ["⊘", "accent"],
  missing: ["·", "dim"],
};
const DRIFT_COLOR: Record<string, string> = { weaken: "error", confirm: "warning" };

function plural(n: number, one: string, many = one + "s"): string {
  return `${n} ${n === 1 ? one : many}`;
}

function needsYou(v: any): string {
  const n = v?.needs_you ?? {};
  const parts = [
    n.amendments > 0 && plural(n.amendments, "amendment"),
    n.questions > 0 && plural(n.questions, "question"),
    n.plans > 0 && plural(n.plans, "plan"),
  ].filter(Boolean);
  return parts.join(" · ");
}

function summary(v: any, p: Paint): string[] {
  const lines: string[] = [];
  for (const c of v.changes ?? []) {
    const tasks: any[] = c.tasks ?? [];
    const proved = tasks.filter((t) => t.state === "proved").length;
    lines.push(`rvow ${c.id}: ${proved}/${tasks.length} proved`);
  }
  const need = needsYou(v);
  if (need) lines.push(p.fg("warning", `needs you: ${need}`));
  return lines;
}

function detail(v: any, p: Paint, width: number): string[] {
  const out: string[] = [];
  const add = (s = "") => out.push(truncateToWidth(s, width));
  for (const c of v.changes ?? []) {
    if (out.length) add();
    add(`${p.fg("dim", c.id)} ${p.bold(c.title)}${v.branch ? p.fg("dim", `  (${v.branch})`) : ""}`);
    for (const t of c.tasks ?? []) {
      const [mark, color] = MARKS[t.state] ?? ["?", "dim"];
      const why = t.reasons?.length ? p.fg("dim", ` (${t.reasons.join(", ")})`) : "";
      add(`  ${p.fg(color, mark)} ${p.fg("dim", t.id)} ${t.title}${why}`);
    }
    if (c.drift?.length) {
      add();
      add(p.bold("Drift"));
      for (const d of c.drift) add(`  ${p.fg(DRIFT_COLOR[d.class] ?? "dim", d.class)} ${d.what}`);
    }
    if (c.pending?.length) {
      add();
      add(`${p.bold("Pending amendments")} ${p.fg("dim", "/rvow-confirm")}`);
      for (const a of c.pending) add(`  ${p.fg("warning", a.id)} ${a.reason}`);
    }
  }
  if (v.plans?.length) {
    if (out.length) add();
    add(p.bold("Plans waiting for accept"));
    for (const pl of v.plans) {
      add(`  ${p.fg("dim", pl.id)} ${pl.title}`);
      for (const q of (v.questions ?? []).filter((x: any) => x.change === pl.id)) {
        add(`    ${p.fg("warning", "?")} ${q.question}`);
        if (q.recommended) add(p.fg("dim", `      recommended: ${q.recommended}`));
      }
    }
  }
  if (v.next) {
    if (out.length) add();
    add(`${p.bold("Next")} ${p.fg("dim", v.next.task)} ${v.next.title}`);
    if (v.next.verify) add(p.fg("dim", `  verify: ${v.next.verify}`));
  }
  return out;
}

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
  if (!r || r.code !== 0) return `cannot read the open questions from '${BIN}'`;
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
    if (!a || a.code !== 0)
      return [...done, `cannot record ${q.ref}; still open: ${open(i).join(", ")}`].join("\n");
    done.push(`${q.ref} ${q.question} -> ${choice}`);
  }
  return [...done, `${round.remaining ?? 0} questions remain.`, EDIT_HINT].join("\n");
}

export default function (pi: ExtensionAPI) {
  let resend = false;
  let continues = 0;
  let cached: any;

  async function refresh(ctx: ExtensionContext) {
    cached = await hook("view", ctx.cwd, {}, SLOW_MS);
    if (!ctx.hasUI) return;
    const v = cached;
    ctx.ui.setWidget(
      "rvow",
      v
        ? (_tui, theme) => ({
            invalidate() {},
            render: (width: number) => summary(v, theme).map((l) => truncateToWidth(l, width)),
          })
        : undefined,
    );
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

  pi.on("session_start", async (_event, ctx) => {
    const text = await brief(ctx.cwd);
    if (text) pi.sendMessage({ customType: "rvow", content: text, display: false });
    await refresh(ctx);
  });

  pi.on("agent_settled", async (_event, ctx) => {
    await refresh(ctx);
  });

  pi.on("tool_result", async (event, ctx) => {
    const cmd = (event.input as Record<string, unknown>).command;
    if (event.toolName === "bash" && typeof cmd === "string" && /\brvow\b/.test(cmd))
      await refresh(ctx);
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

  pi.registerCommand("rvow", {
    description: "Show the rvow view of this branch",
    handler: async (_args, ctx) => {
      await refresh(ctx);
      const v = cached;
      if (!v) return ctx.ui.notify("no active rvow change on this branch", "info");
      if (ctx.mode !== "tui") return ctx.ui.notify(detail(v, PLAIN, 80).join("\n"), "info");
      await ctx.ui.custom<void>((tui, theme, _kb, done) => ({
        invalidate() {},
        render: (width: number) => {
          const w = Math.min(width, 80);
          return ["", ...detail(v, theme, w), "", theme.fg("dim", "esc to close")].map((l) =>
            truncateToWidth(l, w),
          );
        },
        handleInput(data: string) {
          if (matchesKey(data, "escape") || matchesKey(data, "enter") || matchesKey(data, "q"))
            done();
          tui.requestRender();
        },
      }));
    },
  });

  pi.registerCommand("rvow-confirm", {
    description: "Review and decide pending rvow amendments",
    handler: async (_args, ctx) => {
      if (!ctx.hasUI) {
        ctx.ui.notify("Run `rvow confirm` in a terminal to decide amendments.", "warning");
        return;
      }
      const list = await run(["confirm"], ctx.cwd, "", SLOW_MS);
      if (!list) return ctx.ui.notify(`Cannot run '${BIN}'.`, "error");
      const ids = list.out
        .split("\n")
        .filter((l) => l.startsWith("AMD-"))
        .map((l) => l.split(/\s/)[0]);
      if (list.code !== 0 || ids.length === 0) {
        return ctx.ui.notify(
          list.out.trim() || "No pending amendments.",
          list.code === 0 ? "info" : "error",
        );
      }
      for (const id of ids) {
        const accept = await ctx.ui.confirm(`rvow ${id}`, `${list.out.trim()}\n\nAccept ${id}?`);
        const r = await run(
          ["confirm", id, accept ? "--accept" : "--reject"],
          ctx.cwd,
          "",
          SLOW_MS,
        );
        ctx.ui.notify(
          r?.out.trim() || `${id}: ${accept ? "accepted" : "rejected"}`,
          r?.code === 0 ? "info" : "error",
        );
      }
    },
  });

  pi.registerTool({
    name: "rvow_ask",
    label: "rvow ask",
    description:
      "Ask the user the open questions of the rvow plan on this branch, one round of at most 4, and record the answers. Use it instead of asking in chat.",
    parameters: { type: "object", properties: {} } as any,
    async execute(_id, _params, _signal, _update, ctx) {
      return { content: [{ type: "text", text: await ask(ctx) }], details: {} };
    },
  });
}
