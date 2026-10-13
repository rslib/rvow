const FAILED_LINES = 12;

export function failedPrompt(argv: string[], out: string): string {
  const lines = out.split("\n").filter((l) => l.trim());
  return [
    `I tried \`rvow ${argv.join(" ")}\` and it failed:`,
    ...lines.slice(0, FAILED_LINES),
    "Fix what it names first. If nothing changed, do not ask me to run it again.",
  ].join("\n");
}

import { YOLO_TIERS } from "./typed-tools.ts";

export const REFINE_STEPS =
  "Follow `rvow guide proposal`, `rvow guide spec` and `rvow guide tasks`. Ask me about open points with the rvow_ask tool before changing scope. Then cut anything the guides rule out of the proposal and tasks so `rvow check` passes, and tell me what changed.";

export function refinePrompt(plans: { id: string; title: string }[]): string {
  const [p] = plans;
  if (plans.length === 1 && p) {
    return `Review and refine the rvow plan ${p.id} (${p.title}). Run \`rvow show ${p.id}\` and \`rvow check ${p.id}\`. ${REFINE_STEPS}`;
  }
  return [
    "Review and refine these rvow plans, one plan at a time, in this order:",
    ...plans.map((x) => `- ${x.id} (${x.title})`),
    `For each plan: run \`rvow show <id>\` and \`rvow check <id>\`. ${REFINE_STEPS}`,
  ].join("\n");
}

export function decisionPrompt(argv: string[], out: string): string | null {
  const [verb, id] = argv;
  if (verb === "archive") return `I archived ${id}. The change is closed.`;
  if (verb === "accept") {
    const next = out.split("\n").find((l) => l.startsWith("next:"));
    const trimmed = next?.endsWith(".") ? next.slice(0, -1) : next;
    return `I accepted ${id}.${trimmed ? ` ${trimmed}.` : ""} Continue with it.`;
  }
  if (verb === "yolo") {
    if (id === "off") return "I switched yolo off. The usual rules apply again.";
    if (id && id in YOLO_TIERS)
      return `I switched on yolo ${id}: ${YOLO_TIERS[id as keyof typeof YOLO_TIERS]}. It ends by itself, and the gate still checks at push.`;
    return null;
  }
  const ids = [...new Set(`${argv.join(" ")}\n${out}`.match(/AMD-[\w-]+/g) ?? [])].join(", ");
  if (argv.includes("--accept")) return `I accepted the plan edit ${ids}. Continue the tasks.`;
  if (argv.includes("--reject"))
    return `I rejected the plan edit ${ids}. Undo it, or amend it again with a different edit.`;
  return null;
}
