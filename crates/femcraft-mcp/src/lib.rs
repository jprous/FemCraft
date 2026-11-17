//! FemCraft's built-in MCP (Model Context Protocol) server.
//!
//! Methods mimic RFEM 6's API I (see [`api`]); each is one MCP tool with a typed schema.
//! Two transports:
//! - stdio, headless: `femcraft --mcp` ([`stdio::run`] over a [`headless::Headless`] session);
//! - HTTP inside the running app ([`http::serve`]), so edits show in its window. MCP clients
//!   connect to `http://127.0.0.1:7980/mcp` ("streamable HTTP", JSON responses only).

pub mod api;
pub mod headless;
pub mod http;
pub mod stdio;

pub use api::{ModelInfo, Workspace};

use serde_json::{Value as Json, json};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// Default port of the HTTP transport (CADCraft uses 7979).
pub const DEFAULT_PORT: u16 = 7980;

const INSTRUCTIONS: &str = "FemCraft structural modelling (RFEM 6 .rf6 files). Methods follow \
RFEM 6 API I: new_model / open_model, then set_<object>({value: {no, ...fields}}) to create or \
change objects, get_<object>({no}), get_all_object_numbers({type}), and save / save_as to write \
the .rf6. Units are SI (m, N, Pa). RFEM's Z axis points down: gravity loads are positive along Z. \
Lines join nodes (definition_nodes); members sit on a line (line, section_start); surfaces are \
bounded by a closed loop of lines (boundary_lines) and take a thickness. Loads need \
load_case_no and an existing load case. Nothing is written to disk until save / save_as.";

/// Handles one JSON-RPC message; `None` for notifications (no response).
pub fn handle(ws: &mut dyn Workspace, msg: &Json) -> Option<Json> {
    let id = msg.get("id")?.clone();
    let method = msg.get("method").and_then(Json::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Json::Null);
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params.get("protocolVersion").and_then(Json::as_str)
                .unwrap_or(PROTOCOL_VERSION),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "femcraft", "version": env!("CARGO_PKG_VERSION")},
            "instructions": INSTRUCTIONS,
        })),
        "ping" => Ok(json!({})),
        "tools/list" => {
            let tools: Vec<Json> = api::methods()
                .into_iter()
                .map(|m| {
                    json!({"name": m.name, "description": m.description, "inputSchema": m.schema,
                           "annotations": {"readOnlyHint": !m.changes_model}})
                })
                .collect();
            Ok(json!({"tools": tools}))
        }
        "tools/call" => {
            let name = params.get("name").and_then(Json::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            if !api::methods().iter().any(|m| m.name == name) {
                Err((-32602, format!("unknown tool '{name}'")))
            } else {
                // Method errors are tool results with isError, so the model can correct itself.
                Ok(match api::call(ws, name, &args) {
                    Ok(v) => json!({"content": [{"type": "text", "text": v.to_string()}],
                                    "structuredContent": {"result": v}}),
                    Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
                })
            }
        }
        _ => Err((-32601, format!("method '{method}' not found"))),
    };
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err((code, m)) => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": m}})
        }
    })
}

/// Handles a message body (a message or a batch); `None` when nothing is to be sent back.
pub fn handle_text(ws: &mut dyn Workspace, body: &str) -> Option<String> {
    let parsed: Json = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => {
            return Some(
                json!({"jsonrpc": "2.0", "id": null,
                       "error": {"code": -32700, "message": format!("parse error: {e}")}})
                .to_string(),
            );
        }
    };
    match parsed {
        Json::Array(batch) => {
            let out: Vec<Json> = batch.iter().filter_map(|m| handle(ws, m)).collect();
            (!out.is_empty()).then(|| Json::Array(out).to_string())
        }
        m => handle(ws, &m).map(|r| r.to_string()),
    }
}
