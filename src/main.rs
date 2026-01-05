use std::borrow::Cow;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use chrono::{DateTime, Utc};
use fs2::FileExt;
use rand::distr::Alphanumeric;
use rand::Rng;
use rmcp::ErrorData as McpError;
use rmcp::ServiceExt;
use rmcp::handler::server::ServerHandler;
use rmcp::model::CallToolRequestParam;
use rmcp::model::CallToolResult;
use rmcp::model::JsonObject;
use rmcp::model::ListToolsResult;
use rmcp::model::PaginatedRequestParam;
use rmcp::model::ServerCapabilities;
use rmcp::model::ServerInfo;
use rmcp::model::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

const META_FILE_NAME: &str = "meta.json";
const META_LOCK_NAME: &str = "meta.lock";
const STATE_FILE_NAME: &str = "state.md";
const HISTORY_FILE_NAME: &str = "history.jsonl";
const LOCK_FILE_NAME: &str = "task.lock";
const CONFIG_FILE_NAME: &str = "task-loop.config.toml";
const STORE_DIR_NAME: &str = "task_loop";
const SCHEMA_VERSION: u32 = 1;
const DEFAULT_HISTORY_LIMIT: u32 = 200;
const DEFAULT_MATCHER: &str = "exact";
const DEFAULT_LIST_LIMIT: u32 = 50;

#[derive(Clone)]
struct TaskloopServer {
    tools: Arc<Vec<Tool>>,
}

impl TaskloopServer {
    fn new() -> Self {
        let tools = vec![
            Self::task_loop_tool(),
            Self::task_list_tool(),
            Self::task_resume_tool(),
            Self::task_rename_tool(),
            Self::task_delete_tool(),
        ];
        Self {
            tools: Arc::new(tools),
        }
    }

    fn task_loop_tool() -> Tool {
        let schema: JsonObject = serde_json::from_value(json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string", "description": "Prompt to repeat each task iteration." },
                "task_name": { "type": "string", "description": "Meaningful task title (max 30 chars)." },
                "max_iterations": { "type": "integer", "minimum": 0, "description": "0 means unlimited." },
                "completion_promise": { "type": "string", "description": "Exact text that must appear inside <promise>...</promise> to stop." },
                "completion_matcher": {
                    "type": "string",
                    "description": "How to match the promise text: exact | case_insensitive | regex.",
                    "enum": ["exact", "case_insensitive", "regex"]
                },
                "history_limit": { "type": "integer", "minimum": 0, "description": "Max history entries to retain (0 disables pruning)." },
                "storage": { "type": "string", "enum": ["project", "user"], "description": "Where to store task files: project or user (default: project)." },
                "project_dir": { "type": "string", "description": "Project root (defaults to CODEX_CWD or current directory)." }
            },
            "required": ["prompt"],
            "additionalProperties": false
        }))
        .expect("task_loop schema should deserialize");

        Tool::new(
            Cow::Borrowed("task_loop"),
            Cow::Borrowed("Start a Taskloop task in the current Codex session."),
            Arc::new(schema),
        )
    }

    fn task_list_tool() -> Tool {
        let schema: JsonObject = serde_json::from_value(json!({
            "type": "object",
            "properties": {
                "storage": { "type": "string", "enum": ["project", "user"], "description": "Where to list tasks: project or user (default: project)." },
                "project_dir": { "type": "string", "description": "Project root to filter (defaults to current for project storage)." },
                "limit": { "type": "integer", "minimum": 1, "maximum": 2000, "description": "Max tasks to return (default 50)." },
                "offset": { "type": "integer", "minimum": 0, "description": "Offset into sorted task list." }
            },
            "additionalProperties": false
        }))
        .expect("task_list schema should deserialize");

        Tool::new(
            Cow::Borrowed("task_list"),
            Cow::Borrowed("List Taskloop tasks with status and last event."),
            Arc::new(schema),
        )
    }

    fn task_resume_tool() -> Tool {
        let schema: JsonObject = serde_json::from_value(json!({
            "type": "object",
            "properties": {
                "task_name": { "type": "string", "description": "Task name to resume." },
                "storage": { "type": "string", "enum": ["project", "user"], "description": "Where to look for task files: project or user (default: project)." },
                "project_dir": { "type": "string", "description": "Project root (required for user storage tasks)." }
            },
            "required": ["task_name"],
            "additionalProperties": false
        }))
        .expect("task_resume schema should deserialize");

        Tool::new(
            Cow::Borrowed("task_resume"),
            Cow::Borrowed("Resume a paused Taskloop task."),
            Arc::new(schema),
        )
    }

    fn task_rename_tool() -> Tool {
        let schema: JsonObject = serde_json::from_value(json!({
            "type": "object",
            "properties": {
                "task_name": { "type": "string", "description": "Existing task name." },
                "new_name": { "type": "string", "description": "New task name (max 30 chars)." },
                "storage": { "type": "string", "enum": ["project", "user"], "description": "Where to look for task files: project or user (default: project)." },
                "project_dir": { "type": "string", "description": "Project root (required for user storage tasks)." }
            },
            "required": ["task_name", "new_name"],
            "additionalProperties": false
        }))
        .expect("task_rename schema should deserialize");

        Tool::new(
            Cow::Borrowed("task_rename"),
            Cow::Borrowed("Rename a Taskloop task."),
            Arc::new(schema),
        )
    }

    fn task_delete_tool() -> Tool {
        let schema: JsonObject = serde_json::from_value(json!({
            "type": "object",
            "properties": {
                "task_name": { "type": "string", "description": "Task name to delete." },
                "storage": { "type": "string", "enum": ["project", "user"], "description": "Where to look for task files: project or user (default: project)." },
                "project_dir": { "type": "string", "description": "Project root (required for user storage tasks)." }
            },
            "required": ["task_name"],
            "additionalProperties": false
        }))
        .expect("task_delete schema should deserialize");

        Tool::new(
            Cow::Borrowed("task_delete"),
            Cow::Borrowed("Delete a Taskloop task and its history."),
            Arc::new(schema),
        )
    }
}

