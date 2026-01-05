# codex-taskloop-plugin

让 Codex 在单个会话内持续工作，直到任务完成。

## 插件功能
- 让 Codex 在同一会话内持续推进任务，直到完成
- Codex回复后，若未完成任务会继续迭代，满足完成条件或达到最大迭代上限才允许停止
- 在对话中管理任务：启动、列表、恢复、重命名、删除
- 任务数据支持用户级存储(仅用户级安装)和项目级存储

## 快速开始

### 构建
```bash
cargo build --release
```

### 安装
用户级（推荐）：
```bash
/path/to/codex-taskloop-plugin/scripts/install.sh --scope user
```

Windows（PowerShell）：
```powershell
.\scripts\install.ps1 -Scope user
```

项目级（仅项目级存储）：
```bash
/path/to/codex-taskloop-plugin/scripts/install.sh --scope project --project "$(pwd)"
```

完整参数（Bash `install.sh`）：
- `--scope <user|project>`：安装范围，默认 `project`
- `--project <path>`：项目路径（`--scope project` 时使用）
- `--name <name>`：MCP server 名称/stop hook 名称（默认 `codex-taskloop-plugin`）
- `--bin-dir <path>`：指定已构建的二进制目录
- `--no-mcp`：仅安装/复制二进制与技能，不注册 MCP
- `--no-hook`：不安装 stop hook
- `--no-build`：不自动构建（需 `--bin-dir` 提供二进制）

完整参数（PowerShell `install.ps1`）：
- `-Scope <user|project>`：安装范围，默认 `project`
- `-Project <path>`：项目路径（`-Scope project` 时使用）
- `-Name <name>`：MCP server 名称/stop hook 名称（默认 `codex-taskloop-plugin`）
- `-BinDir <path>`：指定已构建的二进制目录
- `-NoMcp`：仅安装/复制二进制与技能，不注册 MCP
- `-NoHook`：不安装 stop hook
- `-NoBuild`：不自动构建（需 `-BinDir` 提供二进制）

常见用法示例：
```bash
# 用户级 + 自定义名称 + 指定二进制目录
./scripts/install.sh --scope user --name my-taskloop --bin-dir ./bin

# 项目级 + 指定项目路径 + 仅安装 MCP（不装 hook）
./scripts/install.sh --scope project --project "/path/to/project" --no-hook
```

PowerShell 示例：
```powershell
# 用户级 + 自定义名称 + 指定二进制目录
.\scripts\install.ps1 -Scope user -Name my-taskloop -BinDir .\bin

# 项目级 + 指定项目路径 + 仅安装 MCP（不装 hook）
.\scripts\install.ps1 -Scope project -Project "C:\path\to\project" -NoHook
```

二进制自动搜索与指定目录：
- 安装脚本会寻找 `codex-taskloop-plugin`、`codex-taskloop-plugin-hook`、`codex-taskloop-plugin-admin`
- 默认搜索顺序：`<script_dir>/../target/release`、`<script_dir>/../bin`
- 未找到且有 `cargo` 时会自动构建（可用 `--no-build` / `-NoBuild` 关闭）
- 指定目录：`--bin-dir /path/to/dir`（PowerShell: `-BinDir`）

### 任务数据存储

#### 任务定义
- 任务的两个核心属性：`task_name`（任务名称）、`project_path`（项目路径）
- 唯一性：项目级存储以 `task_name` 在项目内唯一；用户级存储以 `task_name + project_path` 唯一
- 数据结构：每个存储根目录有 `meta.json`（索引），任务目录内有 `state.md`（状态）与 `history.jsonl`（历史）

#### 用户级存储
- 仅用户级安装可用
- 任务数据存储在`$CODEX_HOME/task_loop/`（未设置则 `~/.codex/task_loop/`）中

#### 项目级存储
- 用户级与项目级安装均可用
- 任务数据存储在项目路径下的 `.codex/task_loop/` 中


### 在对话中使用
用自然语言描述你想做的事即可，Codex 会自动选择合适的工具并补全参数。你可以描述：
- 任务目标与任务名
- 完成条件（例如“当输出某个关键词就停止”）
- 最大迭代次数或历史保留数量
- 使用项目级或用户级存储（以及需要的项目路径）


```
使用项目级存储，启动一个 Taskloop 任务来修复失败测试。任务完成后停止。
```

```
使用用户级存储，启动一个名为“修复失败测试”的任务，最多迭代 10 次。
```

```
列出项目级存储中的最近 10 个 Taskloop 任务。
```

```
只查看当前项目的项目级存储任务列表。
```

```
查看用户级存储中、项目路径为 /path/to/project 的任务列表。
```

```
在用户级存储中恢复暂停的“修复失败测试”任务并继续执行（项目路径为 /path/to/project）。
```

```
在项目级存储中将任务“修复失败测试”重命名为“修复测试”。
```

```
在项目级存储中删除任务“修复测试”。
```

### 工具与参数
参数要点：
- `storage`：`project` 或 `user`，默认 `project`
  - 用户级安装：可用 `project` / `user`
  - 项目级安装：仅允许 `project`
- `project_dir`：项目路径；默认使用 `CODEX_CWD` 或当前目录
  - 当 `storage=user` 且操作是 `task_resume` / `task_rename` / `task_delete` 时必填
  - 当 `storage=user` 且未提供 `project_dir`：`task_list` 会列出所有用户级任务；`task_loop` 会使用当前项目路径
- `task_name`：单行，<= 30 字符（不传则从 prompt 第一行自动生成）
- `completion_promise`：完成条件文本，助手输出 `<promise>...</promise>` 即停止
- `completion_matcher`：`exact` / `case_insensitive` / `regex`（仅在设置 `completion_promise` 时生效）
- `max_iterations`：0 表示不限次数（默认从项目配置读取，未配置时为 0）
- `history_limit`：0 表示不裁剪历史（默认 200，可在项目配置中修改）

工具与参数（括号内为默认/限制）：
- `task_loop`：`prompt`(必填), `task_name`, `max_iterations`(默认 0), `completion_promise`, `completion_matcher`, `history_limit`(默认 200), `storage`(默认 project), `project_dir`
- `task_list`：`storage`(默认 project), `project_dir`, `limit`(默认 50，范围 1..2000), `offset`(>=0)
- `task_resume`：`task_name`(必填), `storage`, `project_dir`
- `task_rename`：`task_name`(必填), `new_name`(必填), `storage`, `project_dir`
- `task_delete`：`task_name`(必填), `storage`, `project_dir`

默认值来源：
- 可在项目的 `.codex/task-loop.config.toml` 配置默认值（`default_max_iterations` / `default_completion_matcher` / `history_limit`）
- 未配置时默认：`max_iterations=0`、`completion_matcher=exact`、`history_limit=200`

## 卸载
用户级：
```bash
/path/to/codex-taskloop-plugin/scripts/uninstall.sh --scope user
```

项目级：
```bash
/path/to/codex-taskloop-plugin/scripts/uninstall.sh --scope project --project "$(pwd)"
```
