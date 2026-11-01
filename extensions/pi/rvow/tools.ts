import { spawn } from "node:child_process";
export const BIN = process.env.RVOW_BIN || "rvow";
/** The wait for a hook call. A test sets RVOW_FAST_MS to see a timeout in less than a second. */
export const FAST_MS = Number(process.env.RVOW_FAST_MS) || 10_000;
export const SLOW_MS = 60_000;
/** `rvow archive` runs the gate checks, and rvow bounds each check with its own
 *  timeout, so the wait only guards against a hung binary. */
export const GATE_MS = 60 * 60_000;

let warned = false;

export function run(
  args: string[],
  cwd: string,
  stdin: string,
  timeoutMs: number,
  signal?: AbortSignal,
): Promise<{ code: number; out: string } | undefined> {
  return new Promise((resolve) => {
    let out = "";
    let done = false;
    const finish = (v?: { code: number; out: string }) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      signal?.removeEventListener("abort", cancel);
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
    let cancelled = false;
    const cancel = () => {
      cancelled = true;
      child?.kill("SIGKILL");
    };
    // A killed run says why, so the caller never sees an empty result.
    const timer = setTimeout(() => {
      out += `${out && !out.endsWith("\n") ? "\n" : ""}rvow ${args.join(" ")}: stopped after ${Math.round(timeoutMs / 1000)} s with no result; run it in a terminal\n`;
      cancel();
    }, timeoutMs);
    let child: ReturnType<typeof spawn> | undefined;
    try {
      child = spawn(BIN, args, { cwd, stdio: ["pipe", "pipe", "ignore"] });
    } catch {
      return fail();
    }
    signal?.addEventListener("abort", cancel, { once: true });
    if (signal?.aborted) cancel();
    child.on("error", fail);
    child.stdout?.on("data", (d: Buffer) => (out += d));
    child.on("close", (code) => finish({ code: cancelled ? 1 : (code ?? 1), out }));
    child.stdin?.on("error", () => {});
    child.stdin?.end(stdin);
  });
}

/** What the user did in this session, sent with every pre-tool-use call so the hook can tell the
 *  user's decisions from the agent's. Only user-driven commands and dialog answers add to it. */
export type UserMade = {
  since: number;
  changes: string[];
  yolo: { tier: string; at: number; until: number }[];
};

export function createUserMade(): UserMade {
  return { since: Date.now(), changes: [], yolo: [] };
}

/** The `key: <ms>` line of `rvow yolo` output: the start `at` or the end `until` of the switch it
 *  sealed, as the file holds them. An indented or repeated line is not one of the top-level fields. */
function yoloMsField(stdout: string, key: "at" | "until"): number | undefined {
  const found = [...stdout.matchAll(new RegExp(`^${key}: (\\d+)$`, "gm"))];
  const [only] = found;
  if (found.length !== 1 || !only?.[1]) return undefined;
  const ms = Number(only[1]);
  return Number.isSafeInteger(ms) ? ms : undefined;
}

/** The times `rvow yolo <tier>` printed for the switch it sealed, or undefined when the output has
 *  no `at` or no `until`: off, a tier already on outside this run, or a run that failed. */
function yoloTimes(stdout: string): { at: number; until: number } | undefined {
  const at = yoloMsField(stdout, "at");
  const until = yoloMsField(stdout, "until");
  if (at === undefined || until === undefined) return undefined;
  return { at, until };
}

/** Record that the user switched a yolo tier on, with the times `rvow yolo` itself printed: the
 *  hook counts the tier only when `at` and `until` equal the ones in the file. Output with no `at`
 *  or no `until` records nothing, so the unseen yolo dialog asks instead. */
export function recordYolo(made: UserMade, tier: string, stdout: string): void {
  const times = yoloTimes(stdout);
  if (times) made.yolo.push({ tier, at: times.at, until: times.until });
}

/** A yolo tier that the hook reported it does not count, from the top-level `unseen_yolo` field of
 *  a hook reply: the tier of the file, when it was switched on and when it ends. */
export type UnseenYolo = { tier: string; at: number; until: number };

