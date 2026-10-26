import type {
  ExtensionAPI,
  ExtensionContext,
  ToolDefinition,
  ToolNamespace,
} from "@earendil-works/pi-coding-agent";
import type { AskAnswer, AskKind } from "./engine.ts";
import {
  ACCEPT_MS,
  amendmentChanges,
  createAcceptMemory,
  declinedAcceptReason,
  GATE_MS,
  recordChanges,
  recordYolo,
  repeatAcceptResult,
  run,
  SLOW_MS,
  shortReview,
  type UserMade,
} from "./tools.ts";
import type { ToolOutput, View } from "./types.ts";

export type ViewInput = { target?: "view" | "status" | "show"; id?: string };
export type ProofInput = { task: string };
export type AcceptInput = { change: string };
export type ArchiveInput = { change: string };
export type ConfirmInput = { decision?: "accept" | "reject"; id?: string };

export const ACCEPT_DESCRIPTION =
  "Ask the user in a dialog to accept a ready rvow plan. Run `rvow check` first. Use it instead of asking the user to type /rvow-accept.";
export const ARCHIVE_DESCRIPTION =
  "Ask the user in a dialog to archive a change when every task is proved or stale. Otherwise the result names what is missing. The dialog also lists the pending amendments and records unrecorded edits to protected files, so do not run rvow amend for them. Use it instead of asking the user to type /rvow-archive.";

/** The reason of an amendment that the archive tool records for drift. */
export function driftReason(items: { what: string }[]): string {
  const text = `recorded by the archive tool for the user to confirm: ${items.map((d) => d.what).join("; ")}`;
  return text.length <= 500 ? text : `${text.slice(0, 497)}...`;
}

/** The `rvow amend` call that records drift the gate would refuse, or `null` when there is none or a pending plan edit names it. */
export function driftAmendArgs(
  found:
    | {
        drift?: { class: string; what: string }[];
        pending?: { items?: { what: string }[] }[];
      }
    | undefined,
  id: string,
): string[] | null {
  const pending = found?.pending ?? [];
  // A pending edit of an older rvow lists no items, so it counts as holding all drift.
  if (!found || pending.some((p) => p.items === undefined)) return null;
  const held = new Set(pending.flatMap((p) => (p.items ?? []).map((i) => i.what)));
  const items = (found.drift ?? []).filter((d) => d.class !== "free" && !held.has(d.what));
  return items.length > 0 ? ["amend", "--reason", driftReason(items), "--change", id] : null;
}

/** The active change `id` on this branch, else in another worktree. */
export function findChange<T extends { id: string }>(
  view: { changes?: T[]; elsewhere?: T[] },
  id: string,
): T | undefined {
  return view.changes?.find((c) => c.id === id) ?? view.elsewhere?.find((c) => c.id === id);
}

/** Why a change cannot be archived yet, or `null` when every task is proved or stale. A stale task is allowed because archive re-proves it; a missing, failed, flaky or blocked task needs the user first. */
export function archiveBlockers(
  found: { id: string; tasks?: { id: string; state?: string }[]; pending?: unknown[] } | undefined,
  id: string,
): string | null {
  const change = found && { id: found.id, tasks: found.tasks ?? [], pending: found.pending ?? [] };
  if (!change)
    return `rvow: ${id} is not an active change on this branch or in any worktree; see rvow status --all`;
  if (change.tasks.length === 0)
    return `rvow: ${id} has no task, so there is nothing proved to archive`;
  const open = change.tasks
    .filter((t) => t.state !== "proved" && !t.state?.startsWith("stale"))
    .map((t) => `${t.id} is ${t.state ?? "unknown"}`);
  if (open.length > 0)
    return `rvow: ${id} is not ready to archive: ${open.join(", ")}; fix them first`;
  return null;
}

const MODEL_TEXT_CAP = 4_000;

export const CONFIRM_DESCRIPTION =
  "List the pending rvow amendments, or pass id to see the diff of one. With a decision, ask the user in a dialog to accept or reject them all, or only id. Nothing changes unless the user says yes.";

export function acceptArgs(input: unknown): string {
  const value = closed(input, ["change"]);
  if (typeof value.change !== "string" || !/^CHG-[a-z0-9]+$/.test(value.change))
    throw new Error("rvow: change must be a complete CHG id");
  return value.change;
}

