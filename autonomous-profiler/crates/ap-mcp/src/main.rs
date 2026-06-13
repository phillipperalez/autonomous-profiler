//! `ap-mcp` — a minimal MCP server (JSON-RPC 2.0 over newline-delimited stdio)
//! exposing the profiler as tools. Deliberately dependency-light so the skeleton
//! always builds; swapping in the `rmcp` SDK for richer schemas/streaming is
//! hack-day fan-out item #7. Tool bodies are thin calls into
//! `ap_collectors::pipeline` + `ap_core::compile`, mirroring the CLI.

use ap_collectors::pipeline::{self, ProfileRecord, ProfileRequest};
use ap_core::collector::{Mode, Target};
use ap_core::compile::{compile, CompileOpts};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const SERVER_NAME: &str = "autonomous-profiler";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_PROTOCOL: &str = "2025-06-18";

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) if !l.trim().is_empty() => l,
            Ok(_) => continue,
            Err(_) => break,
        };
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let id = req.get("id").cloned();
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(Value::Null);

        // Notifications (no id) get processed but never answered.
        let response = match method {
            "initialize" => Some(ok(id, initialize(&params))),
            "tools/list" => Some(ok(id, json!({ "tools": tool_specs() }))),
            "tools/call" => Some(call_tool(id, &params)),
            "ping" => Some(ok(id, json!({}))),
            _ if id.is_some() => Some(err(id, -32601, &format!("method not found: {method}"))),
            _ => None,
        };

        if let Some(resp) = response {
            let _ = writeln!(stdout, "{resp}");
            let _ = stdout.flush();
        }
    }
}

fn initialize(params: &Value) -> Value {
    let protocol = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_PROTOCOL);
    json!({
        "protocolVersion": protocol,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION }
    })
}

