# Guide: design

`design.md` is optional. Most changes do not need it.

## When to write it

Write it only for one of these:

- A cross-cutting change that touches several capabilities.
- A new dependency.
- A migration of existing data or files.
- A security concern.
- A performance limit.

For any other change, skip the file. Put the reason for a choice in the task or the proposal instead.

## What goes in

- Decisions, each with the reason for it.
- Constraints that the work must respect.
- Alternatives that you rejected, in one line each, when the reason is not obvious.

## What stays out

- Keep existing code out; read the code instead, because a copy goes stale and misleads the next agent.
- Keep requirement text out; point to the spec by id, as in `see REQ-k3f9m2`.
- Keep steps of the work out; write those as tasks.

## Size

At most 100 lines. A design that grows past that mixes in a description of the code. Cut it to the decisions.

## Example

<!-- example -->
# Design

## Decisions
- Keep the tail in the proof event, not in a side file. The ledger is the one place that a reader checks, and a side file can go missing. See REQ-new1.
- Cut at 20 lines. One test failure fits, and a ledger line stays small.

## Constraints
- The ledger reads a line whole, so keep the tail under 4 KB.
<!-- /example -->

## Write it with rvow

Pipe the file to `rvow put <change> design`; the hook denies a direct write.