#[derive(Deserialize)]
struct TaskloopLoopArgs {
    prompt: String,
    #[serde(default)]
    task_name: Option<String>,
    #[serde(default)]
    max_iterations: Option<u32>,
    #[serde(default)]
    completion_promise: Option<String>,
    #[serde(default)]
    completion_matcher: Option<String>,
    #[serde(default)]
    history_limit: Option<u32>,
    #[serde(default)]
    storage: Option<String>,
    #[serde(default)]
    project_dir: Option<String>,
}

#[derive(Deserialize)]
struct TaskArgs {
    task_name: String,
    #[serde(default)]
    storage: Option<String>,
    #[serde(default)]
    project_dir: Option<String>,
}

#[derive(Deserialize)]
struct TaskloopRenameArgs {
    task_name: String,
    new_name: String,
    #[serde(default)]
    storage: Option<String>,
    #[serde(default)]
    project_dir: Option<String>,
}

#[derive(Deserialize)]
struct TaskloopListArgs {
    #[serde(default)]
    storage: Option<String>,
    #[serde(default)]
    project_dir: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    offset: Option<u32>,
}

#[derive(Default, Deserialize)]
struct TaskloopConfigFile {
    default_max_iterations: Option<u32>,
    default_completion_matcher: Option<String>,
    history_limit: Option<u32>,
}

#[derive(Clone, Debug)]
struct ConfigDefaults {
    max_iterations: u32,
    completion_matcher: String,
    history_limit: u32,
}