fn ok(id: Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn err(id: Option<Value>, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Wrap a tool result as MCP `content`. `is_error` surfaces tool failures to the
/// model without failing the JSON-RPC call.
fn tool_result(text: String, is_error: bool) -> Value {
    json!({
        "content": [ { "type": "text", "text": text } ],
        "isError": is_error
    })
}

fn call_tool(id: Option<Value>, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let result = match name {
        "profile_target" => tool_profile_target(&args),
        "context_bundle" => tool_context_bundle(&args),
        "list_hot_functions" => tool_list_hot(&args),
        "memory_hotspots" => tool_memory_hotspots(&args),
        _ => Err(format!("unknown tool: {name}")),
    };
    match result {
        Ok(text) => ok(id, tool_result(text, false)),
        Err(e) => ok(id, tool_result(e, true)),
    }
}

// --- tool implementations -------------------------------------------------

fn tool_profile_target(args: &Value) -> Result<String, String> {
    let target_dir = str_arg(args, "target_dir").ok_or("target_dir is required")?;
    let mode = match str_arg(args, "mode").as_deref() {
        Some("alloc") => Mode::Alloc,
        _ => Mode::Cpu,
    };
    let rate = args.get("rate").and_then(Value::as_u64).unwrap_or(1000) as u32;
    let backend = str_arg(args, "backend").unwrap_or_else(|| "samply".into());

    let (target, label, mut roots) = if let Some(bin) = str_arg(args, "bin") {
        let p = PathBuf::from(&bin);
        let roots = p.parent().map(|x| vec![x.to_path_buf()]).unwrap_or_default();
        (Target::Binary { path: p, args: vec![] }, bin, roots)
    } else if let Some(example) = str_arg(args, "example") {
        (
            Target::CargoExample {
                dir: PathBuf::from(&target_dir),
                name: example.clone(),
                features: str_vec(args, "features"),
                args: str_vec(args, "args"),
            },
            example,
            vec![PathBuf::from(&target_dir)],
        )
    } else if mode == Mode::Alloc {
        (Target::Pid(0), "alloc".into(), vec![PathBuf::from(&target_dir)])
    } else {
        return Err("provide `example` or `bin`".into());
    };
    roots.push(PathBuf::from("."));

    let req = ProfileRequest {
        workload: format!("{label} ({mode:?})"),
        target,
        mode,
        rate_hz: rate,
        backend_id: backend,
        dhat_json: str_arg(args, "dhat_json").map(PathBuf::from),
        source_roots: roots,
        repo_dir: Some(PathBuf::from(&target_dir)),
    };

    let record = pipeline::run_profile(req).map_err(|e| format!("{e:#}"))?;
    let id = derive_id(&label);
    pipeline::save(&id, &record).map_err(|e| format!("{e:#}"))?;
    let findings_dir = str_arg(args, "findings_dir").unwrap_or_else(|| "data".into());
    let _ = pipeline::write_findings(std::path::Path::new(&findings_dir), &id, &record);

    let top: Vec<Value> = record
        .model
        .functions
        .iter()
        .take(5)
        .map(|f| json!({ "fn": f.demangled, "self_pct": record.model.self_pct(f) }))
        .collect();
    Ok(json!({
        "profile_id": id,
        "functions": record.model.functions.len(),
        "total_weight": record.model.total_weight,
        "unit": record.model.unit.label(),
        "top": top,
        "hint": "call context_bundle with this profile_id"
    })
    .to_string())
}

fn tool_context_bundle(args: &Value) -> Result<String, String> {
    let record = load_record(args)?;
    let opts = CompileOpts {
        token_budget: args.get("token_budget").and_then(Value::as_u64).unwrap_or(8000) as usize,
        focus: str_arg(args, "focus"),
        source_ctx_lines: args.get("ctx_lines").and_then(Value::as_u64).unwrap_or(6) as usize,
        source_roots: record.source_roots.clone(),
        ..Default::default()
    };
    let bundle = compile(&record.model, &opts);
    if str_arg(args, "format").as_deref() == Some("json") {
        Ok(serde_json::to_string_pretty(&bundle).unwrap_or_default())
    } else {
        Ok(bundle.to_markdown())
    }
}

fn tool_list_hot(args: &Value) -> Result<String, String> {
    let record = load_record(args)?;
    let top = args.get("top").and_then(Value::as_u64).unwrap_or(15) as usize;
    let filter = str_arg(args, "crate");
    let model = &record.model;
    let rows: Vec<Value> = model
        .functions
        .iter()
        .filter(|f| filter.as_ref().map(|c| f.crate_name.contains(c)).unwrap_or(true))
        .take(top)
        .map(|f| {
            json!({
                "fn": f.demangled,
                "crate": f.crate_name,
                "self_pct": model.self_pct(f),
                "total_pct": model.total_pct(f),
                "source": f.source.as_ref().map(|s| format!("{}:{}", s.file, s.line)),
            })
        })
        .collect();
    Ok(serde_json::to_string_pretty(&rows).unwrap_or_default())
}

fn tool_memory_hotspots(args: &Value) -> Result<String, String> {
    tool_list_hot(args)
}

// --- helpers --------------------------------------------------------------

fn load_record(args: &Value) -> Result<ProfileRecord, String> {
    let id = match str_arg(args, "profile_id") {
        Some(id) => id,
        None => pipeline::latest_id().ok_or("no cached profiles")?,
    };
    pipeline::load(&id).map_err(|e| format!("{e:#}"))
}

fn str_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(|s| s.to_string())
}

fn str_vec(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

fn derive_id(label: &str) -> String {
    let slug: String = label
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
        % 1_000_000;
    format!("{slug}-{ms}")
}

fn tool_specs() -> Value {
    json!([
        {
            "name": "profile_target",
            "description": "Profile a Rust target (cargo example or prebuilt binary) and cache the result. Returns a profile_id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "target_dir": { "type": "string", "description": "Cargo project directory" },
                    "example": { "type": "string", "description": "cargo example name to build+profile" },
                    "bin": { "type": "string", "description": "path to a prebuilt binary (alternative to example)" },
                    "mode": { "type": "string", "enum": ["cpu", "alloc"], "description": "default cpu" },
                    "dhat_json": { "type": "string", "description": "for mode=alloc: dhat-heap.json to ingest" },
                    "features": { "type": "array", "items": { "type": "string" } },
                    "rate": { "type": "integer", "description": "CPU sampling rate Hz (default 1000)" },
                    "backend": { "type": "string", "description": "CPU backend id (default samply)" }
                },
                "required": ["target_dir"]
            }
        },
        {
            "name": "context_bundle",
            "description": "Compressed, source-attributed hotspot bundle for an LLM. Markdown by default.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "profile_id": { "type": "string", "description": "default: most recent" },
                    "token_budget": { "type": "integer", "description": "default 8000" },
                    "focus": { "type": "string", "description": "crate/symbol substring to focus on (e.g. polars)" },
                    "ctx_lines": { "type": "integer", "description": "source context lines (default 6)" },
                    "format": { "type": "string", "enum": ["md", "json"], "description": "default md" }
                }
            }
        },
        {
            "name": "list_hot_functions",
            "description": "Ranked hottest functions (self/total %) from a cached profile.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "profile_id": { "type": "string" },
                    "top": { "type": "integer", "description": "default 15" },
                    "crate": { "type": "string", "description": "restrict to crate substring" }
                }
            }
        },
        {
            "name": "memory_hotspots",
            "description": "Heaviest allocation sites from a cached alloc profile (run profile_target with mode=alloc first).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "profile_id": { "type": "string" },
                    "top": { "type": "integer" }
                }
            }
        }
    ])
}
