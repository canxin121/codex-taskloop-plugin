use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde_json::{json, Map, Value};

const HOOK_KEY: &str = "Stop";

fn main() {
    if let Err(err) = run() {
        eprintln!("codex-taskloop-admin error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        print_usage();
        return Ok(());
    }

    let command = args.remove(0);
    match command.as_str() {
        "hooks" => handle_hooks(args),
        "mcp" => handle_mcp(args),
        "stop-hooks" => handle_stop_hooks(args),
        _ => {
            print_usage();
            bail!("unknown command: {command}");
        }
    }
}

fn handle_hooks(args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        print_usage();
        bail!("missing hooks subcommand");
    }
    let sub = &args[0];
    let flags = parse_flags(&args[1..])?;
    let project = flags.get("project").ok_or_else(|| anyhow::anyhow!("--project required"))?;
    let command = flags.get("command").ok_or_else(|| anyhow::anyhow!("--command required"))?;

    let hooks_path = Path::new(project).join(".codex").join("hooks").join("hooks.json");
    let mut root = load_json(&hooks_path)?;

    let changed = match sub.as_str() {
        "add" => add_hook(&mut root, command),
        "remove" => remove_hook(&mut root, command),
        _ => {
            print_usage();
            bail!("unknown hooks subcommand: {sub}");
        }
    };

    if changed {
        save_json(&hooks_path, &root)?;
    }
    Ok(())
}

fn handle_mcp(args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        print_usage();
        bail!("missing mcp subcommand");
    }
    let sub = &args[0];
    let flags = parse_flags(&args[1..])?;
    let name = flags.get("name").ok_or_else(|| anyhow::anyhow!("--name required"))?;
    let codex_home = env::var("CODEX_HOME").ok().filter(|v| !v.trim().is_empty());

    let cfg_path = codex_config_path()?;
    let mut contents = String::new();
    if cfg_path.exists() {
        contents = fs::read_to_string(&cfg_path)?;
    }

    contents = remove_block(&contents, name);
    if sub == "add" {
        let command = flags.get("command").ok_or_else(|| anyhow::anyhow!("--command required"))?;
        let project = flags.get("project").cloned();
        let mut storage_scope = flags.get("storage-scope").cloned();
        if project.is_some() && storage_scope.is_none() {
            storage_scope = Some("local-only".to_string());
        }
        contents = append_block(
            &contents,
            name,
            command,
            project.as_deref(),
            codex_home.as_deref(),
            storage_scope.as_deref(),
        );
    } else if sub != "remove" {
        print_usage();
        bail!("unknown mcp subcommand: {sub}");
    }

    if let Some(parent) = cfg_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(cfg_path, contents)?;
    Ok(())
}

fn handle_stop_hooks(args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        print_usage();
        bail!("missing stop-hooks subcommand");
    }
    let sub = &args[0];
    let flags = parse_flags(&args[1..])?;
    let name = flags.get("name").ok_or_else(|| anyhow::anyhow!("--name required"))?;

    let cfg_path = codex_config_path()?;
    let mut contents = String::new();
    if cfg_path.exists() {
        contents = fs::read_to_string(&cfg_path)?;
    }

    contents = remove_stop_hook_block(&contents, name);
    if sub == "add" {
        let command = flags.get("command").ok_or_else(|| anyhow::anyhow!("--command required"))?;
        let order = flags
            .get("order")
            .and_then(|value| value.parse::<i64>().ok());
        let timeout = flags
            .get("timeout")
            .and_then(|value| value.parse::<u64>().ok());
        let timeout_ms = flags
            .get("timeout-ms")
            .and_then(|value| value.parse::<u64>().ok());
        contents = append_stop_hook_block(&contents, name, command, order, timeout, timeout_ms);
    } else if sub != "remove" {
        print_usage();
        bail!("unknown stop-hooks subcommand: {sub}");
    }

    if let Some(parent) = cfg_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(cfg_path, contents)?;
    Ok(())
}

