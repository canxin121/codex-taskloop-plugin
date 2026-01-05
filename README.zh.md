# codex-taskloop

面向 Codex 的 Taskloop 会话内循环：
- Stop hook（拦截 stop 并注入同一 prompt），以及
- MCP server（创建/更新任务状态与元数据索引）。

这是单个 Codex 会话内的通用循环机制，不依赖 Claude 插件。

文档：
- `docs/WORKING_PRINCIPLES.zh.md`
- `docs/SIMULATED_RUN.zh.md`
- 英文 README: `README.md`

## 架构（简版）

- **MCP server**（`codex-taskloop`）管理任务、`meta.json` 以及 state/history。
- **Stop hook**（`codex-taskloop-hook`）拦截 stop 并决定是否继续。

## 支持能力

核心循环：
- 会话内 Taskloop 循环（stop 时注入相同 prompt）。
- 完成承诺 `<promise>...</promise>`。
- 最大迭代次数（0 表示无限）。

任务模型：
- 任务唯一键 = `task_name + project_path`（同一项目内唯一）。
- `meta.json` 索引任务与进度。
- 每个任务一个目录，含 `state.md` + `history.jsonl` + `task.lock`。

控制：
- 列表（状态 + 最近事件）。
- 重命名任务。
- 恢复任务（仅当 state 仍存在）。
- 删除任务（删除任务目录与元数据）。

存储：
- local（项目）与 global（用户）存储。
- 用户级安装可按每次调用选择 local/global。
- 项目级安装强制 local-only。

可靠性：
- 文件锁避免竞争。
- 原子写入 state/history。
- 对异常 state 文件安全处理。

## 安装

### 1) 构建（已有二进制可跳过）

```bash
cargo build --release --manifest-path /path/to/codex-taskloop/Cargo.toml
```

### 2) 用户级安装（MCP + Stop hook）

注册 MCP server（全局）并在 Codex 配置中写入 stop_hooks：

```bash
/path/to/codex-taskloop/scripts/install-user.sh
```

如果设置了 `CODEX_HOME`，会写入 MCP 环境变量，使全局存储落在该目录下。

用户级安装支持 local + global 两种存储。
存储选择在每次工具调用决定（默认 local）。

### 3) 项目级安装（Stop hook + MCP）

```bash
cd /path/to/your/project

# 可选：隔离 config + global storage 用于测试
# export CODEX_HOME=/tmp/codex-home

/path/to/codex-taskloop/scripts/install.sh --project "$(pwd)"
```

该操作会：
- 写入 `.codex/hooks/hooks.json` 指向 hook 二进制。
- 在 `$CODEX_HOME/config.toml`（或 `~/.codex/config.toml`）注册 MCP。
- 传入 `CODEX_HOME` 到 MCP 环境（如已设置）。
- 强制 local-only（`storage=global` 会被拒绝）。

提示：如需同时用户级 + 项目级安装，请用不同的 MCP server 名称（`--name`）避免覆盖。

### MCP server 名称（为何重要）

Codex 在 `config.toml` 里按名称存 MCP server。重名会覆盖旧配置。

示例（同时保留用户级 + 项目级）：
```bash
# 用户级（global）
/path/to/codex-taskloop/scripts/install-user.sh --name codex-taskloop-user

# 项目级（local-only）
/path/to/codex-taskloop/scripts/install.sh --project "$(pwd)" --name codex-taskloop-project
```

然后在对话中指定：
```
Use the MCP tool codex-taskloop-user.task_loop ... storage: "global"
Use the MCP tool codex-taskloop-project.task_loop ... storage: "local"
```

可选参数：
- `--name <server>`（默认：`codex-taskloop`）
- `--no-mcp` / `--no-hook` / `--no-build`

验证：
```bash
codex mcp list
codex mcp get codex-taskloop
```

## 使用方式（两层）

### A) 用户对话层（你对 Codex 说什么）

你不需要直接调用 MCP 工具，只需描述需求。

示例：
```
Start a Taskloop task to fix failing tests. Use completion_promise DONE and
max_iterations 20. Store it locally.
```

```
Use the MCP tool task_loop with prompt:
"Fix tests and output <promise>DONE</promise> when all pass."
Set completion_promise: DONE, max_iterations: 20, storage: global.
```

控制请求示例：
```
List my active Taskloop tasks (global).
Rename the task to a shorter title.
Resume the paused task and continue.
Delete the old task when done.
```

