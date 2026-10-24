/** Pure code only: every function that takes `$` stays in register.tsx. */

/** Where the engine of this home answers, and the two routes its socket serves. */
export const ENGINE_ORIGIN = 'http://rvow'
export const HOOK_ROUTE = '/v1/hook'
export const ASK_ROUTE = '/v1/ask'

/** The hello an engine answers in its own environment, as `rvow daemon --print-hello` printed it. */
export type EngineHello = Record<string, unknown>

/** The engine of one session: the socket `rvow daemon --print-hello` named, and its hello. */
export type Engine = { socket: string; hello: EngineHello }

/** How many engine failures in a row retire the engine for the rest of the session. */
export const ENGINE_FAILURES = 3

/** The events that may go through the engine; `session-start` never does (see register.tsx). */
export type EngineEvent =
  | 'pre-tool-use'
  | 'post-tool-use'
  | 'stop'
  | 'status'
  | 'view'
  | 'lease-bind'
  | 'lease-end'

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === 'object' && v !== null && !Array.isArray(v)

/**
 * The one line `rvow daemon --print-hello` printed, as the engine of this session, or `null` when the
 * line is missing, is not JSON or does not name a socket and a hello.
 */
export function parseHello(text: string): Engine | null {
  const line = text.trim()
  if (!line) return null
  let parsed: unknown
  try {
    parsed = JSON.parse(line)
  } catch {
    return null
  }
  if (!isRecord(parsed)) return null
  const { socket, hello } = parsed
  if (typeof socket !== 'string' || !socket || !isRecord(hello)) return null
  return { socket, hello }
}

/** The body of one hook call: this session's hello, and the call whose `input` the hook reads. */
export function hookBody(
  hello: EngineHello,
  event: EngineEvent,
  input: Record<string, unknown>,
): string {
  return JSON.stringify({ hello, call: { harness: 'claude', event, input } })
}

/** The questions a host program draws for the user; `confirm` names no change, so it may be empty. */
export type AskKind = 'accept' | 'confirm' | 'archive'

/** What a host answered for one question, `no-host` being that none draws rvow questions. */
export type AskOutcome = 'done' | 'declined' | 'failed' | 'waiting' | 'cancelled' | 'no-host'

const ASK_OUTCOMES: readonly AskOutcome[] = [
  'done',
  'declined',
  'failed',
  'waiting',
  'cancelled',
  'no-host',
]

/** The body of one question for the host: `change` empty when the call names none. */
export function askBody(
  hello: EngineHello,
  kind: AskKind,
  change: string,
  cwd: string,
  session: string | null,
): string {
  return JSON.stringify({ hello, kind, change, cwd, session })
}

/**
 * What one hook answer of the engine says: the hook's own output (`out` missing when it printed
 * nothing), that the engine works in another environment or protocol (`mismatch`, so the session
 * keeps to `rvow hook`), or that the answer is not one this client can read (`bad`, an engine
 * failure: the caller falls back to `rvow hook` for the call).
 */
export type HookAnswer =
  | { kind: 'out'; out: Record<string, unknown> | undefined }
  | { kind: 'mismatch' }
  | { kind: 'bad' }

/** Reads the body of a `/v1/hook` answer; the output is the text `rvow hook` would have printed. */
export function readHookAnswer(text: string): HookAnswer {
  let parsed: unknown
  try {
    parsed = JSON.parse(text)
  } catch {
    return { kind: 'bad' }
  }
  if (!isRecord(parsed)) return { kind: 'bad' }
  if (parsed.mismatch !== null && parsed.mismatch !== undefined) return { kind: 'mismatch' }
  const reply = parsed.reply
  if (!isRecord(reply)) return { kind: 'bad' }
  const output = reply.output
  if (output === null || output === undefined) return { kind: 'out', out: undefined }
  if (typeof output !== 'string') return { kind: 'bad' }
  let out: unknown
  try {
    out = JSON.parse(output)
  } catch {
    return { kind: 'bad' }
  }
  // The process path answers `{}` for text that is JSON but not an object; so does this one.
  return { kind: 'out', out: isRecord(out) ? out : undefined }
}

/** What a host answered for one question: the outcome, and the text the agent hears. */
export type HostAsk = { outcome: AskOutcome; text: string }

/** Reads the body of a `/v1/ask` answer; `null` means the caller shows the harness dialog. */
export function readAskAnswer(text: string): HostAsk | null {
  let parsed: unknown
  try {
    parsed = JSON.parse(text)
  } catch {
    return null
  }
  if (!isRecord(parsed)) return null
  const { outcome, text: answer } = parsed
  if (typeof outcome !== 'string' || typeof answer !== 'string') return null
  if (!ASK_OUTCOMES.includes(outcome as AskOutcome)) return null
  return { outcome: outcome as AskOutcome, text: answer }
}
