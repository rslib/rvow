import type { ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { run, SLOW_MS } from "./tools.ts";
import type { ToolOutput, View } from "./types.ts";

export type ViewInput = { target?: "view" | "status" | "show"; id?: string };
export type ProofInput = { task: string };
export type AcceptInput = { change: string };
export type ArchiveInput = { change: string };
export type ConfirmInput = { decision?: "accept" | "reject" };

export const ACCEPT_DESCRIPTION =
  "Ask the user to accept a ready rvow plan. The user reads the review text and the checklist in a dialog and answers; the plan is accepted only when they say yes. The result tells you what happened, and when it failed it says why. Use this instead of asking the user to type /rvow-accept, and run `rvow check` first so the plan is ready.";
export const ARCHIVE_DESCRIPTION =
  "Ask the user whether a finished rvow change may be archived. It is only offered when every task of the change is proved and no plan edit is pending; otherwise the result names what is missing and nothing is asked. The user answers in a dialog and the change is archived only when they say yes. An archived change is trusted from then on: a push never reruns its tasks. Use it when the last task is proved, instead of asking the user to type /rvow-archive.";

/** Why a change cannot be archived yet, or `null` when every task is proved and no plan edit is pending. */
export function archiveBlockers(
  found: { id: string; tasks?: { id: string; state?: string }[]; pending?: unknown[] } | undefined,
  id: string,
): string | null {
  const change = found && { id: found.id, tasks: found.tasks ?? [], pending: found.pending ?? [] };
  if (!change) return `rvow: ${id} is not an active change on this branch; see rvow status`;
  if (change.tasks.length === 0)
    return `rvow: ${id} has no task, so there is nothing proved to archive`;
  const open = change.tasks
    .filter((t) => t.state !== "proved")
    .map((t) => `${t.id} is ${t.state ?? "unknown"}`);
  if (open.length > 0)
    return `rvow: ${id} is not ready to archive: ${open.join(", ")}; prove them first`;
  if (change.pending.length > 0)
    return `rvow: ${id} has ${change.pending.length} pending plan edit(s); the user decides them first with rvow_confirm`;
  return null;
}

export const CONFIRM_DESCRIPTION =
  "Show the pending rvow plan edits (amendments), or ask the user to accept or reject all of them. With no decision it only lists them. With a decision the user reads the list in a dialog and answers; nothing changes unless they say yes. The result tells you what happened, and when it failed it says why.";

export function acceptArgs(input: unknown): string {
  const value = closed(input, ["change"]);
  if (typeof value.change !== "string" || !/^CHG-[a-z0-9]+$/.test(value.change))
    throw new Error("rvow: change must be a complete CHG id");
  return value.change;
}

export function confirmDecision(input: unknown): "accept" | "reject" | undefined {
  const value = closed(input, ["decision"]);
  if (value.decision !== undefined && value.decision !== "accept" && value.decision !== "reject")
    throw new Error("rvow: decision must be accept or reject");
  return value.decision;
}

export type YoloInput = { tier?: "audit" | "auto" | "bypass" | "off"; for?: string };

/** What each yolo tier changes, as the user reads it in the dialog and the model reads it in the tool text. */
export const YOLO_TIERS = {
  audit: "edits go through before the plan is accepted or outside a task, and are only counted",
  auto: "also lets the agent run rvow accept and rvow confirm",
  bypass: "every hook only audits and a stop is never held back",
} as const;

export const YOLO_DESCRIPTION = [
  "Show or change rvow's yolo mode, which relaxes the hooks for a while. Call it with no tier to see what is on.",
  "A tier needs the user to allow it in a dialog, and the user may refuse.",
  `audit: ${YOLO_TIERS.audit}.`,
  `auto: ${YOLO_TIERS.auto}.`,
  `bypass: ${YOLO_TIERS.bypass}.`,
  "off ends it at once and needs no dialog. A tier ends by itself after 2 hours, or after `for` (such as 30m or 4h, at most 24h).",
  "The gate still checks everything at push, and tampering with the ledger, the enforcement files or the hidden checks stays denied below bypass.",
  "Do not use it to get around one denial; run rvow block with a reason instead.",
].join(" ");

export const VIEW_KEYS = [
  "branch",
  "changes",
  "backlog",
  "archive",
  "questions",
  "next",
  "needs_you",
] as const;

function closed(input: unknown, keys: string[]): Record<string, unknown> {
  if (!input || typeof input !== "object" || Array.isArray(input))
    throw new Error("rvow: expected an object");
  const value = input as Record<string, unknown>;
  if (Object.keys(value).some((key) => !keys.includes(key)))
    throw new Error("rvow: unknown input field; accept and confirm belong to the user");
  return value;
}

export function viewArgs(input: unknown): string[] {
  const value = closed(input, ["target", "id"]);
  const target = value.target ?? "view";
  if (!["view", "status", "show"].includes(String(target)))
    throw new Error("rvow: target must be view, status or show");
  if (target === "show") {
    if (typeof value.id !== "string" || !/^(CHG|TSK|REQ)-[a-z0-9]+$/.test(value.id))
      throw new Error("rvow: show needs a complete CHG, TSK or REQ id");
    return ["show", value.id, "--toon"];
  }
  if (value.id !== undefined) throw new Error("rvow: id is only valid for show");
  return target === "status" ? ["status", "--json"] : ["hook", "view"];
}

export function proofArgs(input: unknown): string[] {
  const value = closed(input, ["task"]);
  if (typeof value.task !== "string" || !/^TSK-[a-z0-9]+$/.test(value.task))
    throw new Error("rvow: prove needs a complete TSK id");
  return ["prove", value.task, "--toon"];
}

export function yoloArgs(input: unknown): string[] {
  const value = closed(input, ["tier", "for"]);
  const { tier, for: length } = value;
  if (tier === undefined) {
    if (length !== undefined) throw new Error("rvow: for needs a tier");
    return ["yolo", "--toon"];
  }
  if (typeof tier !== "string" || !["audit", "auto", "bypass", "off"].includes(tier))
    throw new Error("rvow: tier must be audit, auto, bypass or off");
  if (length === undefined) return ["yolo", tier, "--toon"];
  if (tier === "off") throw new Error("rvow: off takes no length");
  if (typeof length !== "string" || !/^[1-9][0-9]*[smh]$/.test(length))
    throw new Error("rvow: for must look like 30m or 2h");
  return ["yolo", tier, "--for", length, "--toon"];
}

export function serialQueue() {
  let tail: Promise<unknown> = Promise.resolve();
  return <T>(work: () => Promise<T>): Promise<T> => {
    const next = tail.then(work);
    tail = next.catch(() => undefined);
    return next;
  };
}

function scalar(stdout: string, key: string): unknown {
  const matches = [...stdout.matchAll(new RegExp(`^${key}: (.*)$`, "gm"))];
  if (matches.length !== 1) return undefined;
  const text = matches[0]?.[1] ?? "";
  if (text.startsWith('"') || text === "null" || /^\d+$/.test(text)) return JSON.parse(text);
  return text;
}

function parseOutput(args: string[], code: number, stdout: string): ToolOutput {
  if (args[0] === "prove") {
    const task = scalar(stdout, "task");
    const result = scalar(stdout, "result");
    const tests = scalar(stdout, "tests");
    const ms = scalar(stdout, "ms");
    const event = scalar(stdout, "event");
    if (
      task === args[1] &&
      typeof result === "string" &&
      (tests === null || (Number.isSafeInteger(tests) && Number(tests) >= 0)) &&
      Number.isSafeInteger(ms) &&
      Number(ms) >= 0 &&
      typeof event === "string" &&
      /^[a-f0-9]{16}$/.test(event)
    )
      return {
        kind: "proof",
        code,
        task,
        result,
        tests: tests as number | null,
        duration_ms: Number(ms),
        event,
      };
  } else if (code === 0 && ["accept", "confirm", "review", "archive"].includes(args[0] ?? "")) {
    return { kind: "decision", code, text: stdout };
  } else if (code === 0 && args[0] === "yolo") {
    const tier = scalar(stdout, "tier");
    const minutes = scalar(stdout, "minutes");
    if (typeof tier === "string")
      return { kind: "yolo", code, tier, ...(typeof minutes === "number" ? { minutes } : {}) };
  } else if (code === 0 && args[0] === "show") {
    return { kind: "artifact-text", code, id: args[1] ?? "", text: stdout };
  } else if (code === 0) {
    const data: unknown = stdout.trim() ? JSON.parse(stdout) : null;
    if (
      data === null ||
      (typeof data === "object" &&
        !Array.isArray(data) &&
        Array.isArray((data as View).changes) &&
        VIEW_KEYS.every((key) => key in data))
    ) {
      return { kind: args[0] === "status" ? "status" : "view", code, data: data as View | null };
    }
  }
  if (code === 0) throw new Error("rvow: invalid structured command output");
  const error = scalar(stdout, "error");
  const cliCode = scalar(stdout, "code");
  return {
    kind: "failure",
    code,
    error: typeof error === "string" ? error : stdout || "rvow: command failed or was cancelled",
    stdout,
    ...(typeof cliCode === "string" ? { cliCode } : {}),
  };
}

const outputSchema = {
  type: "object",
  required: ["kind", "code"],
  properties: {
    kind: {
      type: "string",
      enum: ["view", "status", "artifact-text", "proof", "yolo", "decision", "failure"],
    },
    tier: { type: "string" },
    minutes: { type: "integer" },
    code: { type: "integer" },
    data: { anyOf: [{ type: "object" }, { type: "null" }] },
    id: { type: "string" },
    text: { type: "string" },
    task: { type: "string" },
    result: { type: "string" },
    tests: { anyOf: [{ type: "integer" }, { type: "null" }] },
    duration_ms: { type: "integer" },
    event: { type: "string", pattern: "^[a-f0-9]{16}$" },
    error: { type: "string" },
    stdout: { type: "string" },
    cliCode: { type: "string" },
  },
  additionalProperties: false,
};

export function registerTypedTools(pi: ExtensionAPI, execute = run) {
  const queue = serialQueue();
  const result = async (
    args: string[],
    cwd: string,
    signal: AbortSignal | undefined,
    timeout: number,
  ) => {
    if (signal?.aborted) throw new Error("rvow: cancelled before execution");
    const value = await execute(args, cwd, "", timeout, signal);
    const output = value ?? { code: 1, out: "rvow: cannot run the configured binary" };
    const details = parseOutput(args, output.code, output.out);
    const text =
      output.out.length <= 16_000
        ? output.out
        : `${output.out.slice(0, 16_000)}\n[output truncated; read rvow ${args[0] === "prove" ? `show ${args[1]} --full` : args.join(" ")} in a terminal for more detail]`;
    return {
      content: [{ type: "text" as const, text }],
      details,
      structuredContent: details,
      ...(output.code !== 0 ? { isError: true } : {}),
    };
  };
  pi.registerTool({
    name: "rvow_view",
    label: "rvow view",
    outputSchema,
    description:
      "Read the branch dashboard, proof status or one plan artifact. Cannot accept or confirm.",
    parameters: {
      type: "object",
      additionalProperties: false,
      properties: {
        target: { type: "string", enum: ["view", "status", "show"] },
        id: { type: "string", pattern: "^(CHG|TSK|REQ)-[a-z0-9]+$" },
      },
    } as ToolDefinition["parameters"],
    execute: async (_id, params, signal, _update, ctx) =>
      result(viewArgs(params), ctx.cwd, signal, SLOW_MS),
  });
  pi.registerTool({
    name: "rvow_prove",
    label: "rvow prove",
    outputSchema,
    description:
      "Run one accepted task's stored verification. Proof calls run in order. Cannot accept or confirm.",
    parameters: {
      type: "object",
      additionalProperties: false,
      required: ["task"],
      properties: { task: { type: "string", pattern: "^TSK-[a-z0-9]+$" } },
    } as ToolDefinition["parameters"],
    execute: async (_id, params, signal, _update, ctx) => {
      const args = proofArgs(params);
      return queue(() => result(args, ctx.cwd, signal, 30 * 60_000));
    },
  });
  // These two tools are the agent's way to the user's decisions. The user answers in a dialog, the tool
  // runs the CLI itself, and a failure comes back as the tool's error with the CLI's own words.
  const text = (value: Awaited<ReturnType<typeof result>>) =>
    value.details.kind === "decision" ? value.details.text : "";
  const declined = (why: string) => ({
    content: [{ type: "text" as const, text: why }],
    details: { kind: "failure" as const, code: 1, error: "declined", stdout: "" },
  });
  pi.registerTool({
    name: "rvow_accept",
    label: "rvow accept",
    outputSchema,
    description: ACCEPT_DESCRIPTION,
    parameters: {
      type: "object",
      additionalProperties: false,
      required: ["change"],
      properties: { change: { type: "string", pattern: "^CHG-[a-z0-9]+$" } },
    } as ToolDefinition["parameters"],
    execute: async (_id, params, signal, _update, ctx) => {
      const change = acceptArgs(params);
      if (!ctx.hasUI) throw new Error("rvow: only the user can accept, and no one is here to ask");
      const review = await queue(() => result(["review", change], ctx.cwd, signal, SLOW_MS));
      if (review.details.kind !== "decision") return review;
      const yes = await ctx.ui.confirm(
        `rvow ${change}`,
        `${text(review).trim()}\n\nAccept ${change}?`,
      );
      if (!yes)
        return declined(
          `The user did not accept ${change}; it stays a plan. The user can run /rvow-accept ${change} to start it.`,
        );
      return queue(() => result(["accept", change, "--yes"], ctx.cwd, signal, 10 * 60_000));
    },
  });
  pi.registerTool({
    name: "rvow_archive",
    label: "rvow archive",
    outputSchema,
    description: ARCHIVE_DESCRIPTION,
    parameters: {
      type: "object",
      additionalProperties: false,
      required: ["change"],
      properties: { change: { type: "string", pattern: "^CHG-[a-z0-9]+$" } },
    } as ToolDefinition["parameters"],
    execute: async (_id, params, signal, _update, ctx) => {
      const change = acceptArgs(params);
      if (!ctx.hasUI) throw new Error("rvow: only the user can archive, and no one is here to ask");
      const seen = await queue(() => result(["hook", "view"], ctx.cwd, signal, SLOW_MS));
      if (seen.details.kind !== "view") return seen;
      const blocked = archiveBlockers(
        seen.details.data?.changes.find((c) => c.id === change),
        change,
      );
      if (blocked)
        return {
          content: [{ type: "text" as const, text: blocked }],
          details: { kind: "failure" as const, code: 1, error: blocked, stdout: "" },
        };
      const yes = await ctx.ui.confirm(
        `rvow ${change}`,
        `Every task of ${change} is proved. Archive it? The gate trusts an archived change and never reruns its tasks.`,
      );
      if (!yes)
        return declined(
          `The user did not archive ${change}; it stays active. The user can run /rvow-archive ${change} to archive it.`,
        );
      return queue(() => result(["archive", change], ctx.cwd, signal, SLOW_MS));
    },
  });
  pi.registerTool({
    name: "rvow_confirm",
    label: "rvow confirm",
    outputSchema,
    description: CONFIRM_DESCRIPTION,
    parameters: {
      type: "object",
      additionalProperties: false,
      properties: { decision: { type: "string", enum: ["accept", "reject"] } },
    } as ToolDefinition["parameters"],
    execute: async (_id, params, signal, _update, ctx) => {
      const decision = confirmDecision(params);
      const list = await queue(() => result(["confirm"], ctx.cwd, signal, SLOW_MS));
      const digest = /^digest (\S+)$/m.exec(text(list))?.[1];
      if (decision === undefined || list.details.kind !== "decision" || !digest) return list;
      if (!ctx.hasUI) throw new Error("rvow: only the user can decide, and no one is here to ask");
      const yes = await ctx.ui.confirm(
        "rvow confirm",
        `${text(list).trim()}\n\n${decision === "accept" ? "Accept" : "Reject"} all of these?`,
      );
      if (!yes)
        return declined(
          "The user did not decide, so the plan edits are still pending. The user can run /rvow-confirm to decide them.",
        );
      return queue(() =>
        result(["confirm", `--${decision}`, "--all", digest], ctx.cwd, signal, SLOW_MS),
      );
    },
  });
  // Only the user's answer in the dialog switches a tier on. The tool runs the CLI itself, since the
  // hook refuses an agent that runs `rvow yolo` in a shell.
  pi.registerTool({
    name: "rvow_yolo",
    label: "rvow yolo",
    outputSchema,
    description: YOLO_DESCRIPTION,
    parameters: {
      type: "object",
      additionalProperties: false,
      properties: {
        tier: { type: "string", enum: ["audit", "auto", "bypass", "off"] },
        for: { type: "string", pattern: "^[1-9][0-9]*[smh]$" },
      },
    } as ToolDefinition["parameters"],
    execute: async (_id, params, signal, _update, ctx) => {
      const args = yoloArgs(params);
      const tier = args[1];
      if (tier && tier !== "off" && !tier.startsWith("--")) {
        const length = args[2] === "--for" ? args[3] : "2h";
        if (!ctx.hasUI)
          throw new Error("rvow: only the user can switch yolo on, and no one is here to ask");
        const allowed = await ctx.ui.confirm(
          "rvow yolo",
          `The agent asks to turn on yolo ${tier} for ${length}: ${YOLO_TIERS[tier as keyof typeof YOLO_TIERS]}. The gate still checks at push. Allow it?`,
        );
        if (!allowed)
          return {
            content: [
              { type: "text" as const, text: "The user declined, so yolo stays as it was." },
            ],
            details: { kind: "failure" as const, code: 1, error: "declined", stdout: "" },
          };
      }
      return queue(() => result(args, ctx.cwd, signal, SLOW_MS));
    },
  });
}
