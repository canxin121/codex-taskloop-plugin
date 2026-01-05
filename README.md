# codex-taskloop

Run a task loop inside a single Codex session so it keeps working until done.

## What it does
- Keeps working inside the same session until done
- Continues after a Codex reply unless a completion condition or max-iteration limit is met
- Manage tasks in chat: start, list, resume, rename, delete
- Task data supports user-level storage (user install only) and project-level storage

## Quick start

### Build
```bash
cargo build --release
```

### Install
User-level (recommended):
```bash
/path/to/codex-taskloop/scripts/install.sh --scope user
```

Windows (PowerShell):
```powershell
.\scripts\install.ps1 -Scope user
```

Project-level (project-only storage):
```bash
/path/to/codex-taskloop/scripts/install.sh --scope project --project "$(pwd)"
```

Binary detection and overrides:
- The install scripts look for `codex-taskloop`, `codex-taskloop-hook`, and `codex-taskloop-admin`
- Default search order: `<script_dir>/../target/release`, `<script_dir>/../bin`
- If not found and `cargo` is available, they build unless you pass `--no-build` / `-NoBuild`
- To use a custom location: `--bin-dir /path/to/dir` (PowerShell: `-BinDir`)

### Task data storage

#### Task definition
- Two core attributes: `task_name` (task name) and `project_path` (project path)
- Uniqueness: project storage uses `task_name` within a project; user storage uses `task_name + project_path`
- Structure: each storage root has `meta.json` (index), and each task directory has `state.md` (state) and `history.jsonl` (history)

#### User-level storage
- Available only with a user-level install
- Stored in `$CODEX_HOME/task_loop/` (fallback `~/.codex/task_loop/`)

#### Project-level storage
- Available in both user-level and project-level installs
- Stored in `<project>/.codex/task_loop/`

### Use it in chat
Describe what you want in natural language and Codex will choose the right tool and fill in the parameters. You can mention:
- Task goal and task name
- Completion condition (for example: “stop when it outputs a keyword”)
- Max iterations or history retention
- Project-level vs user-level storage (and the project path if needed)

Paste any of these into Codex:
```
Use project-level storage to start a Taskloop task to fix failing tests. Stop when it is done.
```

```
Use user-level storage to start a task named "Fix failing tests" with at most 10 iterations.
```

```
List the 10 most recent Taskloop tasks in project-level storage.
```

```
Show only the project-level tasks for the current project.
```

```
List user-level tasks for the project at /path/to/project.
```

```
Resume the paused "Fix failing tests" task from user-level storage (project path /path/to/project).
```

```
Rename the task "Fix failing tests" to "Fix tests" in project-level storage.
```

```
Delete the task "Fix tests" from project-level storage.
```

### Tools and parameters
Parameter notes:
- `storage`: `project` or `user`, default `project`
  - User-level install: `project` / `user` allowed
  - Project-level install: only `project` allowed
- `project_dir`: project path; defaults to `CODEX_CWD` or current directory
  - Required when `storage=user` and using `task_resume` / `task_rename` / `task_delete`
  - With `storage=user` and no `project_dir`: `task_list` returns all user-level tasks; `task_loop` uses the current project path
- `task_name`: single line, <= 30 chars (auto-generated from first prompt line if omitted)
- `completion_promise`: completion text; stop when the assistant outputs `<promise>...</promise>`
- `completion_matcher`: `exact` / `case_insensitive` / `regex` (only applies when `completion_promise` is set)
- `max_iterations`: 0 means unlimited (defaults from project config, or 0)
- `history_limit`: 0 disables pruning (default 200, configurable per project)

Tools and parameters (defaults/limits in parentheses):
- `task_loop`: `prompt`(required), `task_name`, `max_iterations`(default 0), `completion_promise`, `completion_matcher`, `history_limit`(default 200), `storage`(default project), `project_dir`
- `task_list`: `storage`(default project), `project_dir`, `limit`(default 50, range 1..2000), `offset`(>=0)
- `task_resume`: `task_name`(required), `storage`, `project_dir`
- `task_rename`: `task_name`(required), `new_name`(required), `storage`, `project_dir`
- `task_delete`: `task_name`(required), `storage`, `project_dir`

Defaults:
- You can set defaults in `.codex/task-loop.config.toml` (`default_max_iterations` / `default_completion_matcher` / `history_limit`)
- If not set: `max_iterations=0`, `completion_matcher=exact`, `history_limit=200`

## Uninstall
User-level:
```bash
/path/to/codex-taskloop/scripts/uninstall.sh --scope user
```

Project-level:
```bash
/path/to/codex-taskloop/scripts/uninstall.sh --scope project --project "$(pwd)"
```
