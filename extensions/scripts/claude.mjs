// Runs a Claude Code check on the mod. Without the claude CLI it skips with a message, and in CI
// it fails, since CI installs the pinned CLI. Claude Code writes the 'claude-code' types into
// .claude-plugin/types only when a logged-in session loads the mod.
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync } from "node:fs";

const mod = new URL("../claude/", import.meta.url).pathname;
const run = (cmd, args, opts = {}) =>
  spawnSync(cmd, args, { stdio: "inherit", ...opts }).status ?? 1;

try {
  execFileSync("claude", ["--version"], { stdio: "ignore" });
} catch {
  if (process.env.CI) {
    console.error("claude CLI not found; CI installs it, so the Claude Code check fails");
    process.exit(1);
  }
  console.log("claude CLI not found, skipping the Claude Code check");
  process.exit(0);
}

const [mode] = process.argv.slice(2);
if (mode === "validate") process.exit(run("claude", ["plugin", "validate", mod]));
if (mode === "test") process.exit(run("claude", ["plugin", "test", mod]));
if (mode === "tsc") {
  const types = `${mod}.claude-plugin/types/claude-code/index.d.ts`;
  if (!existsSync(types)) {
    console.log("generating the Claude Code types by loading the mod once");
    run("claude", ["-p", "ok", "--model", "haiku", "--plugin-dir", mod], { stdio: "ignore" });
  }
  if (!existsSync(types)) {
    const why = process.env.ANTHROPIC_API_KEY
      ? "the mod did not load"
      : "loading the mod needs a login or ANTHROPIC_API_KEY";
    console.log(`Claude Code types unavailable (${why}), skipping the type check`);
    process.exit(0);
  }
  process.exit(run("npx", ["tsc", "--noEmit", "-p", mod]));
}
console.error("usage: claude.mjs validate|test|tsc");
process.exit(2);
