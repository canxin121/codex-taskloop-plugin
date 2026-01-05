# codex-taskloop

Taskloop-style in-session looping for Codex using:
- a Stop hook (blocks stop and re-injects the same prompt), and
- an MCP server (creates/updates task state + meta index).

This is a generic mechanism for iterative loops inside a single Codex session.
It does not depend on Claude plugins.

Docs:
- `docs/WORKING_PRINCIPLES.md`
- `docs/SIMULATED_RUN.md`
- Chinese README: `README.zh.md`

## Architecture (one screen)

- **MCP server** (`codex-taskloop`) manages tasks, `meta.json`, and state/history files.
- **Stop hook** (`codex-taskloop-hook`) intercepts stop and decides whether to continue.

## Supported Features

Core loop:
- In-session Taskloop loop (same prompt re-injected on stop).
- Completion promises with `<promise>...</promise>` tags.
- Max-iteration guard (0 = unlimited).

Task model:
- Task identity = `task_name + project_path` (unique per project).
- `meta.json` index for tasks and progress.
- Per-task directory with `state.md` + `history.jsonl` + `task.lock`.

Control:
- List tasks (with status + last event).
- Rename task.
- Resume task (only if state still exists).
- Delete task (removes task directory + meta entry).

Storage:
- Local (project) and global (user) storage roots.
- User-level install supports local + global per tool call.
- Project-level install enforces local-only storage.

Reliability:
- File locking to avoid races.
- Atomic writes for state/history.
- Safe handling of malformed state files.

## Install

### 1) Build (skip if binaries already exist)

```bash
cargo build --release --manifest-path /path/to/codex-taskloop/Cargo.toml
```

### 2) User-level install (MCP + Stop hook)

This registers the MCP server globally and installs a global Stop hook via
`stop_hooks` in your Codex config.

```bash
/path/to/codex-taskloop/scripts/install-user.sh
```

If you set `CODEX_HOME`, it will be written into the MCP env so global storage
stays under that root.

User-level install supports both storage modes (local + global).
Storage is chosen per tool call (default: local).

### 3) Install into a project (Stop hook + MCP)

```bash
cd /path/to/your/project

# Optional: isolate config + global storage for testing
# export CODEX_HOME=/tmp/codex-home

/path/to/codex-taskloop/scripts/install.sh --project "$(pwd)"
```

What this does:
- Writes `.codex/hooks/hooks.json` pointing to the Stop hook binary.
- Registers the MCP server in `$CODEX_HOME/config.toml` (or `~/.codex/config.toml`).
- Injects `CODEX_HOME` into the MCP server environment (when set).
- Enforces local-only storage for this project (`storage=global` is rejected).

Tip: If you want both user-level and project-level installs at the same time,
use different MCP server names (`--name`) to avoid overwriting the same entry.

### MCP server names (why this matters)

Codex stores MCP servers by name in `config.toml`. If you install twice with
the same name, the second install replaces the first.

Example (keep both user-level + project-level):
```bash
# User-level (global)
/path/to/codex-taskloop/scripts/install-user.sh --name codex-taskloop-user

# Project-level (local-only)
/path/to/codex-taskloop/scripts/install.sh --project "$(pwd)" --name codex-taskloop-project
```

Then you can explicitly target the server in chat:
```
Use the MCP tool codex-taskloop-user.task_loop ... storage: "global"
Use the MCP tool codex-taskloop-project.task_loop ... storage: "local"
```

Optional flags:
- `--name <server>` (default: `codex-taskloop`)
- `--no-mcp` / `--no-hook` / `--no-build`

Verify registration:
```bash
codex mcp list
codex mcp get codex-taskloop
```

## Usage (two layers)

### A) User-facing usage (how you talk to Codex)

You do not call MCP tools directly. You just ask Codex to do it.

Examples to paste into the Codex chat:
```
Start a Taskloop task to fix failing tests. Use completion_promise DONE and
max_iterations 20. Store it locally.
```

```
Use the MCP tool task_loop with prompt:
"Fix tests and output <promise>DONE</promise> when all pass."
Set completion_promise: DONE, max_iterations: 20, storage: global.
```