### B) 工具调用层（Codex 实际会调用的 MCP）

启动任务（local 存储）：
```
task_loop {
  prompt: "Fix the failing tests. Output <promise>DONE</promise> when all tests pass.",
  task_name: "Fix failing tests",
  completion_promise: "DONE",
  max_iterations: 20,
  storage: "local"
}
```

列表：
```
task_list { storage: "global", limit: 20, offset: 0 }
```

恢复 / 重命名 / 删除：
```
task_resume { task_name: "Fix failing tests", storage: "local" }
task_rename { task_name: "Fix failing tests", new_name: "Fix tests", storage: "local" }
task_delete { task_name: "Fix tests", storage: "local" }
```

## 存储模型

支持两种存储位置：
- local（默认）：项目内 `.codex/task_loop/`
- global：`$CODEX_HOME/task_loop/`（未设置则 `~/.codex/task_loop/`）

说明：
- `storage=global` 时，`task_list` 的 `project_dir` 可选（不传则列出所有全局任务）；
  其它工具调用必须提供 `project_dir`。
- `storage=local` 时，`project_dir` 默认为 `CODEX_CWD` 或当前目录。

范围规则：
- 用户级安装：`storage=local` 与 `storage=global` 均可。
- 项目级安装：仅 local；`storage=global` 会被拒绝。
  该规则通过 `TASKLOOP_STORAGE_SCOPE=local-only` 在安装时固定。

## 任务生命周期与 Stop 规则

Stop hook 在每次 stop 时触发：
1. 读取 local + global 的 `meta.json`。
2. 按当前 `project_path` 过滤任务。
3. 选择最近更新的 active 任务。
4. 达到 `max_iterations`（且 > 0）时允许 stop 并删除 state。
5. `<promise>...</promise>` 匹配成功则允许 stop 并删除 state。
6. 否则：`iteration` +1、更新 `updated_at`、追加 history、阻止 stop。

## 文件与布局

存储根目录（local 或 global）：
- `meta.json`（任务索引 + 进度）
- `meta.lock`
- `<task_dir>/state.md`
- `<task_dir>/history.jsonl`
- `<task_dir>/task.lock`

Local 根目录：
- `.codex/task_loop/`

Global 根目录：
- `$CODEX_HOME/task_loop/`（或 `~/.codex/task_loop/`）

## 默认配置

在项目内创建 `.codex/task-loop.config.toml`：

```toml
default_max_iterations = 0
default_completion_matcher = "exact"
history_limit = 200
```

默认值会在启动任务或 state 缺字段时生效。

## 排错

- 工具不可用：确认 `codex mcp list` 中有 `codex-taskloop`。
- global 存储落到 `~/.codex`：运行 `install.sh` 与启动 `codex` 前设置 `CODEX_HOME`。
- 任务不继续：检查 `.codex/hooks/hooks.json` 是否指向 hook 二进制。
- `storage=global` 被拒绝：你使用的是项目级安装（local-only），请改用用户级安装。
- resume 失败：任务已完成且 state 已被删除。

## 卸载

用户级 MCP + Stop hook：
```bash
/path/to/codex-taskloop/scripts/uninstall-user.sh
```

项目级安装：
```bash
/path/to/codex-taskloop/scripts/uninstall.sh --project "$(pwd)"
```

## 开发

```bash
cargo build --release --manifest-path /path/to/codex-taskloop/Cargo.toml
```

## MCP 工具参考（完整列表）

通用参数：
- `storage`（可选）：`local` 或 `global`（默认 `local`）。
- `project_dir`（可选）：项目路径；global 的 resume/rename/delete 必填。

工具列表：
- `task_loop`（启动任务）
  - `prompt`（必填）
  - `task_name`（可选，<= 30 字符；未提供则自动生成）
  - `max_iterations`（可选，0 = 无限）
  - `completion_promise`（可选）
  - `completion_matcher`（可选：`exact`/`case_insensitive`/`regex`）
  - `history_limit`（可选，0 表示不裁剪）
  - `project_dir`（可选）
- `task_list`（列出任务）
  - `limit` / `offset`（可选）
  - `project_dir`（可选，仅用于 global 过滤）
- `task_resume`（恢复任务）
- `task_rename`（重命名任务）
- `task_delete`（删除任务）
