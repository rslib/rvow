# Guide: spec

Write one file per capability: `specs/<capability>.md` in the change. It holds deltas, one block per requirement.

## Look first

- Run `rvow spec list` to see every capability with its requirement ids and titles.
- Run `rvow show <id>` for each related requirement. Do not read whole files.
- Read a change's spec with `rvow show <change> spec:<name>`; replace one block with `rvow put <requirement id>`.
- Change an existing requirement before you add a near copy of it.

## Deltas

- `## ADD REQ-new1 <title>`: a new requirement. Number the placeholders new1, new2 and so on; rvow mints the real id at accept and rewrites every reference.
- `## CHANGE REQ-id <title>`: the full new text and scenarios. Do not send a partial edit.
- `## DEPRECATE REQ-id`, `## MOVE REQ-id` and `## REMOVE REQ-id`: the id only, with no text.

## One requirement

- One behavior per requirement.
- One EARS sentence with exactly one uppercase SHALL or MUST. Forms: `<system> SHALL ...`, `WHEN <event>, <system> SHALL ...`, `WHILE <state>, <system> SHALL ...`, `IF <condition>, THEN <system> SHALL ...`, `WHERE <feature>, <system> SHALL ...`, where `<system>` names the product, as the example names rvow.
- At most 60 words and 5 scenarios.
- Each scenario has an optional GIVEN, one WHEN and at least one THEN.
- Give concrete values in scenarios. Include one edge case, such as an empty input or a limit.
- Test each requirement: if the implementation can change without changing externally visible behavior, it does not belong in the spec. Write it in `design.md` or in a task.
- Name the visible behavior, not files or code.
- Write the number or the limit instead of vague words such as fast or several.

## Example

<!-- example -->
## ADD REQ-new1 Keep the output tail

WHEN a proof run fails, rvow SHALL store the last 20 lines of its output in the proof event.

### Scenario: A failed run keeps its tail
- GIVEN a verify command that prints 30 lines and exits 1
- WHEN `rvow prove` runs on that task
- THEN the proof event holds lines 11 to 30

### Scenario: A silent failure keeps no tail
- GIVEN a verify command that prints nothing and exits 1
- WHEN `rvow prove` runs on that task
- THEN the proof event holds 0 lines
<!-- /example -->
