import type { ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { run, SLOW_MS } from "./tools.ts";
import type { ToolOutput, View } from "./types.ts";

export type ViewInput = { target?: "view" | "status" | "show"; id?: string };
export type ProofInput = { task: string };

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
    kind: { type: "string", enum: ["view", "status", "artifact-text", "proof", "failure"] },
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
}