export function confirmId(input: unknown): string | undefined {
  const value = closed(input, ["decision", "id"]);
  if (value.id === undefined) return undefined;
  if (typeof value.id !== "string" || !/^AMD-[a-z0-9]+$/.test(value.id))
    throw new Error("rvow: id must be an AMD id");
  return value.id;
}

/** The lines of one amendment in a `rvow confirm` listing, or undefined when it is not listed. */
export function amendmentBlock(listing: string, id: string): string | undefined {
  const lines: string[] = [];
  let inside = false;
  for (const line of listing.split("\n")) {
    if (/^digest \S+$/.test(line) || /^decide with:/.test(line)) break;
    const head = /^(AMD-[\w-]+) for /.exec(line);
    if (head) inside = head[1] === id;
    if (inside && line.trim()) lines.push(line);
  }
  return lines.length > 0 ? lines.join("\n") : undefined;
}

export type ChangeAmendment = { id: string; block: string };

/** The pending amendments of one change in a `rvow confirm` listing, those holding a weaken item first. */
export function changeAmendments(listing: string, change: string): ChangeAmendment[] {
  const found: ChangeAmendment[] = [];
  let current: { id: string; change: string; lines: string[] } | undefined;
  const flush = () => {
    if (current?.change === change) found.push({ id: current.id, block: current.lines.join("\n") });
    current = undefined;
  };
  for (const line of listing.split("\n")) {
    if (/^digest \S+$/.test(line) || /^decide with:/.test(line)) break;
    const head = /^(AMD-[\w-]+) for (\S+)/.exec(line);
    if (head) {
      flush();
      current = { id: head[1] ?? "", change: head[2] ?? "", lines: [line] };
    } else if (current && line.trim()) current.lines.push(line);
  }
  flush();
  const weakens = (a: ChangeAmendment) => /^ {2}weaken:/m.test(a.block);
  return [...found.filter(weakens), ...found.filter((a) => !weakens(a))];
}

const ASK_ROUNDS = 3;

export type ArchiveAsk =
  | { kind: "none" }
  | { kind: "yes"; ids: string[] }
  | { kind: "no" }
  | { kind: "changing" }
  | { kind: "unreadable"; text: string };

/**
 * Show every pending amendment of `change` in one dialog and, on a yes, re-read the list: when it is
 * not the list the user saw, ask again with the new one, so no unseen amendment is ever accepted.
 * `rvow confirm --all` decides every pending amendment of the branch, so the caller accepts by id.
 */
export async function askPendingAmendments(
  change: string,
  read: () => Promise<{ ok: boolean; text: string }>,
  confirm: (message: string) => Promise<boolean>,
  tail: string,
): Promise<ArchiveAsk> {
  for (let round = 0; round < ASK_ROUNDS; round++) {
    const first = await read();
    if (!first.ok) return { kind: "unreadable", text: first.text };
    const shown = changeAmendments(first.text, change);
    if (shown.length === 0) return { kind: "none" };
    const body = shown.map((a) => a.block).join("\n");
    const yes = await confirm(
      `${change} has ${shown.length} pending plan edit(s), weaken ones first. Archiving accepts all of them:\n\n${body}\n\n${tail}`,
    );
    if (!yes) return { kind: "no" };
    const again = await read();
    if (!again.ok) return { kind: "unreadable", text: again.text };
    const now = changeAmendments(again.text, change);
    if (now.map((a) => a.block).join("\n") === body)
      return { kind: "yes", ids: shown.map((a) => a.id) };
  }
  return { kind: "changing" };
}