impl ConfigDefaults {
    fn from_config(config: TaskloopConfigFile) -> Self {
        let matcher = normalize_matcher_name(
            config
                .default_completion_matcher
                .unwrap_or_else(|| DEFAULT_MATCHER.to_string()),
        )
        .unwrap_or_else(|| DEFAULT_MATCHER.to_string());
        let history_limit = config.history_limit.unwrap_or(DEFAULT_HISTORY_LIMIT);
        Self {
            max_iterations: config.default_max_iterations.unwrap_or(0),
            completion_matcher: matcher,
            history_limit,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StorageMode {
    Project,
    User,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StorageScope {
    ProjectOnly,
    Both,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TaskMeta {
    task_name: String,
    task_dir: String,
    project_path: Option<String>,
    status: Option<String>,
    iteration: Option<u32>,
    max_iterations: Option<u32>,
    last_event: Option<String>,
    started_at: Option<String>,
    updated_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MetaIndex {
    schema_version: u32,
    tasks: Vec<TaskMeta>,
}

impl MetaIndex {
    fn empty() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            tasks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct StateData {
    schema_version: u32,
    task_name: String,
    project_path: Option<String>,
    active: bool,
    iteration: u32,
    max_iterations: u32,
    completion_promise: Option<String>,
    completion_matcher: Option<String>,
    history_limit: Option<u32>,
    started_at: Option<String>,
    updated_at: Option<String>,
    prompt: String,
    extras: Vec<(String, String)>,
}

impl ServerHandler for TaskloopServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..ServerInfo::default()
        }
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let tools = self.tools.clone();
        async move {
            Ok(ListToolsResult {
                tools: (*tools).clone(),
                next_cursor: None,
                meta: None,
            })
        }
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let scope = storage_scope_from_env();
        match request.name.as_ref() {
            "task_loop" => {
                let args: TaskloopLoopArgs = parse_args(request.arguments)?;
                if args.prompt.trim().is_empty() {
                    return Err(McpError::invalid_params("prompt cannot be empty", None));
                }
                let project_dir = resolve_project_dir(args.project_dir.clone())?;
                let project_path = project_dir.to_string_lossy().to_string();
                let defaults = load_config_defaults(&project_dir);
                let storage_mode = enforce_storage_mode(scope, parse_storage_mode(args.storage)?)?;
                let storage_root = storage_root_for_mode(&project_dir, storage_mode);
                let task_name = normalize_task_name(args.task_name, &args.prompt)?;

                let completion_promise = args
                    .completion_promise
                    .filter(|value| !value.trim().is_empty());
                let completion_matcher = normalize_matcher(
                    args.completion_matcher
                        .or_else(|| Some(defaults.completion_matcher.clone())),
                    completion_promise.as_deref(),
                )?;

                let max_iterations = args.max_iterations.unwrap_or(defaults.max_iterations);
                let history_limit = args.history_limit.unwrap_or(defaults.history_limit);

                let now = Utc::now().to_rfc3339();

                let meta_lock = meta_lock_path(&storage_root);
                let (task_dir, _state_path) = with_lock(&meta_lock, || {
                    let mut meta = load_meta(&storage_root)?;
                    if task_exists(&meta, &task_name, Some(&project_path), storage_mode) {
                        anyhow::bail!("task already exists");
                    }
                    let task_dir = generate_task_dir(&storage_root);
                    let task_root = task_root(&storage_root, &task_dir);
                    let state_path = state_file_path(&task_root);
                    let history_path = history_file_path(&task_root);

                    let state = StateData {
                        schema_version: SCHEMA_VERSION,
                        task_name: task_name.clone(),
                        project_path: Some(project_path.clone()),
                        active: true,
                        iteration: 1,
                        max_iterations,
                        completion_promise,
                        completion_matcher,
                        history_limit: Some(history_limit),
                        started_at: Some(now.clone()),
                        updated_at: Some(now.clone()),
                        prompt: args.prompt.clone(),
                        extras: Vec::new(),
                    };

                    write_state_data(&state_path, &state)?;
                    append_history_event(&history_path, history_limit, json!({
                        "ts": Utc::now().to_rfc3339(),
                        "event": "start",
                        "task_name": task_name,
                        "project_path": project_path,
                        "task_dir": task_dir,
                        "active": state.active,
                        "iteration": state.iteration,
                        "max_iterations": state.max_iterations,
                        "completion_promise": state.completion_promise,
                        "completion_matcher": state.completion_matcher,
                        "history_limit": state.history_limit,
                        "state_file": state_path.display().to_string(),
                        "history_file": history_path.display().to_string(),
                    }));

                    meta.tasks.push(TaskMeta {
                        task_name: state.task_name.clone(),
                        task_dir: task_dir.clone(),
                        project_path: state.project_path.clone(),
                        status: Some("in_progress".to_string()),
                        iteration: Some(state.iteration),
                        max_iterations: Some(state.max_iterations),
                        last_event: Some("start".to_string()),
                        started_at: state.started_at.clone(),
                        updated_at: state.updated_at.clone(),
                    });
                    write_meta(&storage_root, &meta)?;

                    Ok((task_dir, state_path))
                })
                .map_err(|err| McpError::internal_error(err.to_string(), None))?;

                let message = format!(
                    "Taskloop task started: '{}' (storage: {}, dir: {})",
                    task_name,
                    storage_label(storage_mode),
                    task_dir
                );

                Ok(CallToolResult::success(vec![rmcp::model::Content::text(
                    message,
                )]))
            }
            "task_list" => {
                let args: TaskloopListArgs = parse_args(request.arguments)?;
                let storage_mode = enforce_storage_mode(scope, parse_storage_mode(args.storage)?)?;
                let resolved_project_dir = if storage_mode == StorageMode::Project {
                    Some(resolve_project_dir(args.project_dir.clone())?)
                } else if let Some(value) = args.project_dir.clone() {
                    Some(resolve_project_dir(Some(value))?)
                } else {
                    None
                };
                let filter_project_path = resolved_project_dir
                    .as_ref()
                    .map(|dir| dir.to_string_lossy().to_string());
                let storage_root = match storage_mode {
                    StorageMode::Project => {
                        let dir = resolved_project_dir
                            .as_ref()
                            .expect("project storage requires project_dir");
                        storage_root_for_mode(dir, StorageMode::Project)
                    }
                    StorageMode::User => user_store_root(),
                };

                let limit = args.limit.unwrap_or(DEFAULT_LIST_LIMIT).clamp(1, 2000) as usize;
                let offset = args.offset.unwrap_or(0) as usize;

                let meta_lock = meta_lock_path(&storage_root);
                let meta = with_lock(&meta_lock, || load_meta(&storage_root))
                    .map_err(|err| McpError::internal_error(err.to_string(), None))?;

                let mut summaries = Vec::new();
                for task in meta.tasks.iter() {
                    if let Some(filter) = filter_project_path.as_deref() {
                        if task.project_path.as_deref() != Some(filter) {
                            continue;
                        }
                    }

                    let task_root = task_root(&storage_root, &task.task_dir);
                    let state_path = state_file_path(&task_root);
                    let history_path = history_file_path(&task_root);
                    let state = read_state_data(&state_path).ok();
                    let last_event = read_last_history_event(&history_path).ok().flatten();
                    let status = derive_status(state.as_ref(), last_event.as_ref());

                    let (iteration, max_iterations) = match state.as_ref() {
                        Some(state) => (Some(state.iteration), Some(state.max_iterations)),
                        None => (
                            last_event
                                .as_ref()
                                .and_then(|v| v.get("iteration"))
                                .and_then(|v| v.as_u64())
                                .map(|v| v as u32),
                            last_event
                                .as_ref()
                                .and_then(|v| v.get("max_iterations"))
                                .and_then(|v| v.as_u64())
                                .map(|v| v as u32),
                        ),
                    };

                    let last_updated = state
                        .as_ref()
                        .and_then(|s| s.updated_at.clone())
                        .or_else(|| {
                            last_event
                                .as_ref()
                                .and_then(|v| v.get("ts"))
                                .and_then(|v| v.as_str())
                                .map(|v| v.to_string())
                        })
                        .or_else(|| task.updated_at.clone());

                    let sort_ts = last_updated
                        .as_deref()
                        .and_then(parse_timestamp)
                        .unwrap_or(0);

                    summaries.push(TaskSummary {
                        task_name: task.task_name.clone(),
                        task_dir: task.task_dir.clone(),
                        project_path: task.project_path.clone(),
                        status: status.to_string(),
                        iteration,
                        max_iterations,
                        last_event: last_event
                            .as_ref()
                            .and_then(|v| v.get("event"))
                            .and_then(|v| v.as_str())
                            .map(|v| v.to_string()),
                        last_updated,
                        sort_ts,
                    });
                }

                summaries.sort_by(|a, b| b.sort_ts.cmp(&a.sort_ts));
                let total = summaries.len();
                let results = summaries
                    .into_iter()
                    .skip(offset)
                    .take(limit)
                    .map(|summary| summary.into_json(storage_label(storage_mode)))
                    .collect::<Vec<_>>();

                let summary = format!("Found {} task(s) (showing {}..{})", total, offset, offset + results.len());
                Ok(CallToolResult {
                    content: vec![rmcp::model::Content::text(summary)],
                    structured_content: Some(json!({
                        "tasks": results,
                        "total": total,
                        "offset": offset,
                        "limit": limit,
                        "storage": storage_label(storage_mode)
                    })),
                    is_error: Some(false),
                    meta: None,
                })
            }
            "task_resume" => {
                let args: TaskArgs = parse_args(request.arguments)?;
                let provided_project = args.project_dir.is_some();
                let storage_mode = enforce_storage_mode(scope, parse_storage_mode(args.storage)?)?;
                if storage_mode == StorageMode::User && !provided_project {
                    return Err(McpError::invalid_params(
                        "project_dir is required for user storage",
                        None,
                    ));
                }
                let project_dir = resolve_project_dir(args.project_dir)?;
                let project_path = project_dir.to_string_lossy().to_string();
                let storage_root = storage_root_for_mode(&project_dir, storage_mode);

                let task_dir = find_task_dir(&storage_root, &args.task_name, &project_path, storage_mode)
                    .map_err(|err| McpError::internal_error(err.to_string(), None))?;

                let task_root = task_root(&storage_root, &task_dir);
                let state_path = state_file_path(&task_root);
                let history_path = history_file_path(&task_root);
                let lock_path = lock_file_path(&task_root);
                let defaults = load_config_defaults(&project_dir);

                let message = with_lock(&lock_path, || {
                    if !state_path.exists() {
                        return Err(anyhow::anyhow!("no task state file found"));
                    }
                    let mut state = read_state_data(&state_path)?;
                    state.active = true;
                    state.updated_at = Some(Utc::now().to_rfc3339());
                    write_state_data(&state_path, &state)?;
                    append_history_event(&history_path, state.history_limit.unwrap_or(defaults.history_limit), json!({
                        "ts": Utc::now().to_rfc3339(),
                        "event": "resume",
                        "task_name": state.task_name,
                        "project_path": state.project_path,
                        "task_dir": task_dir,
                        "active": state.active,
                        "iteration": state.iteration,
                        "max_iterations": state.max_iterations,
                        "completion_promise": state.completion_promise,
                        "completion_matcher": state.completion_matcher,
                        "history_limit": state.history_limit,
                        "state_file": state_path.display().to_string(),
                        "history_file": history_path.display().to_string(),
                    }));
                    Ok(format!(
                        "Resumed Taskloop task '{}' (storage: {})",
                        state.task_name,
                        storage_label(storage_mode)
                    ))
                })
                .map_err(|err| McpError::internal_error(err.to_string(), None))?;

                update_meta_status(
                    &storage_root,
                    &task_dir,
                    Some("in_progress"),
                    Some("resume"),
                );

                Ok(CallToolResult::success(vec![rmcp::model::Content::text(
                    message,
                )]))
            }
            "task_rename" => {
                let args: TaskloopRenameArgs = parse_args(request.arguments)?;
                let provided_project = args.project_dir.is_some();
                let storage_mode = enforce_storage_mode(scope, parse_storage_mode(args.storage)?)?;
                if storage_mode == StorageMode::User && !provided_project {
                    return Err(McpError::invalid_params(
                        "project_dir is required for user storage",
                        None,
                    ));
                }
                let project_dir = resolve_project_dir(args.project_dir)?;
                let project_path = project_dir.to_string_lossy().to_string();
                let storage_root = storage_root_for_mode(&project_dir, storage_mode);

                let new_name = normalize_task_name(Some(args.new_name), "")?;
                let meta_lock = meta_lock_path(&storage_root);
                let task_dir = with_lock(&meta_lock, || {
                    let mut meta = load_meta(&storage_root)?;
                    if task_exists(&meta, &new_name, Some(&project_path), storage_mode) {
                        anyhow::bail!("task name already exists");
                    }
                    let index = find_task_index(&meta, &args.task_name, Some(&project_path), storage_mode)
                        .ok_or_else(|| anyhow::anyhow!("task not found"))?;
                    let task_dir = meta.tasks[index].task_dir.clone();
                    meta.tasks[index].task_name = new_name.clone();
                    meta.tasks[index].updated_at = Some(Utc::now().to_rfc3339());
                    meta.tasks[index].last_event = Some("rename".to_string());
                    write_meta(&storage_root, &meta)?;
                    Ok(task_dir)
                })
                .map_err(|err| McpError::internal_error(err.to_string(), None))?;

                let task_root = task_root(&storage_root, &task_dir);
                let state_path = state_file_path(&task_root);
                let history_path = history_file_path(&task_root);
                let lock_path = lock_file_path(&task_root);
                let defaults = load_config_defaults(&project_dir);

                let message = with_lock(&lock_path, || {
                    if state_path.exists() {
                        let mut state = read_state_data(&state_path)?;
                        state.task_name = new_name.clone();
                        state.updated_at = Some(Utc::now().to_rfc3339());
                        write_state_data(&state_path, &state)?;
                        append_history_event(&history_path, state.history_limit.unwrap_or(defaults.history_limit), json!({
                            "ts": Utc::now().to_rfc3339(),
                            "event": "rename",
                            "task_name": state.task_name,
                            "project_path": state.project_path,
                            "task_dir": task_dir,
                            "active": state.active,
                            "iteration": state.iteration,
                            "max_iterations": state.max_iterations,
                            "completion_promise": state.completion_promise,
                            "completion_matcher": state.completion_matcher,
                            "history_limit": state.history_limit,
                            "state_file": state_path.display().to_string(),
                            "history_file": history_path.display().to_string(),
                            "previous_name": args.task_name,
                        }));
                    }
                    Ok(format!(
                        "Renamed Taskloop task to '{}' (storage: {})",
                        new_name,
                        storage_label(storage_mode)
                    ))
                })
                .map_err(|err| McpError::internal_error(err.to_string(), None))?;

                Ok(CallToolResult::success(vec![rmcp::model::Content::text(
                    message,
                )]))
            }
            "task_delete" => {
                let args: TaskArgs = parse_args(request.arguments)?;
                let provided_project = args.project_dir.is_some();
                let storage_mode = enforce_storage_mode(scope, parse_storage_mode(args.storage)?)?;
                if storage_mode == StorageMode::User && !provided_project {
                    return Err(McpError::invalid_params(
                        "project_dir is required for user storage",
                        None,
                    ));
                }
                let project_dir = resolve_project_dir(args.project_dir)?;
                let project_path = project_dir.to_string_lossy().to_string();
                let storage_root = storage_root_for_mode(&project_dir, storage_mode);

                let meta_lock = meta_lock_path(&storage_root);
                let task_dir = with_lock(&meta_lock, || {
                    let mut meta = load_meta(&storage_root)?;
                    let index = find_task_index(&meta, &args.task_name, Some(&project_path), storage_mode)
                        .ok_or_else(|| anyhow::anyhow!("task not found"))?;
                    let task_dir = meta.tasks[index].task_dir.clone();
                    meta.tasks.remove(index);
                    write_meta(&storage_root, &meta)?;
                    Ok(task_dir)
                })
                .map_err(|err| McpError::internal_error(err.to_string(), None))?;

                let task_root = task_root(&storage_root, &task_dir);
                let state_path = state_file_path(&task_root);
                let history_path = history_file_path(&task_root);
                let lock_path = lock_file_path(&task_root);

                let message = with_lock(&lock_path, || {
                    if state_path.exists() {
                        let _ = fs::remove_file(&state_path);
                    }
                    if history_path.exists() {
                        let _ = fs::remove_file(&history_path);
                    }
                    if task_root.exists() {
                        let _ = fs::remove_dir_all(&task_root);
                    }
                    Ok(format!(
                        "Deleted Taskloop task '{}' (storage: {})",
                        args.task_name,
                        storage_label(storage_mode)
                    ))
                })
                .map_err(|err| McpError::internal_error(err.to_string(), None))?;

                Ok(CallToolResult::success(vec![rmcp::model::Content::text(
                    message,
                )]))
            }
            other => Err(McpError::invalid_params(
                format!("unknown tool: {other}"),
                None,
            )),
        }
    }
}

fn parse_args<T: for<'de> Deserialize<'de>>(
    arguments: Option<JsonObject>,
) -> Result<T, McpError> {
    let value = serde_json::Value::Object(
        arguments
            .unwrap_or_default()
            .into_iter()
            .collect::<serde_json::Map<String, serde_json::Value>>(),
    );
    serde_json::from_value(value).map_err(|err| McpError::invalid_params(err.to_string(), None))
}

fn resolve_project_dir(project_dir: Option<String>) -> Result<PathBuf, McpError> {
    let raw = if let Some(dir) = project_dir {
        expand_tilde(&dir)
    } else if let Ok(dir) = std::env::var("CODEX_CWD") {
        if !dir.trim().is_empty() {
            expand_tilde(&dir)
        } else {
            std::env::current_dir().map_err(|err| McpError::internal_error(err.to_string(), None))?
        }
    } else {
        std::env::current_dir().map_err(|err| McpError::internal_error(err.to_string(), None))?
    };

    let absolute = if raw.is_absolute() {
        raw
    } else {
        std::env::current_dir()
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .join(raw)
    };

    Ok(fs::canonicalize(&absolute).unwrap_or(absolute))
}

fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home);
        }
    }
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(stripped);
        }
    }
    PathBuf::from(path)
}

