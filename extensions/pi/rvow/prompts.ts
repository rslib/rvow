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
  const ids = [...new Set(`${argv.join(" ")}\n${out}`.match(/AMD-[\w-]+/g) ?? [])].join(", ");
  if (argv.includes("--accept")) return `I accepted the plan edit ${ids}. Continue the tasks.`;
  if (argv.includes("--reject"))
    return `I rejected the plan edit ${ids}. Undo it, or amend it again with a different edit.`;
  return null;
}