fn parse_flags(args: &[String]) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    let mut idx = 0;
    while idx < args.len() {
        let key = &args[idx];
        if !key.starts_with("--") {
            bail!("expected flag starting with --, got: {key}");
        }
        if idx + 1 >= args.len() {
            bail!("missing value for flag: {key}");
        }
        map.insert(key.trim_start_matches("--").to_string(), args[idx + 1].clone());
        idx += 2;
    }
    Ok(map)
}

fn load_json(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(Value::Object(Map::new()));
    }
    let text = fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&text)?;
    if value.is_object() {
        Ok(value)
    } else {
        Ok(Value::Object(Map::new()))
    }
}

fn save_json(path: &Path, data: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(data)?;
    fs::write(path, format!("{text}\n"))?;
    Ok(())
}

fn hook_matches(value: &Value, command: &str) -> bool {
    value
        .as_object()
        .and_then(|obj| obj.get("command"))
        .and_then(|v| v.as_str())
        .map(|v| v == command)
        .unwrap_or(false)
}

fn filter_items(items: &[Value], command: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for item in items {
        if let Some(obj) = item.as_object() {
            if let Some(inner_hooks) = obj.get("hooks").and_then(|v| v.as_array()) {
                let filtered = inner_hooks
                    .iter()
                    .filter(|h| !hook_matches(h, command))
                    .cloned()
                    .collect::<Vec<Value>>();
                if !filtered.is_empty() {
                    let mut new_obj = obj.clone();
                    new_obj.insert("hooks".to_string(), Value::Array(filtered));
                    out.push(Value::Object(new_obj));
                }
                continue;
            }
        }
        if hook_matches(item, command) {
            continue;
        }
        out.push(item.clone());
    }
    out
}