fn parse_storage_mode(value: Option<String>) -> Result<StorageMode, McpError> {
    let mode = value.unwrap_or_else(|| "project".to_string());
    let normalized = mode.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "" | "project" | "local" => Ok(StorageMode::Project),
        "user" | "global" => Ok(StorageMode::User),
        _ => Err(McpError::invalid_params(
            "storage must be project or user",
            None,
        )),
    }
}

fn storage_scope_from_env() -> StorageScope {
    let value = std::env::var("TASKLOOP_STORAGE_SCOPE").unwrap_or_default();
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "project" | "project-only" | "local" | "local-only" => StorageScope::ProjectOnly,
        "" | "both" | "all" | "user" | "global" => StorageScope::Both,
        _ => StorageScope::Both,
    }
}

fn enforce_storage_mode(
    scope: StorageScope,
    mode: StorageMode,
) -> Result<StorageMode, McpError> {
    if scope == StorageScope::ProjectOnly && mode == StorageMode::User {
        return Err(McpError::invalid_params(
            "storage=user is not allowed for project-level install; use storage=project",
            None,
        ));
    }
    Ok(mode)
}

fn project_store_root(project_dir: &Path) -> PathBuf {
    project_dir.join(".codex").join(STORE_DIR_NAME)
}

fn codex_home_dir() -> PathBuf {
    if let Ok(home) = std::env::var("CODEX_HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".codex");
    }
    PathBuf::from(".codex")
}

