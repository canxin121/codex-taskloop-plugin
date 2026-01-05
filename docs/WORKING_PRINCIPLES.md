# Working Principles: codex-taskloop-plugin

This document describes the internal behavior of the Taskloop task loop for Codex.
It matches the current code in `src/main.rs` and `src/bin/hook.rs`.

## Components

1) MCP server (`codex-taskloop-plugin`)
- Creates task state files.
- Maintains `meta.json` index.
- Exposes MCP tools for task control.

2) Stop hook (`codex-taskloop-plugin-hook`)
- Runs on every stop attempt.
- Decides whether to block or approve stopping.

Installation scopes:
- User-level: stop hook registered in `config.toml` under `stop_hooks`.
- Project-level: stop hook stored in `.codex/hooks/hooks.json`.

## Storage layout

Two storage roots:
- Project (project-level): `<project>/.codex/task_loop/`
- User (user-level): `$CODEX_HOME/task_loop/` (fallback `~/.codex/task_loop/`)

Each storage root contains:
- `meta.json` (task index)
- `meta.lock` (lock for meta updates)
- `<task_dir>/state.md`
- `<task_dir>/history.jsonl`
- `<task_dir>/task.lock`

`task_dir` is a random alphanumeric directory name.

## Task identity

A task is uniquely identified by:
- Project storage: `task_name` (unique within a project)
- User storage: `task_name + project_path`

Constraints:
- `task_name` is required, single-line, <= 30 characters.
- `project_path` is an absolute path.

`task_name` can be auto-generated from the first non-empty prompt line.

## meta.json schema

`meta.json` stores a compact index of tasks:

```json
{
  "schema_version": 1,
  "tasks": [
    {
      "task_name": "Fix tests",
      "task_dir": "abc123xyz",
      "project_path": "/abs/project",
      "status": "in_progress",
      "iteration": 2,
      "max_iterations": 10,
      "last_event": "loop",
      "started_at": "2026-01-05T00:00:00Z",
      "updated_at": "2026-01-05T00:10:00Z"
    }
  ]
}
```

Notes:
- `project_path` is stored for both project and user tasks.
- `status` is derived from state/history and updated by the hook.

## State file format

State file is Markdown with YAML frontmatter:

```markdown
---
schema_version: 1
task_name: Fix tests
project_path: /abs/project
active: true
iteration: 1
max_iterations: 0
completion_promise: "DONE"
completion_matcher: "exact"
history_limit: 200
started_at: "2026-01-05T00:00:00Z"
updated_at: "2026-01-05T00:00:00Z"
---

Fix failing tests. Output <promise>DONE</promise> when all tests pass.
```

## History file format

History is JSONL (one JSON object per line). Each entry includes:
- `ts`, `task_name`, `project_path`, `task_dir`
- `iteration`, `max_iterations`
- `completion_promise` / `completion_matcher`
- `prompt_preview`
- `state_file` / `history_file`
- `event` (e.g. `start`, `loop`, `resume`, `paused`, `promise_matched`, `max_iterations`, `invalid_state`, `invalid_matcher`)

History is pruned to the most recent `history_limit` entries.

## Stop hook task selection

On each stop attempt, the hook:
1. Loads `meta.json` from project and user roots.
2. Filters tasks by current `project_path`.
3. Keeps only tasks with existing `state.md`.
4. Sorts candidates by `updated_at`/`started_at` (fallback to state file mtime) and iterates most recent first.

This prevents unrelated tasks from other projects from interfering.

## Stop hook decision flow

For the selected task:
1. Read and validate state.
2. If `active=false`: record `paused` and move to the next candidate.
3. If `iteration >= max_iterations` (and max_iterations > 0): delete state, record `max_iterations`, approve stop.
4. Extract last assistant message (`last_agent_message` or `rollout_path`).
5. If `<promise>...</promise>` matches: delete state, record `promise_matched`, approve stop.
6. If the matcher is invalid: delete state, record `invalid_matcher`, approve stop.
7. Otherwise: increment iteration, write state, record `loop`, block stop with original prompt.

If state is malformed, the hook deletes it, records `invalid_state`, and moves on.
If no active task yields a decision, the hook approves stop.

## MCP tool behaviors

- `task_loop`: creates task, writes state/history, updates `meta.json`.
- `task_list`: reads `meta.json`, merges state/history to compute status.
- `task_rename`: updates `meta.json`, updates state/history when state exists.
- `task_resume`: sets `active=true` if state exists; fails if task is completed.
- `task_delete`: removes task directory and meta entry.

## Storage scope enforcement

- User-level install: `storage=project` or `storage=user` allowed.
- Project-level install: only `storage=project` allowed.
  Enforced by `TASKLOOP_STORAGE_SCOPE=project-only`.

## Reliability

- File locking on `meta.lock` and `task.lock`.
- Atomic writes for `state.md` and history pruning.
- Graceful handling of malformed state/history.
