# 工作原理：codex-taskloop

本文描述 Codex 中 Taskloop 任务循环的内部机制，内容与 `src/main.rs` 和
`src/bin/hook.rs` 当前实现一致。

## 组件

1) MCP server（`codex-taskloop`）
- 创建任务 state。
- 维护 `meta.json` 索引。
- 暴露 MCP 工具。

2) Stop hook（`codex-taskloop-hook`）
- 每次 stop 时触发。
- 决定是否阻止 stop。

安装范围：
- 用户级：hook 写入 `config.toml` 的 `stop_hooks`。
- 项目级：hook 写入 `.codex/hooks/hooks.json`。

## 存储布局

两种存储根目录：
- local（项目）：`<project>/.codex/task_loop/`
- global（用户）：`$CODEX_HOME/task_loop/`（未设置则 `~/.codex/task_loop/`）

每个根目录包含：
- `meta.json`（任务索引）
- `meta.lock`（索引锁）
- `<task_dir>/state.md`
- `<task_dir>/history.jsonl`
- `<task_dir>/task.lock`

`task_dir` 为随机字母数字目录名。

## 任务唯一标识

任务唯一键为：
- `task_name + project_path`

约束：
- `task_name` 必填、单行、长度 <= 30。
- `project_path` 为绝对路径。

`task_name` 可从 prompt 第一行自动生成。

## meta.json 结构

`meta.json` 用于存储任务索引：

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

说明：
- `project_path` 在 local 与 global 都会记录。
- `status` 由 state/history 派生并由 hook 更新。

## State 文件格式

State 文件为 Markdown + YAML frontmatter：

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

## History 文件格式

History 为 JSONL（每行一个 JSON），字段包括：
- `ts`, `task_name`, `project_path`, `task_dir`
- `iteration`, `max_iterations`
- `completion_promise` / `completion_matcher`
- `prompt_preview`
- `state_file` / `history_file`
- `event`（如 `start`, `loop`, `resume`, `promise_matched`, `max_iterations`）

历史按 `history_limit` 保留最近 N 条。

## Stop hook 任务选择

Stop hook 在 stop 时：
1. 读取 local + global 的 `meta.json`。
2. 按当前 `project_path` 过滤任务。
3. 仅保留 state 仍存在的任务。
4. 从 active 任务中选择最近更新的一个。

## Stop hook 决策流程

对选中的任务：
1. 读取并校验 state。
2. 若 `active=false`：允许 stop，并记录 `paused`。
3. 若 `iteration >= max_iterations`（且 max_iterations > 0）：删除 state，记录 `max_iterations`，允许 stop。
4. 解析最后一条 assistant 消息（`last_agent_message` 或 `rollout_path`）。
5. 若 `<promise>...</promise>` 匹配：删除 state，记录 `promise_matched`，允许 stop。
6. 否则：`iteration` +1、写回 state、记录 `loop`、阻止 stop。

若 state 异常，hook 会删除并记录 `invalid_state`。

## MCP 工具行为

- `task_loop`：创建任务，写 state/history，更新 `meta.json`。
- `task_list`：读取 `meta.json`，合并 state/history 计算状态。
- `task_rename`：更新 `meta.json`；state 存在时同步更新与历史记录。
- `task_resume`：state 存在时 `active=true`；完成任务会失败。
- `task_delete`：删除任务目录并移除 meta 条目。

## 存储范围限制

- 用户级安装：可用 `storage=local` 或 `storage=global`。
- 项目级安装：仅允许 `storage=local`。
  由 `TASKLOOP_STORAGE_SCOPE=local-only` 强制。

## 可靠性

- `meta.lock` 与 `task.lock` 互斥锁。
- `state.md` 与历史裁剪使用原子写。
- 对异常 state/history 有安全降级处理。
