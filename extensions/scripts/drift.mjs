// Fails in CI, and only warns elsewhere, when the installed pi or Claude Code differs from the version the types were pinned to.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

const pkg = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
const installed = (cmd) => {
  try {
    return execFileSync(cmd, ["--version"], { encoding: "utf8" }).match(/\d+\.\d+\.\d+\S*/)?.[0];
  } catch {
    return undefined;
  }
};
const checks = [
  [
    "pi",
    pkg.devDependencies["@earendil-works/pi-coding-agent"],
    "pin @earendil-works/pi-coding-agent and @earendil-works/pi-tui to it (npm i -D --save-exact)",
  ],
  [
    "claude",
    pkg.claudeCodeTypes,
    'delete extensions/claude/.claude-plugin/types, run `npm run typecheck:claude` to regenerate, then set "claudeCodeTypes" in extensions/package.json',
  ],
];
let failed = false;
for (const [cmd, pinned, fix] of checks) {
  const have = installed(cmd);
  if (!have) console.log(`drift: ${cmd} not installed, skipping`);
  else if (have !== pinned) {
    const text = `drift: ${cmd} ${have} is installed but the types are pinned to ${pinned}; ${fix}`;
    // A local auto-update must not block a push; only CI, which installs the pin, fails.
    if (process.env.CI) {
      console.error(text);
      failed = true;
    } else console.warn(`warning: ${text}`);
  } else console.log(`drift: ${cmd} ${have} ok`);
}
process.exit(failed ? 1 : 0);