/** The unseen yolo tier of a hook reply, or undefined when the reply has none. */
export function unseenYolo(reply: unknown): UnseenYolo | undefined {
  const field = (reply as { unseen_yolo?: unknown } | undefined)?.unseen_yolo;
  if (!field || typeof field !== "object") return undefined;
  const { tier, at, until } = field as Record<string, unknown>;
  if (typeof tier !== "string" || !tier) return undefined;
  if (typeof at !== "number" || !Number.isFinite(at)) return undefined;
  if (typeof until !== "number" || !Number.isFinite(until)) return undefined;
  return { tier, at, until };
}

/** Record the unseen tier as the user's, with the file's own times, which is what the hook counts. */
export function keepYolo(made: UserMade, on: UnseenYolo): void {
  made.yolo.push({ tier: on.tier, at: on.at, until: on.until });
}

/** The two answers of the unseen yolo dialog: keep the tier, or turn it off. */
export const UNSEEN_YOLO_OPTIONS = ["Keep", "Turn off"];

/** The time of day a yolo run ends, for the unseen yolo question. */
function clock(at: number): string {
  const d = new Date(at);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/** The question the unseen yolo dialog asks: only a dialog or a slash command counts as the user's. */
export function unseenYoloPrompt(on: UnseenYolo): string {
  return `yolo ${on.tier} is on until ${clock(on.until)}, but rvow did not see you switch it on (only /rvow-yolo or the yolo dialog count). If you did not run \`rvow yolo\`, the agent did. Keep it or turn it off?`;
}

/** The changes that the amendments `ids` of a `rvow confirm` listing belong to, or every listed one without `ids`. */
export function amendmentChanges(listing: string, ids?: string[]): string[] {
  const found = new Set<string>();
  for (const line of listing.split("\n")) {
    const head = /^(AMD-[\w-]+) for (\S+)/.exec(line);
    if (head && (!ids || ids.includes(head[1] ?? ""))) found.add(head[2] ?? "");
  }
  return [...found];
}

/** Add `ids` to the changes the user approved. */
export function recordChanges(made: UserMade, ids: string[]): void {
  for (const id of ids) if (!made.changes.includes(id)) made.changes.push(id);
}

/** The wait for `rvow accept`, which runs no verify command. */
/** Cut text bound for the model at `max` chars, ending in `...`. */
export function cut(text: string, max: number): string {
  return text.length <= max ? text : `${text.slice(0, max)}...`;
}

export const ACCEPT_MS = 600_000;

/** The most checklist lines the accept dialog shows before it cuts the rest. */
export const ACCEPT_DIALOG_LINES = 10;

/** The dialog's share of `review`: the first lines in full, or the first
 *  10 lines and one line that names `rvow review <id>` and the cut count. */
export function shortReview(review: string, change: string): string {
  const lines = review.split("\n");
  if (lines.at(-1) === "") lines.pop();
  if (lines.length <= ACCEPT_DIALOG_LINES) return review;
  const cut = lines.length - ACCEPT_DIALOG_LINES;
  return [
    ...lines.slice(0, ACCEPT_DIALOG_LINES),
    `... ${cut} more lines: run \`rvow review ${change}\``,
  ].join("\n");
}

/** What the agent hears when the user does not accept: the plan stays, and the slash command starts it. */
export function declinedAcceptReason(change: string): string {
  return `The user did not accept ${change}; it stays a plan.`;
}

/** A repeat accept while the plan is unchanged: the old reason, and the slash command the user can run. */
export function repeatAcceptResult(reason: string, change: string): string {
  return `${reason} The user can run \`/rvow-accept ${change}\` to start it.`;
}

export type AcceptMemory = { review: string; reason: string; declined: boolean };

/** The review text and reason of each plan's last failed or declined accept.
 *  One store per tool registration, so the memory lives as long as the session. */
export function createAcceptMemory() {
  const seen = new Map<string, AcceptMemory>();
  return {
    /** The remembered accept when `review` matches the plan's last failed or declined one, else null. */
    remembered(change: string, review: string): AcceptMemory | null {
      const found = seen.get(change);
      return found && found.review === review ? found : null;
    },
    /** Remember a failed or declined accept, so a repeat with the same review asks nothing. */
    remember(change: string, review: string, reason: string, declined: boolean): void {
      seen.set(change, { review, reason, declined });
    },
    /** Forget a plan's remembered accept, so the next call asks as usual. */
    forget(change: string): void {
      seen.delete(change);
    },
  };
}

export type AcceptMemoryStore = ReturnType<typeof createAcceptMemory>;
