use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use chrono::{DateTime, Utc};
use fs2::FileExt;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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
const DEFAULT_MAX_ITERATIONS: u32 = 0;

const KNOWN_KEYS: &[&str] = &[
    "schema_version",
    "task_name",
    "project_path",
    "active",
    "iteration",
    "max_iterations",
    "completion_promise",
    "completion_matcher",
    "history_limit",
    "started_at",
    "updated_at",
];

#[derive(Deserialize, Default)]
struct TaskloopConfigFile {
    default_max_iterations: Option<u32>,
    default_completion_matcher: Option<String>,
    history_limit: Option<u32>,
}

#[derive(Clone)]
struct ConfigDefaults {
    max_iterations: u32,
    completion_matcher: String,
    history_limit: u32,
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

#[derive(Clone)]
struct StorageRoot {
    root: PathBuf,
    project_path: String,
}

#[derive(Clone)]
struct Candidate {
    task_name: String,
    task_dir: String,
    state_path: PathBuf,
    history_path: PathBuf,
    root: PathBuf,
    active_hint: bool,
    sort_ts: i64,
}

#[derive(Clone)]
struct TaskState {
    schema_version: u32,
    task_name: String,
    project_path: String,
    active: bool,
    iteration: u32,
    max_iterations: u32,
    completion_promise: Option<String>,
    completion_matcher: Option<String>,
    history_limit: u32,
    started_at: String,
    updated_at: String,
}

#[derive(Default)]
struct Decision {
    decision: &'static str,
    reason: Option<String>,
    system_message: Option<String>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("codex-taskloop-hook error: {err}");
        output(&Decision {
            decision: "approve",
            ..Decision::default()
        });
    }
}

