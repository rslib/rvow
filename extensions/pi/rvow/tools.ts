import { spawn } from "node:child_process";
export const BIN = process.env.RVOW_BIN || "rvow";
export const FAST_MS = 10_000;
export const SLOW_MS = 60_000;

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
    const timer = setTimeout(cancel, timeoutMs);
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
