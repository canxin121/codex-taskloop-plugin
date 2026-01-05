# Simulated Run and Conversation Flow

This is a narrative example (not a real transcript) showing how a task loop
runs inside a single Codex session.

## Who makes the decision

The `decision` field is produced by the Stop hook program, not the model.
The model only outputs text; the hook reads state + the last assistant message
and returns:
- `decision=block` to continue, or
- `decision=approve` to stop.

## Scenario

Goal: fix failing tests and stop when all tests pass.
Assumptions:
- Stop hook is installed.
- MCP server is registered.
- Optional defaults exist in `.codex/task-loop.config.toml`.

## Simulated run (conversation style)

```
User: Start a loop to fix failing tests. Stop when DONE.
Codex: (calls task_loop)
Tool:  Taskloop task started.
```

Behind the scenes:
- MCP creates a task under the chosen storage root.
- `meta.json` is updated with the new task.
- `history.jsonl` records `start`.

State file (simplified):
```markdown
---
schema_version: 1
task_name: Fix failing tests
project_path: /abs/path/to/project
active: true
iteration: 1
max_iterations: 10
completion_promise: "DONE"
completion_matcher: "exact"
history_limit: 200
started_at: "2026-01-05T00:00:00Z"
updated_at: "2026-01-05T00:00:00Z"
---

Fix failing tests. Output <promise>DONE</promise> when all tests pass.
```

```
Codex: Runs tests, edits code, tries to stop.
Hook:  decision=block, reason=<same prompt>, systemMessage=iteration 2/10
```

Behind the scenes:
- Hook reads `<task_dir>/state.md`.
- No `<promise>DONE</promise>` found, so it blocks.
- `iteration` increments to 2, history records `loop`.
- `meta.json` is updated with the latest status/iteration.

```
Codex: Continues, fixes remaining failures, tries to stop.
Hook:  decision=block, reason=<same prompt>, systemMessage=iteration 3/10
```

Behind the scenes:
- Same prompt is injected again.
- Hook repeats the checks, increments iteration, records `loop`.

```
Codex: All tests passing. <promise>DONE</promise>
Hook:  decision=approve
Codex: Final response to user.
```

Behind the scenes:
- Hook detects `<promise>DONE</promise>` and matches it.
- `state.md` is removed, history records `promise_matched`.
- `meta.json` is updated to `completed`.

## Variations

- Unlimited loop: set `max_iterations = 0`.
- Case-insensitive promise: set `completion_matcher = "case_insensitive"`.
- Multiple tasks: start tasks with different `task_name` values.
