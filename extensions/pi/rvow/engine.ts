// The engine client: one pi session sends its hooks to the `rvow daemon` that serves its rvow
// home, instead of starting `rvow hook` per call, and asks the host program that draws rvow
// questions to put them to the user on its own screen. Every call the engine cannot answer falls
// back to `rvow hook`, and every question no host draws falls back to the extension's own dialog.
import http from "node:http";
import { isRecord } from "./formats.ts";
import { FAST_MS, run } from "./tools.ts";

/** How many engine calls may fail in a row before the rest of the session runs `rvow hook`. */
const FAILS_BEFORE_OFF = 3;
/** The status the engine answers a call from another environment with. */
const MISMATCH = 409;

/** The hello `rvow daemon --print-hello` prints, sent back with every call of the session. */
export type Hello = {
  proto_major?: number;
  proto_minor?: number;
  build?: string;
  env?: string;
  ui?: string;
  [field: string]: unknown;
};

/** Where the engine of this session listens, and the hello its calls carry. */
export type Engine = { socket: string; hello: Hello };

/** The question of the agent that a host program can put to the user. */
export type AskKind = "accept" | "confirm" | "archive";

/** What the host did with a question: `no-host` says no program draws rvow questions. */
export type AskOutcome = "done" | "declined" | "failed" | "waiting" | "cancelled" | "no-host";

/** The host's answer: what it did, and the text the agent reads as the tool result. */
export type AskAnswer = { outcome: AskOutcome; text: string };

const OUTCOMES = new Set<string>(["done", "declined", "failed", "waiting", "cancelled", "no-host"]);

/** The one JSON line `rvow daemon --print-hello` prints, or undefined when `out` holds no engine. */
export function parseHello(out: string): Engine | undefined {
  let parsed: unknown;
  try {
    parsed = JSON.parse(out);
  } catch {
    return undefined;
  }
  if (!isRecord(parsed)) return undefined;
  const { socket, hello } = parsed;
  if (typeof socket !== "string" || !socket || !isRecord(hello)) return undefined;
  return { socket, hello };
}

/** Run `rvow daemon --print-hello` once: the engine this process can call, or undefined when it runs none. */
export async function readHello(cwd: string, timeoutMs = FAST_MS): Promise<Engine | undefined> {
  const r = await run(["daemon", "--print-hello"], cwd, "", timeoutMs);
  return r?.code === 0 ? parseHello(r.out) : undefined;
}

/** What one request to the engine socket answered. */
type Reply = { status: number; body: string };

/** What one `/v1/hook` call gave back. */
type Answer =
  /** The engine ran it: `output` is what `rvow hook` would print, empty when it prints nothing. */
  | { kind: "ran"; output: string }
  /** The engine runs in another environment or speaks another protocol. */
  | { kind: "mismatch" }
  /** No usable engine answered. */
  | { kind: "failed" };

/**
 * One JSON body posted to `path` on the engine socket. Never rejects: undefined means no engine
 * answered. A closed connection, an aborted signal and the deadline all answer the caller, so a
 * call never waits longer than the process path would.
 */
function post(
  engine: Engine,
  path: string,
  body: string,
  timeoutMs: number,
  signal?: AbortSignal,
): Promise<Reply | undefined> {
  return new Promise((resolve) => {
    let done = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let cancel = () => {};
    const finish = (reply?: Reply) => {
      if (done) return;
      done = true;
      if (timer !== undefined) clearTimeout(timer);
      signal?.removeEventListener("abort", cancel);
      resolve(reply);
    };
    let request: http.ClientRequest;
    try {
      request = http.request({
        socketPath: engine.socket,
        path,
        method: "POST",
        headers: {
          host: "rvow",
          "content-type": "application/json",
          "content-length": Buffer.byteLength(body),
        },
      });
    } catch {
      return finish();
    }
    cancel = () => {
      request.destroy();
      finish();
    };
    timer = setTimeout(cancel, timeoutMs);
    signal?.addEventListener("abort", cancel, { once: true });
    if (signal?.aborted) cancel();
    request.on("error", () => finish());
    request.on("response", (res) => {
      let text = "";
      res.setEncoding("utf8");
      res.on("data", (chunk: string) => {
        text += chunk;
      });
      res.on("error", () => finish());
      res.on("end", () => finish({ status: res.statusCode ?? 0, body: text }));
    });
    request.end(body);
  });
}

