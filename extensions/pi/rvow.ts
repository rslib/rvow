import { spawn } from "node:child_process";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const BIN = process.env.RVOW_BIN || "rvow";
const MAX_CONTINUES = 8;
const FAST_MS = 10_000;
const SLOW_MS = 60_000;

let warned = false;

function run(args: string[], cwd: string, stdin: string, timeoutMs: number): Promise<{ code: number; out: string } | undefined> {
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
        console.error(`rvow: cannot run '${BIN}'; its checks are off (set RVOW_BIN or put rvow on PATH)`);
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
  const text = (await hook("session-start", cwd, {}, SLOW_MS))?.hookSpecificOutput?.additionalContext;
  return typeof text === "string" && text ? text : undefined;
}

export default function (pi: ExtensionAPI) {
  let resend = false;
  let continues = 0;

  pi.on("tool_call", async (event, ctx) => {
    const input: Record<string, unknown> = { ...(event.input as Record<string, unknown>) };
    if (typeof input.path === "string" && input.file_path === undefined) input.file_path = input.path;
    const name = CLAUDE_NAMES[event.toolName] ?? event.toolName;
    const out = (await hook("pre-tool-use", ctx.cwd, { tool_name: name, tool_input: input }, FAST_MS))?.hookSpecificOutput;
    if (out?.permissionDecision === "deny") {
      return { block: true, reason: String(out.permissionDecisionReason ?? "rvow: denied") };
    }
  });

  pi.on("session_start", async (_event, ctx) => {
    const text = await brief(ctx.cwd);
    if (text) pi.sendMessage({ customType: "rvow", content: text, display: false });
  });

  pi.on("session_compact", () => {
    resend = true;
  });

  pi.on("context", async (event, ctx) => {
    if (!resend) return;
    resend = false;
    const text = await brief(ctx.cwd);
    if (!text) return;
    const msg = { role: "custom" as const, customType: "rvow", content: text, display: false, timestamp: Date.now() };
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
    const entry = { type: "custom_message" as const, customType: "rvow", content: reason, display: false };
    return { entries: [...event.entries, entry], continue: true };
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
      const ids = list.out.split("\n").filter((l) => l.startsWith("AMD-")).map((l) => l.split(/\s/)[0]);
      if (list.code !== 0 || ids.length === 0) {
        return ctx.ui.notify(list.out.trim() || "No pending amendments.", list.code === 0 ? "info" : "error");
      }
      for (const id of ids) {
        const accept = await ctx.ui.confirm(`rvow ${id}`, `${list.out.trim()}\n\nAccept ${id}?`);
        const r = await run(["confirm", id, accept ? "--accept" : "--reject"], ctx.cwd, "", SLOW_MS);
        ctx.ui.notify(r?.out.trim() || `${id}: ${accept ? "accepted" : "rejected"}`, r?.code === 0 ? "info" : "error");
      }
    },
  });
}
