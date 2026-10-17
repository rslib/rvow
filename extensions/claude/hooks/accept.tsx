/** Pure code only: every function that takes `$` stays in register.tsx. */

/** The wait for one `rvow` command, `accept` included, which runs no verify command. */
export const ACCEPT_MS = 600_000

/** The most checklist lines the accept dialog shows before it cuts the rest. */
export const ACCEPT_DIALOG_LINES = 10

/** The dialog's share of `review`: the first lines in full, or the first
 *  10 lines and one line that names `rvow review <id>` and the cut count. */
export function shortReview(review: string, change: string): string {
  const lines = review.split('\n')
  if (lines.at(-1) === '') lines.pop()
  if (lines.length <= ACCEPT_DIALOG_LINES) return review
  const cut = lines.length - ACCEPT_DIALOG_LINES
  return [
    ...lines.slice(0, ACCEPT_DIALOG_LINES),
    `... ${cut} more lines: run \`rvow review ${change}\``,
  ].join('\n')
}

/** What the agent hears when the user does not accept: the plan stays, and the slash command starts it. */
export function declinedAcceptReason(change: string): string {
  return `The user did not accept ${change}; it stays a plan.`
}

/** A repeat accept while the plan is unchanged: the old reason, and the slash command the user can run. */
export function repeatAcceptResult(reason: string, change: string): string {
  return `${reason} The user can run \`/rvow-accept ${change}\` to start it.`
}