/**
 * One hook call on the engine socket. The input crosses as the caller wrote it, so the engine hands
 * the hook the same bytes `rvow hook` reads from stdin.
 */
async function callHook(
  engine: Engine,
  event: string,
  input: string,
  timeoutMs: number,
  signal?: AbortSignal,
): Promise<Answer> {
  const body = `{"hello":${JSON.stringify(engine.hello)},"call":{"harness":"claude","event":${JSON.stringify(event)},"input":${input}}}`;
  const reply = await post(engine, "/v1/hook", body, timeoutMs, signal);
  // 403 is a process the engine does not trust and every other status is its own refusal: run `rvow hook`.
  if (reply?.status !== 200) return { kind: "failed" };
  let parsed: unknown;
  try {
    parsed = JSON.parse(reply.body);
  } catch {
    return { kind: "failed" };
  }
  if (!isRecord(parsed)) return { kind: "failed" };
  const ran = isRecord(parsed.reply) ? parsed.reply : undefined;
  if (ran && (typeof ran.output === "string" || ran.output === null))
    return { kind: "ran", output: typeof ran.output === "string" ? ran.output : "" };
  return parsed.mismatch !== null && parsed.mismatch !== undefined
    ? { kind: "mismatch" }
    : { kind: "failed" };
}

/**
 * The engine of one pi session: the hello its calls carry, how many calls in a row failed, and
 * whether the session may use `rvow hook` only. A mismatch with the engine, or three failed calls
 * in a row, ends the engine for the rest of the session.
 */
export function createEngineSession() {
  let engine: Engine | undefined;
  let fails = 0;
  let off = false;
  return {
    /** Begin a session: no engine yet, and no failure remembered. */
    reset(): void {
      engine = undefined;
      fails = 0;
      off = false;
    },
    /** Keep the engine `rvow daemon --print-hello` named for this session. */
    use(found: Engine): void {
      engine = found;
      fails = 0;
      off = false;
    },
    /** One hook through the engine: what `rvow hook` would print, or undefined to run it as a process. */
    async hook(
      event: string,
      input: string,
      timeoutMs: number,
      signal?: AbortSignal,
    ): Promise<{ output: string } | undefined> {
      // `session-start` records the harness process the engine trusts and starts the engine, so it
      // always runs as a process. An aborted session asks for nothing.
      const found = off ? undefined : engine;
      if (!found || event === "session-start" || signal?.aborted) return undefined;
      const answer = await callHook(found, event, input, timeoutMs, signal);
      if (answer.kind === "ran") {
        fails = 0;
        return { output: answer.output };
      }
      if (answer.kind === "mismatch") {
        off = true;
        return undefined;
      }
      fails += 1;
      if (fails >= FAILS_BEFORE_OFF) off = true;
      return undefined;
    },
    /**
     * Ask the host program to put one question to the user. Undefined means no host draws rvow
     * questions, or the call did not work: the caller shows its own dialog exactly as before.
     */
    async ask(
      kind: AskKind,
      change: string,
      cwd: string,
      session: string | null,
      timeoutMs: number,
      signal?: AbortSignal,
    ): Promise<AskAnswer | undefined> {
      const found = off ? undefined : engine;
      if (!found || signal?.aborted) return undefined;
      const body = JSON.stringify({ hello: found.hello, kind, change, cwd, session });
      const reply = await post(found, "/v1/ask", body, timeoutMs, signal);
      // 409 Conflict is the engine of another environment, exactly as a hook mismatch is: the rest
      // of the session runs `rvow hook` and every question goes to the dialog below.
      if (reply?.status === MISMATCH) off = true;
      // A question the engine never answered says nothing about its hook path, so it counts as no
      // failure at all: the session keeps its engine for the hooks that follow.
      if (reply?.status !== 200) return undefined;
      let parsed: unknown;
      try {
        parsed = JSON.parse(reply.body);
      } catch {
        return undefined;
      }
      if (!isRecord(parsed) || typeof parsed.text !== "string") return undefined;
      const outcome = parsed.outcome;
      if (typeof outcome !== "string" || !OUTCOMES.has(outcome)) return undefined;
      return outcome === "no-host"
        ? undefined
        : { outcome: outcome as AskOutcome, text: parsed.text };
    },
  };
}
