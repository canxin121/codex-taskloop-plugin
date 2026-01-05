# 模拟运行与对话流程

本文为叙述示例（非真实日志），展示任务循环在单个 Codex 会话内的运行。

## decision 由谁决定

`decision` 字段来自 Stop hook 程序，而不是模型。
模型只输出文本；hook 读取 state 与最后一条 assistant 消息后返回：
- `decision=block` 继续循环
- `decision=approve` 允许 stop

## 场景

目标：修复失败测试，全部通过后停止。
前提：
- Stop hook 已安装
- MCP server 已注册
- 可选默认值存在于 `.codex/task-loop.config.toml`

## 模拟运行（对话样式）

```
User: Start a loop to fix failing tests. Stop when DONE.
Codex: (calls task_loop)
Tool:  Taskloop task started.
```

幕后发生：
- MCP 在选定存储根目录创建任务。
- `meta.json` 写入任务索引。
- `history.jsonl` 记录 `start`。

状态文件（简化）：
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

幕后发生：
- Hook 读取 `<task_dir>/state.md`。
- 未出现 `<promise>DONE</promise>`，因此阻止 stop。
- `iteration` 递增到 2，history 记录 `loop`。
- `meta.json` 更新状态/迭代信息。

```
Codex: Continues, fixes remaining failures, tries to stop.
Hook:  decision=block, reason=<same prompt>, systemMessage=iteration 3/10
```

幕后发生：
- 再次注入同一 prompt。
- Hook 重复检查、迭代 +1，记录 `loop`。

```
Codex: All tests passing. <promise>DONE</promise>
Hook:  decision=approve
Codex: Final response to user.
```

幕后发生：
- Hook 检测到 `<promise>DONE</promise>` 并匹配成功。
- 删除 `state.md`，history 记录 `promise_matched`。
- `meta.json` 标记为 `completed`。

## 变体

- 无限循环：`max_iterations = 0`。
- 大小写不敏感匹配：`completion_matcher = "case_insensitive"`。
- 多任务并存：使用不同 `task_name`。