fn ensure_stop_list(root: &mut Value) -> &mut Vec<Value> {
    if !root.is_object() {
        *root = Value::Object(Map::new());
    }
    let obj = root.as_object_mut().expect("root must be object");
    if !obj.contains_key("hooks") {
        let mut hooks_map = Map::new();
        if let Some(stop_value) = obj.remove(HOOK_KEY) {
            if stop_value.is_array() {
                hooks_map.insert(HOOK_KEY.to_string(), stop_value);
            }
        }
        hooks_map
            .entry(HOOK_KEY.to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        obj.insert("hooks".to_string(), Value::Object(hooks_map));
    }

    let hooks = obj
        .get_mut("hooks")
        .and_then(|v| v.as_object_mut())
        .expect("hooks must be object");
    hooks
        .entry(HOOK_KEY.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    hooks
        .get_mut(HOOK_KEY)
        .and_then(|v| v.as_array_mut())
        .expect("Stop list should be array")
}

fn add_hook(root: &mut Value, command: &str) -> bool {
    let stop_list = ensure_stop_list(root);
    for item in stop_list.iter() {
        if hook_matches(item, command) {
            return false;
        }
        if let Some(inner_hooks) = item.as_object().and_then(|v| v.get("hooks")).and_then(|v| v.as_array()) {
            if inner_hooks.iter().any(|h| hook_matches(h, command)) {
                return false;
            }
        }
    }
    stop_list.push(json!({"type": "command", "command": command}));
    true
}

fn remove_hook(root: &mut Value, command: &str) -> bool {
    let mut changed = false;
    if let Some(obj) = root.as_object_mut() {
        if let Some(hooks) = obj.get_mut("hooks").and_then(|v| v.as_object_mut()) {
            if let Some(stop) = hooks.get_mut(HOOK_KEY).and_then(|v| v.as_array_mut()) {
                let filtered = filter_items(stop, command);
                if filtered != *stop {
                    changed = true;
                    if filtered.is_empty() {
                        hooks.remove(HOOK_KEY);
                    } else {
                        *stop = filtered;
                    }
                }
            }
            if hooks.get(HOOK_KEY).is_none() {
                if hooks.is_empty() {
                    obj.remove("hooks");
                }
            }
        }
        if let Some(stop) = obj.get_mut(HOOK_KEY).and_then(|v| v.as_array_mut()) {
            let filtered = filter_items(stop, command);
            if filtered != *stop {
                changed = true;
                if filtered.is_empty() {
                    obj.remove(HOOK_KEY);
                } else {
                    *stop = filtered;
                }
            }
        }
    }
    changed
}

fn codex_config_path() -> Result<PathBuf> {
    if let Ok(home) = env::var("CODEX_HOME") {
        return Ok(PathBuf::from(home).join("config.toml"));
    }
    let home = env::var("HOME").map_err(|_| anyhow::anyhow!("HOME not set"))?;
    Ok(PathBuf::from(home).join(".codex").join("config.toml"))
}

fn remove_block(contents: &str, name: &str) -> String {
    let mut out = Vec::new();
    let mut skipping = false;
    let header_prefix = format!("[mcp_servers.{name}");
    for line in contents.lines() {
        let stripped = line.trim();
        if stripped.starts_with('[') && stripped.ends_with(']') {
            if stripped.starts_with(&header_prefix) {
                skipping = true;
                continue;
            }
            if skipping {
                skipping = false;
            }
        }
        if skipping {
            continue;
        }
        out.push(line);
    }
    let mut result = out.join("\n");
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

fn append_block(
    contents: &str,
    name: &str,
    command: &str,
    project: Option<&str>,
    codex_home: Option<&str>,
    storage_scope: Option<&str>,
) -> String {
    let mut out = contents.trim_end().to_string();
    out.push_str("\n\n");
    out.push_str(&format!("[mcp_servers.{name}]\n"));
    out.push_str(&format!("command = \"{command}\"\n"));
    out.push_str("args = []\n");
    out.push_str("enabled = true\n");
    let mut env_items = Vec::new();
    if let Some(project) = project {
        env_items.push(format!("CODEX_CWD = \"{project}\""));
    }
    if let Some(codex_home) = codex_home {
        env_items.push(format!("CODEX_HOME = \"{codex_home}\""));
    }
    if let Some(storage_scope) = storage_scope {
        env_items.push(format!("TASKLOOP_STORAGE_SCOPE = \"{storage_scope}\""));
    }
    if !env_items.is_empty() {
        out.push_str(&format!("env = {{ {} }}\n", env_items.join(", ")));
    }
    out.push('\n');
    out
}

fn remove_stop_hook_block(contents: &str, name: &str) -> String {
    let mut out = Vec::new();
    let mut skipping = false;
    let header_prefix = format!("[stop_hooks.sources.{name}");
    for line in contents.lines() {
        let stripped = line.trim();
        if stripped.starts_with('[') && stripped.ends_with(']') {
            if stripped.starts_with(&header_prefix) {
                skipping = true;
                continue;
            }
            if skipping {
                skipping = false;
            }
        }
        if skipping {
            continue;
        }
        out.push(line);
    }
    let mut result = out.join("\n");
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

fn append_stop_hook_block(
    contents: &str,
    name: &str,
    command: &str,
    order: Option<i64>,
    timeout: Option<u64>,
    timeout_ms: Option<u64>,
) -> String {
    let mut out = contents.trim_end().to_string();
    out.push_str("\n\n");
    out.push_str(&format!("[stop_hooks.sources.{name}]\n"));
    out.push_str("type = \"command\"\n");
    out.push_str(&format!("command = \"{command}\"\n"));
    out.push_str("enabled = true\n");
    if let Some(order) = order {
        out.push_str(&format!("order = {order}\n"));
    }
    if let Some(timeout) = timeout {
        out.push_str(&format!("timeout = {timeout}\n"));
    }
    if let Some(timeout_ms) = timeout_ms {
        out.push_str(&format!("timeout_ms = {timeout_ms}\n"));
    }
    out.push('\n');
    out
}

fn print_usage() {
    eprintln!("Usage:");
    eprintln!("  codex-taskloop-admin hooks add --project <dir> --command <path>");
    eprintln!("  codex-taskloop-admin hooks remove --project <dir> --command <path>");
    eprintln!("  codex-taskloop-admin mcp add --name <name> --command <path> [--project <dir>] [--storage-scope <value>]");
    eprintln!("  codex-taskloop-admin mcp remove --name <name>");
    eprintln!("  codex-taskloop-admin stop-hooks add --name <name> --command <path> [--order N] [--timeout N] [--timeout-ms N]");
    eprintln!("  codex-taskloop-admin stop-hooks remove --name <name>");
}