fn run() -> Result<()> {
    let hook_input = read_input_json();
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let project_path = canonical_project_path(&project_dir);
    let roots = storage_roots_for_hook(&project_dir, &project_path);
    let mut candidates = list_candidates(&roots)?;
    if candidates.is_empty() {
        output(&Decision {
            decision: "approve",
            ..Decision::default()
        });
        return Ok(());
    }
    candidates.sort_by(|a, b| b.sort_ts.cmp(&a.sort_ts));
    let defaults = load_config_defaults(&project_dir.join(".codex"));
    let last_message = extract_last_message(&hook_input);

    for candidate in candidates {
        if !candidate.active_hint {
            continue;
        }
        let state_path = candidate.state_path.clone();
        let history_path = candidate.history_path.clone();
        let task_root = candidate.root.join(&candidate.task_dir);
        let lock_path = lock_file_path(&task_root);

        let decision = with_lock(&lock_path, || {
            if !state_path.exists() {
                return Ok(None);
            }
            let (frontmatter, prompt) = match read_state(&state_path) {
                Ok(value) => value,
                Err(_) => {
                    let _ = fs::remove_file(&state_path);
                    append_history_event(
                        &history_path,
                        defaults.history_limit,
                        json!({
                            "ts": now_iso(),
                            "event": "invalid_state",
                            "task_name": candidate.task_name,
                            "project_path": project_path,
                            "task_dir": candidate.task_dir,
                            "state_file": state_path.display().to_string(),
                            "history_file": history_path.display().to_string(),
                            "decision": "approve",
                        }),
                    );
                    update_meta_task(
                        &candidate.root,
                        &candidate.task_dir,
                        Some("error"),
                        Some("invalid_state"),
                        None,
                        None,
                    );
                    return Ok(None);
                }
            };

            let extras = collect_extras(&frontmatter);
            let mut state = match parse_state(&frontmatter, &candidate.task_name, &project_path, &defaults) {
                Ok(value) => value,
                Err(_) => {
                    let _ = fs::remove_file(&state_path);
                    append_history_event(
                        &history_path,
                        defaults.history_limit,
                        json!({
                            "ts": now_iso(),
                            "event": "invalid_state",
                            "task_name": candidate.task_name,
                            "project_path": project_path,
                            "task_dir": candidate.task_dir,
                            "state_file": state_path.display().to_string(),
                            "history_file": history_path.display().to_string(),
                            "decision": "approve",
                        }),
                    );
                    update_meta_task(
                        &candidate.root,
                        &candidate.task_dir,
                        Some("error"),
                        Some("invalid_state"),
                        None,
                        None,
                    );
                    return Ok(None);
                }
            };

            if !state.active {
                let mut event = history_base(&state, &prompt, &state_path, &history_path, &candidate.task_dir);
                event["event"] = json!("paused");
                event["decision"] = json!("approve");
                append_history_event(&history_path, state.history_limit, event);
                update_meta_task(
                    &candidate.root,
                    &candidate.task_dir,
                    Some("paused"),
                    Some("paused"),
                    Some(state.iteration),
                    Some(state.max_iterations),
                );
                return Ok(None);
            }

            if state.max_iterations > 0 && state.iteration >= state.max_iterations {
                let _ = fs::remove_file(&state_path);
                let mut event = history_base(&state, &prompt, &state_path, &history_path, &candidate.task_dir);
                event["event"] = json!("max_iterations");
                event["decision"] = json!("approve");
                append_history_event(&history_path, state.history_limit, event);
                update_meta_task(
                    &candidate.root,
                    &candidate.task_dir,
                    Some("completed"),
                    Some("max_iterations"),
                    Some(state.iteration),
                    Some(state.max_iterations),
                );
                return Ok(Some(Decision {
                    decision: "approve",
                    ..Decision::default()
                }));
            }

            if let Some(promise) = state.completion_promise.clone() {
                if let Some(promise_text) = extract_promise_text(&last_message) {
                    let matcher = state
                        .completion_matcher
                        .clone()
                        .unwrap_or_else(|| DEFAULT_MATCHER.to_string());
                    match promise_matches(&promise_text, &promise, &matcher) {
                        None => {
                            let _ = fs::remove_file(&state_path);
                            let mut event = history_base(&state, &prompt, &state_path, &history_path, &candidate.task_dir);
                            event["event"] = json!("invalid_matcher");
                            event["decision"] = json!("approve");
                            event["last_message_excerpt"] = json!(snippet(&last_message));
                            append_history_event(&history_path, state.history_limit, event);
                            update_meta_task(
                                &candidate.root,
                                &candidate.task_dir,
                                Some("error"),
                                Some("invalid_matcher"),
                                Some(state.iteration),
                                Some(state.max_iterations),
                            );
                            return Ok(Some(Decision {
                                decision: "approve",
                                ..Decision::default()
                            }));
                        }
                        Some(true) => {
                            let _ = fs::remove_file(&state_path);
                            let mut event = history_base(&state, &prompt, &state_path, &history_path, &candidate.task_dir);
                            event["event"] = json!("promise_matched");
                            event["decision"] = json!("approve");
                            event["promise_text"] = json!(promise_text);
                            event["last_message_excerpt"] = json!(snippet(&last_message));
                            append_history_event(&history_path, state.history_limit, event);
                            update_meta_task(
                                &candidate.root,
                                &candidate.task_dir,
                                Some("completed"),
                                Some("promise_matched"),
                                Some(state.iteration),
                                Some(state.max_iterations),
                            );
                            return Ok(Some(Decision {
                                decision: "approve",
                                ..Decision::default()
                            }));
                        }
                        Some(false) => {}
                    }
                }
            }

            if prompt.trim().is_empty() {
                let _ = fs::remove_file(&state_path);
                let mut event = history_base(&state, &prompt, &state_path, &history_path, &candidate.task_dir);
                event["event"] = json!("empty_prompt");
                event["decision"] = json!("approve");
                append_history_event(&history_path, state.history_limit, event);
                update_meta_task(
                    &candidate.root,
                    &candidate.task_dir,
                    Some("error"),
                    Some("empty_prompt"),
                    Some(state.iteration),
                    Some(state.max_iterations),
                );
                return Ok(Some(Decision {
                    decision: "approve",
                    ..Decision::default()
                }));
            }

            state.iteration = state.iteration.saturating_add(1);
            state.updated_at = now_iso();
            if frontmatter.get("started_at").is_none() {
                state.started_at = state.updated_at.clone();
            }

            if let Err(_) = write_state(&state_path, &state, &extras, &prompt) {
                let _ = fs::remove_file(&state_path);
                let mut event = history_base(&state, &prompt, &state_path, &history_path, &candidate.task_dir);
                event["event"] = json!("write_failed");
                event["decision"] = json!("approve");
                append_history_event(&history_path, state.history_limit, event);
                update_meta_task(
                    &candidate.root,
                    &candidate.task_dir,
                    Some("error"),
                    Some("write_failed"),
                    Some(state.iteration),
                    Some(state.max_iterations),
                );
                return Ok(Some(Decision {
                    decision: "approve",
                    ..Decision::default()
                }));
            }

            let mut event = history_base(&state, &prompt, &state_path, &history_path, &candidate.task_dir);
            event["event"] = json!("loop");
            event["decision"] = json!("block");
            event["last_message_excerpt"] = json!(snippet(&last_message));
            append_history_event(&history_path, state.history_limit, event);
            update_meta_task(
                &candidate.root,
                &candidate.task_dir,
                Some("in_progress"),
                Some("loop"),
                Some(state.iteration),
                Some(state.max_iterations),
            );

            Ok(Some(Decision {
                decision: "block",
                reason: Some(prompt),
                system_message: Some(build_system_message(&state)),
            }))
        })?;

        if let Some(value) = decision {
            output(&value);
            return Ok(());
        }
    }

    output(&Decision {
        decision: "approve",
        ..Decision::default()
    });
    Ok(())
}