fn user_store_root() -> PathBuf {
    codex_home_dir().join(STORE_DIR_NAME)
}

fn storage_root_for_mode(project_dir: &Path, mode: StorageMode) -> PathBuf {
    match mode {
        StorageMode::Project => project_store_root(project_dir),
        StorageMode::User => user_store_root(),
    }
}

fn storage_label(mode: StorageMode) -> &'static str {
    match mode {
        StorageMode::Project => "project",
        StorageMode::User => "user",
    }
}

fn task_root(root: &Path, task_dir: &str) -> PathBuf {
    root.join(task_dir)
}

fn state_file_path(task_root: &Path) -> PathBuf {
    task_root.join(STATE_FILE_NAME)
}

fn history_file_path(task_root: &Path) -> PathBuf {
    task_root.join(HISTORY_FILE_NAME)
}

fn lock_file_path(task_root: &Path) -> PathBuf {
    task_root.join(LOCK_FILE_NAME)
}

fn meta_file_path(root: &Path) -> PathBuf {
    root.join(META_FILE_NAME)
}

fn meta_lock_path(root: &Path) -> PathBuf {
    root.join(META_LOCK_NAME)
}

fn config_file_path(project_dir: &Path) -> PathBuf {
    project_dir.join(".codex").join(CONFIG_FILE_NAME)
}

