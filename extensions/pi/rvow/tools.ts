import { spawn } from "node:child_process";
export const BIN = process.env.RVOW_BIN || "rvow";
export const FAST_MS = 10_000;
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

/** The wait for `rvow accept`, which runs no verify command. */
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