Control requests:
```
List my active Taskloop tasks (global).
Rename the task to a shorter title.
Resume the paused task and continue.
Delete the old task when done.
```

### B) Tool-level usage (what Codex actually calls)

Start a task (local storage):
```
task_loop {
  prompt: "Fix the failing tests. Output <promise>DONE</promise> when all tests pass.",
  task_name: "Fix failing tests",
  completion_promise: "DONE",
  max_iterations: 20,
  storage: "local"
}
```

List tasks:
```
task_list { storage: "global", limit: 20, offset: 0 }
```

Resume / rename / delete:
```
task_resume { task_name: "Fix failing tests", storage: "local" }
task_rename { task_name: "Fix failing tests", new_name: "Fix tests", storage: "local" }
task_delete { task_name: "Fix tests", storage: "local" }
```

## Storage model

Two storage locations are supported:
- local (default): `.codex/task_loop/` inside the project
- global: `$CODEX_HOME/task_loop/` (falls back to `~/.codex/task_loop/`)

Notes:
- For `storage=global`, `project_dir` is optional in `task_list` (omit to list
  all global tasks); for other tools it is required.
- For `storage=local`, `project_dir` defaults to `CODEX_CWD` or current dir.

Scope rules:
- User-level install: `storage=local` and `storage=global` are both allowed.
- Project-level install: local only. Requests with `storage=global` are rejected.
  Storage scope is fixed at install time via `TASKLOOP_STORAGE_SCOPE=local-only`.

## Task lifecycle and stop rules

The Stop hook runs on every stop attempt:
1. Load `meta.json` from local and global roots.
2. Filter tasks by current `project_path`.
3. Pick the most recently updated active task.
4. If `max_iterations` reached (and > 0), approve stop and delete state.
5. If `<promise>...</promise>` matches (exact / case_insensitive / regex), approve and delete state.
6. Otherwise increment `iteration`, update `updated_at`, append history, and block stop.

## Files and layout

Storage root (local or global):
- `meta.json` (task index + progress)
- `meta.lock`
- `<task_dir>/state.md`
- `<task_dir>/history.jsonl`
- `<task_dir>/task.lock`

Local storage root:
- `.codex/task_loop/`

Global storage root:
- `$CODEX_HOME/task_loop/` (or `~/.codex/task_loop/`)

## Config defaults

Create `.codex/task-loop.config.toml`:

```toml
default_max_iterations = 0
default_completion_matcher = "exact"
history_limit = 200
```

Defaults apply when starting a task and when fields are missing in the state file.

## Troubleshooting

- Tools not available: ensure `codex mcp list` shows `codex-taskloop`.
- Global storage went to `~/.codex`: export `CODEX_HOME` before running `install.sh`
  and before launching `codex`.
- Task not continuing: check `.codex/hooks/hooks.json` points to the hook binary.
- `storage=global` rejected: you used project-level install (local-only). Use user-level install.
- Resume failed: the task is completed and its state file is gone.

## Uninstall

User-level MCP + Stop hook:
```bash
/path/to/codex-taskloop/scripts/uninstall-user.sh
```

Project-level install:
```bash
/path/to/codex-taskloop/scripts/uninstall.sh --project "$(pwd)"
```

## Development

```bash
cargo build --release --manifest-path /path/to/codex-taskloop/Cargo.toml
```

## MCP tool reference (full list)

Common options:
- `storage` (optional): `local` or `global` (default: `local`).
- `project_dir` (optional): project root; required for global resume/rename/delete.

Tools:
- `task_loop` (start a task)
  - `prompt` (required)
  - `task_name` (optional, <= 30 chars; auto-generated if missing)
  - `max_iterations` (optional, 0 = unlimited)
  - `completion_promise` (optional)
  - `completion_matcher` (optional: `exact`, `case_insensitive`, `regex`)
  - `history_limit` (optional, 0 disables pruning)
  - `project_dir` (optional)
- `task_list` (list tasks)
  - `limit` / `offset` (optional)
  - `project_dir` (optional filter for global storage)
- `task_resume` (resume task)
- `task_rename` (rename task)
- `task_delete` (delete task)