fn load_config_defaults(project_dir: &Path) -> ConfigDefaults {
    let path = config_file_path(project_dir);
    if let Ok(contents) = fs::read_to_string(&path) {
        if let Ok(cfg) = toml::from_str::<TaskloopConfigFile>(&contents) {
            return ConfigDefaults::from_config(cfg);
        }
    }
    ConfigDefaults::from_config(TaskloopConfigFile::default())
}

fn normalize_matcher_name(value: String) -> Option<String> {
    let normalized = match value.trim().to_ascii_lowercase().as_str() {
        "exact" => "exact",
        "case_insensitive" | "ci" | "insensitive" => "case_insensitive",
        "regex" => "regex",
        _ => return None,
    };
    Some(normalized.to_string())
}

fn normalize_matcher(
    matcher: Option<String>,
    completion_promise: Option<&str>,
) -> Result<Option<String>, McpError> {
    let Some(value) = matcher else {
        return Ok(None);
    };
    if completion_promise.is_none() {
        return Ok(None);
    }
    let Some(normalized) = normalize_matcher_name(value) else {
        return Err(McpError::invalid_params(
            "completion_matcher must be exact, case_insensitive, or regex",
            None,
        ));
    };
    Ok(Some(normalized))
}

fn normalize_task_name(task_name: Option<String>, prompt: &str) -> Result<String, McpError> {
    let raw = task_name.unwrap_or_else(|| generate_task_name(prompt));
    let trimmed = normalize_space(&raw);
    if trimmed.is_empty() {
        return Err(McpError::invalid_params("task_name cannot be empty", None));
    }
    if trimmed.contains('\n') || trimmed.contains('\r') {
        return Err(McpError::invalid_params("task_name must be single line", None));
    }
    if trimmed.chars().count() > 30 {
        return Err(McpError::invalid_params(
            "task_name must be <= 30 characters",
            None,
        ));
    }
    Ok(trimmed)
}

fn generate_task_name(prompt: &str) -> String {
    let line = prompt
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(prompt);
    let normalized = normalize_space(line);
    if normalized.is_empty() {
        return "task".to_string();
    }
    normalized.chars().take(30).collect::<String>()
}

fn normalize_space(value: &str) -> String {
    value.split_whitespace().collect::<Vec<&str>>().join(" ")
}