export function confirmDecision(input: unknown): "accept" | "reject" | undefined {
  const value = closed(input, ["decision", "id"]);
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
  "Show or change rvow's yolo mode, which relaxes the hooks for a while. With no tier it shows what is on.",
  "A tier (audit, auto or bypass) needs the user's yes in a dialog, and off needs none.",
  `audit: ${YOLO_TIERS.audit}. auto: ${YOLO_TIERS.auto}. bypass: ${YOLO_TIERS.bypass}.`,
  "The gate still checks everything at push.",
  "Do not use it to get around one denial. Run rvow block with a reason instead.",
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
  } else if (
    code === 0 &&
    ["accept", "confirm", "review", "archive", "amend"].includes(args[0] ?? "")
  ) {
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

/** What the extension lends the tools: the user's record, the hook's verdict on a shell command (the deny reason, or undefined to allow), and the host program that draws rvow questions. */
export type Gate = {
  made: UserMade;
  guard: (ctx: ExtensionContext, command: string) => Promise<string | undefined>;
  /** Put one accept, confirm or archive question to the user; undefined means no host draws rvow questions, so the tool opens its own dialog. */
  ask?: (kind: AskKind, change: string, ctx: ExtensionContext) => Promise<AskAnswer | undefined>;
};

/** The result of a question the host program drew: its own text, and no dialog here. */
function hostAnswer(answer: AskAnswer) {
  const text = answer.text.trim() || `rvow: the host answered ${answer.outcome}`;
  return answer.outcome === "done"
    ? {
        content: [{ type: "text" as const, text }],
        details: { kind: "decision" as const, code: 0, text },
      }
    : {
        content: [{ type: "text" as const, text }],
        details: { kind: "failure" as const, code: 1, error: text, stdout: "" },
      };
}

/** Tool search ranks by this text, and only codemode listings show the description. */
export const RVOW_NAMESPACE: ToolNamespace = {
  name: "rvow",
  description:
    "rvow holds the agent to the plan the user accepted. Its tools put each decision to the user in a dialog.",
  instructions:
    "Use the rvow tools to accept or approve a ready plan, archive a finished change, confirm or reject amendments (plan edits), ask the open questions of a plan, turn yolo mode on or off and run the spec conformance judge. The hooks deny rvow accept, rvow confirm and rvow archive in the shell, so call these tools instead.",
};

/** The tools that put a decision to the user. They wait for `tool_search` while it is on. */
export const DIALOG_TOOLS = [
  "rvow_accept",
  "rvow_archive",
  "rvow_confirm",
  "rvow_yolo",
  "rvow_ask",
  "rvow_judge",
] as const;

/** Declare the dialog tools to the model when no `tool_search` can load them, so they stay reachable. */
export function declareDialogTools(pi: Pick<ExtensionAPI, "getActiveTools" | "setActiveTools">) {
  const active = pi.getActiveTools();
  if (active.includes("tool_search")) return;
  const missing = DIALOG_TOOLS.filter((name) => !active.includes(name));
  if (missing.length > 0) pi.setActiveTools([...active, ...missing]);
}

export function registerTypedTools(pi: ExtensionAPI, execute = run, gate?: Gate) {
  const queue = serialQueue();
  const acceptMemory = createAcceptMemory();
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
      output.out.length <= MODEL_TEXT_CAP
        ? output.out
        : `${output.out.slice(0, MODEL_TEXT_CAP)}\n[output truncated; read rvow ${args[0] === "prove" ? `show ${args[1]} --full` : args.join(" ")} in a terminal for more detail]`;
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
    exposure: "codemode",
    namespace: RVOW_NAMESPACE,
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
    exposure: "codemode",
    namespace: RVOW_NAMESPACE,
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
      // The hook judges `rvow prove` by the agent's lease, so ask it first; a denied prove never runs.
      const denied = await gate?.guard(ctx, `rvow prove ${args[1]}`);
      if (denied)
        return {
          content: [{ type: "text" as const, text: denied }],
          details: { kind: "failure" as const, code: 1, error: denied, stdout: "" },
          isError: true as const,
        };
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
    exposure: "deferred",
    namespace: RVOW_NAMESPACE,
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
      // A host program that draws rvow questions asks the user on its own screen, so the tool
      // opens no dialog and returns the host's own words as the result.
      const host = await gate?.ask?.("accept", change, ctx);
      if (host) {
        if (host.outcome === "done") {
          if (gate) recordChanges(gate.made, [change]);
          acceptMemory.forget(change);
        }
        return hostAnswer(host);
      }
      if (!ctx.hasUI) throw new Error("rvow: only the user can accept, and no one is here to ask");
      const review = await queue(() => result(["review", change], ctx.cwd, signal, SLOW_MS));
      if (review.details.kind !== "decision") return review;
      const reviewText = text(review);
      // A repeat while the plan is unchanged asks nothing: the earlier reason stands.
      const remembered = acceptMemory.remembered(change, reviewText);
      if (remembered) {
        const out = repeatAcceptResult(remembered.reason, change);
        return remembered.declined
          ? declined(out)
          : {
              content: [{ type: "text" as const, text: out }],
              details: {
                kind: "failure" as const,
                code: 1,
                error: remembered.reason,
                stdout: "",
              },
              isError: true as const,
            };
      }
      const yes = await ctx.ui.confirm(
        `rvow ${change}`,
        `${shortReview(reviewText, change).trim()}\n\nAccept ${change}?`,
      );
      if (!yes) {
        const reason = declinedAcceptReason(change);
        acceptMemory.remember(change, reviewText, reason, true);
        return declined(repeatAcceptResult(reason, change));
      }
      const done = await queue(() =>
        result(["accept", change, "--yes"], ctx.cwd, signal, ACCEPT_MS),
      );
      if (done.details.kind === "decision") {
        if (gate) recordChanges(gate.made, [change]);
        acceptMemory.forget(change);
        return done;
      }
      const [first] = done.content;
      const reason =
        first && first.type === "text" && first.text.trim() ? first.text : "rvow accept failed";
      acceptMemory.remember(change, reviewText, reason, false);
      return done;
    },
  });
  pi.registerTool({
    name: "rvow_archive",
    label: "rvow archive",
    exposure: "deferred",
    namespace: RVOW_NAMESPACE,
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
      // A host program that draws rvow questions asks the user on its own screen, so the tool
      // opens no dialog and returns the host's own words as the result.
      const host = await gate?.ask?.("archive", change, ctx);
      if (host) return hostAnswer(host);
      if (!ctx.hasUI) throw new Error("rvow: only the user can archive, and no one is here to ask");
      const seen = await queue(() => result(["hook", "view"], ctx.cwd, signal, SLOW_MS));
      if (seen.details.kind !== "view") return seen;
      const found = seen.details.data ? findChange(seen.details.data, change) : undefined;
      const blocked = archiveBlockers(found, change);
      if (blocked)
        return {
          content: [{ type: "text" as const, text: blocked }],
          details: { kind: "failure" as const, code: 1, error: blocked, stdout: "" },
        };
      const amendArgs = driftAmendArgs(found, change);
      if (amendArgs) {
        const amended = await queue(() => result(amendArgs, ctx.cwd, signal, SLOW_MS));
        if (amended.details.kind !== "decision") return amended;
      }
      const tail = `Archive ${change}? The gate trusts an archived change and never reruns its tasks.`;
      let accept: string[] = [];
      let asked = false;
      if ((found?.pending ?? []).length > 0 || amendArgs) {
        const answer = await askPendingAmendments(
          change,
          async () => {
            const list = await queue(() => result(["confirm", "--full"], ctx.cwd, signal, SLOW_MS));
            return { ok: list.details.kind === "decision", text: text(list) };
          },
          (message) => ctx.ui.confirm(`rvow ${change}`, message),
          tail,
        );
        if (answer.kind === "unreadable")
          return {
            content: [{ type: "text" as const, text: answer.text }],
            details: { kind: "failure" as const, code: 1, error: answer.text, stdout: "" },
            isError: true as const,
          };
        if (answer.kind === "changing")
          return declined(
            `The pending plan edits of ${change} kept changing, so nothing was accepted and it stays active. The user can run /rvow-archive ${change} to decide them and archive it.`,
          );
        if (answer.kind === "no")
          return declined(
            `The user did not archive ${change}; it stays active and its plan edits stay pending. The user can run /rvow-archive ${change} to decide them and archive it.`,
          );
        if (answer.kind === "yes") {
          accept = answer.ids;
          asked = true;
        }
      }
      if (!asked) {
        const yes = await ctx.ui.confirm(
          `rvow ${change}`,
          `Every task of ${change} is proved or stale, and archive re-proves the stale ones. Archive it? The gate trusts an archived change and never reruns its tasks.`,
        );
        if (!yes)
          return declined(
            `The user did not archive ${change}; it stays active. The user can run /rvow-archive ${change} to archive it.`,
          );
      }
      if (accept.length > 0) {
        const done = await queue(() =>
          result(["confirm", ...accept, "--accept"], ctx.cwd, signal, SLOW_MS),
        );
        if (done.details.kind !== "decision") return done;
        if (gate) recordChanges(gate.made, [change]);
      }
      return queue(() => result(["archive", change], ctx.cwd, signal, GATE_MS));
    },
  });
  pi.registerTool({
    name: "rvow_confirm",
    label: "rvow confirm",
    exposure: "deferred",
    namespace: RVOW_NAMESPACE,
    outputSchema,
    description: CONFIRM_DESCRIPTION,
    parameters: {
      type: "object",
      additionalProperties: false,
      properties: {
        decision: { type: "string", enum: ["accept", "reject"] },
        id: { type: "string", pattern: "^AMD-[a-z0-9]+$" },
      },
    } as ToolDefinition["parameters"],
    execute: async (_id, params, signal, _update, ctx) => {
      const decision = confirmDecision(params);
      const named = confirmId(params);
      if (decision === undefined)
        return queue(() =>
          result(named === undefined ? ["confirm"] : ["confirm", named], ctx.cwd, signal, SLOW_MS),
        );
      const list = await queue(() => result(["confirm", "--full"], ctx.cwd, signal, SLOW_MS));
      // A host program that draws rvow questions puts them to the user on its own screen, so the
      // tool opens no dialog and returns the host's own words as the result.
      const change = amendmentChanges(text(list), named === undefined ? undefined : [named])[0];
      const host = change === undefined ? undefined : await gate?.ask?.("confirm", change, ctx);
      if (host) return hostAnswer(host);
      if (named !== undefined && decision !== undefined && list.details.kind === "decision") {
        const block = amendmentBlock(text(list), named);
        if (!block) return declined(`${named} is not pending.`);
        if (!ctx.hasUI)
          throw new Error("rvow: only the user can decide, and no one is here to ask");
        const yes = await ctx.ui.confirm(
          "rvow confirm",
          `${block}\n\n${decision === "accept" ? "Accept" : "Reject"} ${named}?`,
        );
        if (!yes)
          return declined(
            `The user did not decide ${named}, so it is still pending. The user can run /rvow-confirm to decide it.`,
          );
        const done = await queue(() =>
          result(["confirm", named, `--${decision}`], ctx.cwd, signal, SLOW_MS),
        );
        if (gate && decision === "accept" && done.details.kind === "decision")
          recordChanges(gate.made, amendmentChanges(text(list), [named]));
        return done;
      }
      const digest = /^digest (\S+)$/m.exec(text(list))?.[1];
      if (list.details.kind !== "decision" || !digest) return list;
      if (!ctx.hasUI) throw new Error("rvow: only the user can decide, and no one is here to ask");
      const yes = await ctx.ui.confirm(
        "rvow confirm",
        `${text(list).trim()}\n\n${decision === "accept" ? "Accept" : "Reject"} all of these?`,
      );
      if (!yes)
        return declined(
          "The user did not decide, so the plan edits are still pending. The user can run /rvow-confirm to decide them.",
        );
      const done = await queue(() =>
        result(["confirm", `--${decision}`, "--all", digest], ctx.cwd, signal, SLOW_MS),
      );
      if (gate && decision === "accept" && done.details.kind === "decision")
        recordChanges(gate.made, amendmentChanges(text(list)));
      return done;
    },
  });
  // Only the user's answer in the dialog switches a tier on. The tool runs the CLI itself, since the
  // hook refuses an agent that runs `rvow yolo` in a shell.
  pi.registerTool({
    name: "rvow_yolo",
    label: "rvow yolo",
    exposure: "deferred",
    namespace: RVOW_NAMESPACE,
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
      const on = tier !== undefined && tier !== "off" && !tier.startsWith("--");
      if (on) {
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
      const at = Date.now();
      const done = await queue(() => result(args, ctx.cwd, signal, SLOW_MS));
      if (gate && on && tier && done.details.kind === "yolo")
        recordYolo(gate.made, tier, at, args[2] === "--for" ? args[3] : undefined);
      return done;
    },
  });
  return acceptMemory;
}