fn read_input_json() -> Value {
    let mut buf = String::new();
    if io::stdin().read_to_string(&mut buf).is_ok() {
        if let Ok(value) = serde_json::from_str::<Value>(&buf) {
            return value;
        }
    }
    Value::Null
}

fn read_state(path: &Path) -> Result<(HashMap<String, String>, String)> {
    let text = fs::read_to_string(path)?;
    if !text.starts_with("---") {
        anyhow::bail!("state file missing frontmatter");
    }
    let mut parts = text.splitn(3, "---");
    let _ = parts.next();
    let frontmatter = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("state file missing frontmatter"))?;
    let body = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("state file missing closing frontmatter"))?;
    let mut map = HashMap::new();
    for line in frontmatter.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        map.insert(
            key.trim().to_string(),
            value.trim().trim_matches('"').to_string(),
        );
    }
    Ok((map, body.trim_start_matches('\n').to_string()))
}

fn write_state(
    path: &Path,
    state: &TaskState,
    extras: &HashMap<String, String>,
    body: &str,
) -> Result<()> {
    let mut lines = Vec::new();
    lines.push("---".to_string());
    lines.push(format!("schema_version: {}", state.schema_version));
    lines.push(format!("task_name: {}", state.task_name));
    lines.push(format!("project_path: {}", state.project_path));
    lines.push(format!("active: {}", if state.active { "true" } else { "false" }));
    lines.push(format!("iteration: {}", state.iteration));
    lines.push(format!("max_iterations: {}", state.max_iterations));

    if let Some(promise) = state.completion_promise.as_deref() {
        let escaped = escape_yaml(promise)?;
        lines.push(format!("completion_promise: \"{escaped}\""));
    } else {
        lines.push("completion_promise: null".to_string());
    }

    if let Some(matcher) = state.completion_matcher.as_deref() {
        let escaped = escape_yaml(matcher)?;
        lines.push(format!("completion_matcher: \"{escaped}\""));
    }

    lines.push(format!("history_limit: {}", state.history_limit));
    lines.push(format!("started_at: \"{}\"", state.started_at));
    lines.push(format!("updated_at: \"{}\"", state.updated_at));

    for (key, value) in extras.iter() {
        if is_known_key(key) {
            continue;
        }
        lines.push(format!("{key}: {value}"));
    }
    lines.push("---".to_string());
    lines.push(String::new());
    lines.push(body.trim_end().to_string());
    lines.push(String::new());

    write_atomic(path, lines.join("\n").as_bytes())?;
    Ok(())
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

fn escape_yaml(value: &str) -> Result<String> {
    if value.contains('\n') || value.contains('\r') {
        anyhow::bail!("value must be a single line");
    }
    Ok(value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn load_config_defaults(codex_dir: &Path) -> ConfigDefaults {
    let path = codex_dir.join(CONFIG_FILE_NAME);
    if let Ok(contents) = fs::read_to_string(&path) {
        if let Ok(cfg) = toml::from_str::<TaskloopConfigFile>(&contents) {
            let matcher = normalize_matcher_name(
                cfg.default_completion_matcher
                    .unwrap_or_else(|| DEFAULT_MATCHER.to_string()),
            )
            .unwrap_or_else(|| DEFAULT_MATCHER.to_string());
            return ConfigDefaults {
                max_iterations: cfg.default_max_iterations.unwrap_or(DEFAULT_MAX_ITERATIONS),
                completion_matcher: matcher,
                history_limit: cfg.history_limit.unwrap_or(DEFAULT_HISTORY_LIMIT),
            };
        }
    }
    ConfigDefaults {
        max_iterations: DEFAULT_MAX_ITERATIONS,
        completion_matcher: DEFAULT_MATCHER.to_string(),
        history_limit: DEFAULT_HISTORY_LIMIT,
    }
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

fn parse_state(
    frontmatter: &HashMap<String, String>,
    fallback_task_name: &str,
    fallback_project_path: &str,
    defaults: &ConfigDefaults,
) -> Result<TaskState> {
    let schema_version = parse_u32(frontmatter.get("schema_version"), SCHEMA_VERSION, "schema")?;
    let active = parse_bool(frontmatter.get("active"), true);
    let iteration = parse_u32(frontmatter.get("iteration"), 0, "iteration")?;

    let max_iterations = match frontmatter.get("max_iterations") {
        Some(value) => parse_u32(Some(value), defaults.max_iterations, "max_iterations")?,
        None => defaults.max_iterations,
    };

    let mut completion_promise = frontmatter
        .get("completion_promise")
        .map(|value| value.trim().to_string());
    if let Some(value) = completion_promise.as_ref() {
        if value.is_empty() || value.eq_ignore_ascii_case("null") {
            completion_promise = None;
        }
    }

    let mut completion_matcher = frontmatter
        .get("completion_matcher")
        .and_then(|value| normalize_matcher_name(value.to_string()));
    if completion_promise.is_some() {
        if completion_matcher.is_none() {
            completion_matcher = Some(defaults.completion_matcher.clone());
        }
    } else {
        completion_matcher = None;
    }

    let history_limit = match frontmatter.get("history_limit") {
        Some(value) => parse_u32(Some(value), defaults.history_limit, "history_limit")?,
        None => defaults.history_limit,
    };

    let started_at = frontmatter
        .get("started_at")
        .cloned()
        .unwrap_or_else(now_iso);

    let task_name = frontmatter
        .get("task_name")
        .cloned()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| fallback_task_name.to_string());

    let project_path = frontmatter
        .get("project_path")
        .cloned()
        .filter(|path| !path.trim().is_empty())
        .unwrap_or_else(|| fallback_project_path.to_string());

    Ok(TaskState {
        schema_version,
        task_name,
        project_path,
        active,
        iteration,
        max_iterations,
        completion_promise,
        completion_matcher,
        history_limit,
        started_at,
        updated_at: now_iso(),
    })
}

fn parse_u32(value: Option<&String>, default: u32, field: &str) -> Result<u32> {
    let Some(value) = value else {
        return Ok(default);
    };
    if value.is_empty() {
        return Ok(default);
    }
    value
        .parse::<u32>()
        .map_err(|_| anyhow::anyhow!("invalid {field} value"))
}

fn parse_bool(value: Option<&String>, default: bool) -> bool {
    let Some(value) = value else {
        return default;
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => true,
        "false" | "0" | "no" => false,
        _ => default,
    }
}

fn local_store_root(project_dir: &Path) -> PathBuf {
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

fn global_store_root() -> PathBuf {
    codex_home_dir().join(STORE_DIR_NAME)
}

fn storage_roots_for_hook(project_dir: &Path, project_path: &str) -> Vec<StorageRoot> {
    vec![
        StorageRoot {
            root: local_store_root(project_dir),
            project_path: project_path.to_string(),
        },
        StorageRoot {
            root: global_store_root(),
            project_path: project_path.to_string(),
        },
    ]
}

fn list_candidates(roots: &[StorageRoot]) -> Result<Vec<Candidate>> {
    let mut candidates = Vec::new();
    for root in roots {
        let meta = match load_meta(&root.root) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        for task in meta.tasks {
            if task.project_path.as_deref() != Some(root.project_path.as_str()) {
                continue;
            }
            let task_root = root.root.join(&task.task_dir);
            let state_path = task_root.join(STATE_FILE_NAME);
            let history_path = task_root.join(HISTORY_FILE_NAME);
            if !state_path.exists() {
                continue;
            }
            let mut active_hint = true;
            let mut sort_ts = state_path
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(system_time_to_ts)
                .unwrap_or(0);

            if let Ok((frontmatter, _)) = read_state(&state_path) {
                active_hint = parse_bool(frontmatter.get("active"), true);
                if let Some(ts) = frontmatter
                    .get("updated_at")
                    .and_then(parse_timestamp)
                    .or_else(|| frontmatter.get("started_at").and_then(parse_timestamp))
                {
                    sort_ts = ts;
                }
            }
            candidates.push(Candidate {
                task_name: task.task_name,
                task_dir: task.task_dir,
                state_path,
                history_path,
                root: root.root.clone(),
                active_hint,
                sort_ts,
            });
        }
    }
    Ok(candidates)
}

fn history_base(
    state: &TaskState,
    prompt: &str,
    state_path: &Path,
    history_path: &Path,
    task_dir: &str,
) -> Value {
    json!({
        "ts": now_iso(),
        "task_name": state.task_name,
        "project_path": state.project_path,
        "task_dir": task_dir,
        "schema_version": state.schema_version,
        "active": state.active,
        "iteration": state.iteration,
        "max_iterations": state.max_iterations,
        "completion_promise": state.completion_promise,
        "completion_matcher": state.completion_matcher,
        "history_limit": state.history_limit,
        "prompt_preview": snippet(prompt),
        "state_file": state_path.display().to_string(),
        "history_file": history_path.display().to_string(),
    })
}

fn build_system_message(state: &TaskState) -> String {
    let iter_part = if state.max_iterations > 0 {
        let remaining = state.max_iterations.saturating_sub(state.iteration);
        format!(
            "iteration {}/{} (remaining {})",
            state.iteration, state.max_iterations, remaining
        )
    } else {
        format!("iteration {} (no max)", state.iteration)
    };
    if let Some(promise) = state.completion_promise.as_ref() {
        let matcher = state
            .completion_matcher
            .clone()
            .unwrap_or_else(|| DEFAULT_MATCHER.to_string());
        format!(
            "Taskloop task '{}' | {} | To stop: output <promise>{}</promise> (matcher: {}). Only when true.",
            state.task_name, iter_part, promise, matcher
        )
    } else {
        format!(
            "Taskloop task '{}' | {} | No completion promise set.",
            state.task_name, iter_part
        )
    }
}

fn extract_last_message(input: &Value) -> String {
    if let Some(value) = input.get("last_agent_message").and_then(|v| v.as_str()) {
        if !value.trim().is_empty() {
            return value.to_string();
        }
    }
    let rollout_path = match input.get("rollout_path").and_then(|v| v.as_str()) {
        Some(path) if !path.trim().is_empty() => path,
        _ => return String::new(),
    };
    let path = Path::new(rollout_path);
    let Ok(contents) = fs::read_to_string(path) else {
        return String::new();
    };
    for line in contents.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let payload = value.get("payload").and_then(|v| v.as_object());
        let Some(payload) = payload else {
            continue;
        };
        if payload.get("type").and_then(|v| v.as_str()) != Some("message") {
            continue;
        }
        if payload.get("role").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let content = payload.get("content").and_then(|v| v.as_array());
        let Some(content) = content else {
            continue;
        };
        let mut texts = Vec::new();
        for item in content {
            let Some(item) = item.as_object() else {
                continue;
            };
            let item_type = item.get("type").and_then(|v| v.as_str());
            if !matches!(item_type, Some("output_text") | Some("text")) {
                continue;
            }
            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                if !text.is_empty() {
                    texts.push(text.to_string());
                }
            }
        }
        if !texts.is_empty() {
            return texts.join("\n");
        }
    }
    String::new()
}

fn extract_promise_text(message: &str) -> Option<String> {
    let regex = RegexBuilder::new(r"<promise>(.*?)</promise>")
        .case_insensitive(true)
        .dot_matches_new_line(true)
        .build()
        .ok()?;
    let captures = regex.captures(message)?;
    captures.get(1).map(|m| normalize_space(m.as_str()))
}

fn promise_matches(promise_text: &str, completion_promise: &str, matcher: &str) -> Option<bool> {
    match matcher {
        "regex" => {
            let regex = RegexBuilder::new(completion_promise)
                .dot_matches_new_line(true)
                .build()
                .ok()?;
            Some(regex.is_match(promise_text))
        }
        "case_insensitive" => Some(
            normalize_space(promise_text).to_ascii_lowercase()
                == normalize_space(completion_promise).to_ascii_lowercase(),
        ),
        _ => Some(normalize_space(promise_text) == normalize_space(completion_promise)),
    }
}

fn normalize_space(value: &str) -> String {
    value.split_whitespace().collect::<Vec<&str>>().join(" ")
}

fn snippet(value: &str) -> String {
    let max_len = 200;
    if value.len() <= max_len {
        return value.to_string();
    }
    let mut out = value.chars().take(max_len).collect::<String>();
    out.push_str("...");
    out
}

fn append_history_event(path: &Path, history_limit: u32, event: Value) {
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
    let contents = fs::read_to_string(path)?;
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

fn output(decision: &Decision) {
    let mut payload = serde_json::Map::new();
    payload.insert("decision".to_string(), json!(decision.decision));
    if let Some(reason) = decision.reason.as_ref() {
        payload.insert("reason".to_string(), json!(reason));
    }
    if let Some(system) = decision.system_message.as_ref() {
        payload.insert("systemMessage".to_string(), json!(system));
    }
    println!("{}", Value::Object(payload));
}

fn now_iso() -> String {
    Utc::now().to_rfc3339()
}

fn parse_timestamp(value: &String) -> Option<i64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(trimmed)
        .map(|dt| dt.timestamp())
        .ok()
}

fn system_time_to_ts(time: SystemTime) -> Option<i64> {
    let dt: DateTime<Utc> = time.into();
    Some(dt.timestamp())
}

fn is_known_key(key: &str) -> bool {
    KNOWN_KEYS.iter().any(|k| k == &key)
}

fn collect_extras(frontmatter: &HashMap<String, String>) -> HashMap<String, String> {
    frontmatter
        .iter()
        .filter(|(k, _)| !is_known_key(k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn canonical_project_path(project_dir: &Path) -> String {
    fs::canonicalize(project_dir)
        .unwrap_or_else(|_| project_dir.to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn meta_file_path(root: &Path) -> PathBuf {
    root.join(META_FILE_NAME)
}

fn meta_lock_path(root: &Path) -> PathBuf {
    root.join(META_LOCK_NAME)
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
    let mut updated = meta.clone();
    updated.schema_version = SCHEMA_VERSION;
    let text = serde_json::to_string_pretty(&updated)?;
    write_atomic(&meta_file_path(root), format!("{text}\n").as_bytes())?;
    Ok(())
}

fn update_meta_task(
    root: &Path,
    task_dir: &str,
    status: Option<&str>,
    last_event: Option<&str>,
    iteration: Option<u32>,
    max_iterations: Option<u32>,
) {
    let meta_lock = meta_lock_path(root);
    let _ = with_lock(&meta_lock, || {
        let mut meta = load_meta(root)?;
        if let Some(task) = meta.tasks.iter_mut().find(|task| task.task_dir == task_dir) {
            if let Some(status) = status {
                task.status = Some(status.to_string());
            }
            if let Some(last_event) = last_event {
                task.last_event = Some(last_event.to_string());
            }
            if let Some(iteration) = iteration {
                task.iteration = Some(iteration);
            }
            if let Some(max_iterations) = max_iterations {
                task.max_iterations = Some(max_iterations);
            }
            task.updated_at = Some(now_iso());
            write_meta(root, &meta)?;
        }
        Ok(())
    });
}

fn lock_file_path(task_root: &Path) -> PathBuf {
    task_root.join(LOCK_FILE_NAME)
}