fn escape_yaml(value: &str) -> Result<String> {
    if value.contains('\n') || value.contains('\r') {
        anyhow::bail!("completion_promise must be a single line");
    }
    Ok(value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn load_meta(root: &Path) -> Result<MetaIndex> {
    let path = meta_file_path(root);
    if !path.exists() {
        return Ok(MetaIndex::empty());
    }
    let text = fs::read_to_string(&path)?;
    let mut meta: MetaIndex = serde_json::from_str(&text)?;
    if meta.schema_version == 0 {
        meta.schema_version = SCHEMA_VERSION;
    }
    Ok(meta)
}

fn write_meta(root: &Path, meta: &MetaIndex) -> Result<()> {
    if let Some(parent) = root.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }
    let mut updated = meta.clone();
    updated.schema_version = SCHEMA_VERSION;
    let text = serde_json::to_string_pretty(&updated)?;
    write_atomic(&meta_file_path(root), format!("{text}\n").as_bytes())?;
    Ok(())
}

fn task_exists(meta: &MetaIndex, task_name: &str, project_path: Option<&str>, mode: StorageMode) -> bool {
    meta.tasks.iter().any(|task| match mode {
        StorageMode::Project => task.task_name == task_name,
        StorageMode::User => {
            task.task_name == task_name && task.project_path.as_deref() == project_path
        }
    })
}

fn find_task_index(
    meta: &MetaIndex,
    task_name: &str,
    project_path: Option<&str>,
    mode: StorageMode,
) -> Option<usize> {
    meta.tasks.iter().position(|task| match mode {
        StorageMode::Project => task.task_name == task_name,
        StorageMode::User => {
            task.task_name == task_name && task.project_path.as_deref() == project_path
        }
    })
}

fn find_task_dir(
    storage_root: &Path,
    task_name: &str,
    project_path: &str,
    mode: StorageMode,
) -> Result<String> {
    let meta_lock = meta_lock_path(storage_root);
    let meta = with_lock(&meta_lock, || load_meta(storage_root))?;
    let index = find_task_index(&meta, task_name, Some(project_path), mode)
        .ok_or_else(|| anyhow::anyhow!("task not found"))?;
    Ok(meta.tasks[index].task_dir.clone())
}

fn update_meta_status(
    storage_root: &Path,
    task_dir: &str,
    status: Option<&str>,
    last_event: Option<&str>,
) {
    let meta_lock = meta_lock_path(storage_root);
    let result = with_lock(&meta_lock, || {
        let mut meta = load_meta(storage_root)?;
        if let Some(task) = meta.tasks.iter_mut().find(|task| task.task_dir == task_dir) {
            if let Some(status) = status {
                task.status = Some(status.to_string());
            }
            if let Some(last_event) = last_event {
                task.last_event = Some(last_event.to_string());
            }
            task.updated_at = Some(Utc::now().to_rfc3339());
            write_meta(storage_root, &meta)?;
        }
        Ok(())
    });
    let _ = result;
}

fn generate_task_dir(root: &Path) -> String {
    let mut rng = rand::rng();
    for _ in 0..100 {
        let candidate: String = (&mut rng)
            .sample_iter(&Alphanumeric)
            .take(12)
            .map(char::from)
            .collect::<String>()
            .to_ascii_lowercase();
        if !root.join(&candidate).exists() {
            return candidate;
        }
    }
    format!("task-{}", Utc::now().timestamp_millis())
}

fn write_state_data(state_path: &Path, data: &StateData) -> Result<()> {
    if let Some(parent) = state_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let completion_line = match data.completion_promise.as_deref() {
        Some(value) if !value.trim().is_empty() => {
            let escaped = escape_yaml(value)?;
            format!("completion_promise: \"{}\"", escaped)
        }
        _ => "completion_promise: null".to_string(),
    };

    let matcher_line = match data.completion_matcher.as_deref() {
        Some(value) => {
            let escaped = escape_yaml(value)?;
            Some(format!("completion_matcher: \"{}\"", escaped))
        }
        None => None,
    };

    let history_line = data
        .history_limit
        .map(|limit| format!("history_limit: {limit}"));

    let timestamp = data
        .started_at
        .clone()
        .unwrap_or_else(|| Utc::now().to_rfc3339());
    let updated_at = data
        .updated_at
        .clone()
        .unwrap_or_else(|| Utc::now().to_rfc3339());

    let mut lines = Vec::new();
    lines.push("---".to_string());
    lines.push(format!("schema_version: {}", data.schema_version));
    lines.push(format!("task_name: {}", data.task_name));
    if let Some(project_path) = data.project_path.as_deref() {
        lines.push(format!("project_path: {}", project_path));
    }
    lines.push(format!("active: {}", if data.active { "true" } else { "false" }));
    lines.push(format!("iteration: {}", data.iteration));
    lines.push(format!("max_iterations: {}", data.max_iterations));
    lines.push(completion_line);
    if let Some(line) = matcher_line {
        lines.push(line);
    }
    if let Some(line) = history_line {
        lines.push(line);
    }
    lines.push(format!("started_at: \"{}\"", timestamp));
    lines.push(format!("updated_at: \"{}\"", updated_at));
    for (key, value) in data.extras.iter() {
        if is_known_key(key) {
            continue;
        }
        lines.push(format!("{key}: {value}"));
    }
    lines.push("---".to_string());
    lines.push(String::new());
    lines.push(data.prompt.trim_end().to_string());
    lines.push(String::new());

    write_atomic(state_path, lines.join("\n").as_bytes())?;
    Ok(())
}

fn read_state_data(state_path: &Path) -> Result<StateData> {
    let text = std::fs::read_to_string(state_path)?;
    if !text.starts_with("---") {
        anyhow::bail!("state file missing frontmatter");
    }
    let parts: Vec<&str> = text.splitn(3, "---").collect();
    if parts.len() < 3 {
        anyhow::bail!("state file missing closing frontmatter");
    }
    let frontmatter_text = parts[1];
    let prompt = parts[2].trim_start_matches('\n').to_string();

    let mut schema_version = SCHEMA_VERSION;
    let mut task_name = String::new();
    let mut project_path: Option<String> = None;
    let mut active = true;
    let mut iteration: Option<u32> = None;
    let mut max_iterations: Option<u32> = None;
    let mut completion_promise: Option<String> = None;
    let mut completion_matcher: Option<String> = None;
    let mut history_limit: Option<u32> = None;
    let mut started_at: Option<String> = None;
    let mut updated_at: Option<String> = None;
    let mut extras = Vec::new();

    for line in frontmatter_text.lines() {
        let Some((raw_key, raw_value)) = line.split_once(':') else {
            continue;
        };
        let key = raw_key.trim().to_string();
        let value = raw_value.trim().trim_matches('"').to_string();
        match key.as_str() {
            "schema_version" => {
                if let Ok(parsed) = value.parse::<u32>() {
                    schema_version = parsed;
                }
            }
            "task_name" => {
                task_name = value;
            }
            "project_path" => {
                if !value.is_empty() {
                    project_path = Some(value);
                }
            }
            "active" => {
                active = matches!(
                    value.to_ascii_lowercase().as_str(),
                    "true" | "1" | "yes"
                );
            }
            "iteration" => {
                iteration = Some(value.parse::<u32>().map_err(|_| {
                    anyhow::anyhow!("invalid iteration value in state file")
                })?);
            }
            "max_iterations" => {
                max_iterations = Some(value.parse::<u32>().map_err(|_| {
                    anyhow::anyhow!("invalid max_iterations value in state file")
                })?);
            }
            "completion_promise" => {
                if value.is_empty() || value.eq_ignore_ascii_case("null") {
                    completion_promise = None;
                } else {
                    completion_promise = Some(value);
                }
            }
            "completion_matcher" => {
                if !value.is_empty() {
                    completion_matcher = normalize_matcher_name(value);
                }
            }
            "history_limit" => {
                if let Ok(parsed) = value.parse::<u32>() {
                    history_limit = Some(parsed);
                }
            }
            "started_at" => {
                if !value.is_empty() {
                    started_at = Some(value);
                }
            }
            "updated_at" => {
                if !value.is_empty() {
                    updated_at = Some(value);
                }
            }
            _ => extras.push((key, value)),
        }
    }

    Ok(StateData {
        schema_version,
        task_name,
        project_path,
        active,
        iteration: iteration.unwrap_or(0),
        max_iterations: max_iterations.unwrap_or(0),
        completion_promise,
        completion_matcher,
        history_limit,
        started_at,
        updated_at,
        prompt,
        extras,
    })
}

fn is_known_key(key: &str) -> bool {
    matches!(
        key,
        "schema_version"
            | "task_name"
            | "project_path"
            | "active"
            | "iteration"
            | "max_iterations"
            | "completion_promise"
            | "completion_matcher"
            | "history_limit"
            | "started_at"
            | "updated_at"
    )
}

fn with_lock<T, F>(lock_path: &Path, action: F) -> Result<T>
where
    F: FnOnce() -> Result<T>,
{
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(lock_path)?;
    file.lock_exclusive()?;
    let result = action();
    let _ = file.unlock();
    result
}

fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = path.with_extension(format!("tmp-{nanos}"));
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

fn append_history_event(path: &Path, history_limit: u32, event: serde_json::Value) {
    if let Some(parent) = path.parent() {
        if fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    if let Ok(line) = serde_json::to_string(&event) {
        let _ = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| writeln!(file, "{line}"));
    }
    if history_limit > 0 {
        let _ = prune_history(path, history_limit as usize);
    }
}

fn prune_history(path: &Path, limit: usize) -> Result<()> {
    let contents = std::fs::read_to_string(path)?;
    let mut lines: Vec<&str> = contents.lines().collect();
    if lines.len() <= limit {
        return Ok(());
    }
    lines = lines.split_off(lines.len() - limit);
    let mut output = lines.join("\n");
    if !output.is_empty() {
        output.push('\n');
    }
    write_atomic(path, output.as_bytes())?;
    Ok(())
}

fn read_last_history_event(path: &Path) -> Result<Option<serde_json::Value>> {
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(path)?;
    for line in contents.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn parse_timestamp(value: &str) -> Option<i64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(trimmed)
        .map(|dt| dt.timestamp())
        .ok()
}

fn derive_status(
    state: Option<&StateData>,
    last_event: Option<&serde_json::Value>,
) -> &'static str {
    if let Some(state) = state {
        return if state.active { "in_progress" } else { "paused" };
    }
    let event = last_event
        .and_then(|value| value.get("event"))
        .and_then(|value| value.as_str());
    match event {
        Some("promise_matched") | Some("max_iterations") => "completed",
        Some("cancel") | Some("delete") => "cancelled",
        Some("invalid_state")
        | Some("invalid_matcher")
        | Some("write_failed")
        | Some("empty_prompt") => "error",
        Some(_) => "completed",
        None => "error",
    }
}

#[derive(Debug)]
struct TaskSummary {
    task_name: String,
    task_dir: String,
    project_path: Option<String>,
    status: String,
    iteration: Option<u32>,
    max_iterations: Option<u32>,
    last_event: Option<String>,
    last_updated: Option<String>,
    sort_ts: i64,
}

impl TaskSummary {
    fn into_json(self, storage: &str) -> serde_json::Value {
        json!({
            "task_name": self.task_name,
            "task_dir": self.task_dir,
            "project_path": self.project_path,
            "status": self.status,
            "iteration": self.iteration,
            "max_iterations": self.max_iterations,
            "last_event": self.last_event,
            "last_updated": self.last_updated,
            "storage": storage,
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = TaskloopServer::new();
    let running = server.serve((tokio::io::stdin(), tokio::io::stdout())).await?;
    running.waiting().await?;
    Ok(())
}
