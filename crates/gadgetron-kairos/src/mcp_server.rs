//! Manual stdio MCP server — the `gadgetron mcp serve` entry point.
//!
//! Spec: `docs/design/phase2/01-knowledge-layer.md §6.1` (manual MCP
//! fallback path — `rmcp` integration deferred to P2B per that doc's
//! rationale).
//!
//! # Protocol
//!
//! Line-delimited JSON-RPC 2.0 over stdin (requests) and stdout
//! (responses + notifications). Each message is a single line of
//! minified JSON terminated by `\n`. The server reads one line at a
//! time, dispatches based on `method`, and writes one response line
//! per request that expects one (notifications like `initialized` get
//! no response).
//!
//! # Methods implemented
//!
//! - **`initialize`** — MCP protocol handshake. Returns the server's
//!   `protocolVersion`, `capabilities`, and `serverInfo`. Required
//!   before any tool-related method.
//! - **`initialized`** — Notification from the client that it's ready.
//!   We just silently ack (no response).
//! - **`tools/list`** — Returns the flattened list of tools from
//!   `McpToolRegistry::all_schemas()`. Each tool is shaped as
//!   `{ name, description, inputSchema }` per the MCP spec. Tool
//!   names are the raw registry names (no `mcp__<server>__` prefix —
//!   that transformation happens on the Claude Code side for
//!   `--allowed-tools`; the wire protocol uses the server's own names).
//! - **`tools/call`** — Dispatches through `McpToolRegistry::dispatch`
//!   and returns the `ToolResult` wrapped in the MCP result shape
//!   `{ content: [{ type: "text", text: ... }], isError }`.
//! - **Anything else** — Returns JSON-RPC error code `-32601`
//!   (method not found).
//!
//! # Lifecycle
//!
//! The server exits cleanly on stdin EOF — which happens when Claude
//! Code (the parent process) exits. This is the per-request-per-
//! subprocess model from 00-overview.md §5: one `claude -p`
//! invocation → one `gadgetron mcp serve` child → exits together.
//!
//! # Why manual JSON-RPC, not `rmcp`?
//!
//! `01 v3 §6` evaluated the `rmcp` Rust SDK and found its API
//! unstable at P2A authoring time. The manual implementation is ~100
//! lines and only needs to handle four MCP methods, so the complexity
//! cost of the dependency was judged higher than the protocol code
//! itself. P2B reopens the evaluation.

use std::sync::Arc;

use gadgetron_core::agent::tools::{McpError, ToolSchema};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::registry::McpToolRegistry;

/// JSON-RPC 2.0 request envelope — subset used by MCP.
#[derive(Debug, Deserialize)]
pub struct RpcRequest {
    #[allow(dead_code)]
    #[serde(default)]
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// JSON-RPC 2.0 response envelope.
#[derive(Debug, Serialize)]
pub struct RpcResponse {
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// JSON-RPC 2.0 error envelope (subset — we don't emit `data`).
#[derive(Debug, Serialize, PartialEq)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

impl RpcResponse {
    pub fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Option<Value>, error: RpcError) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// Run the stdio MCP server until stdin EOF.
///
/// `registry` is the frozen `McpToolRegistry` built by the caller
/// with all the providers it wants to expose (typically just
/// `KnowledgeToolProvider` in P2A).
pub async fn serve_stdio(registry: Arc<McpToolRegistry>) -> std::io::Result<()> {
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut stdout = tokio::io::stdout();
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break; // EOF — parent exited.
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request: RpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                // Malformed JSON → emit parse error response. Per
                // JSON-RPC 2.0, parse errors use id: null.
                let resp = RpcResponse::err(
                    None,
                    RpcError {
                        code: -32700,
                        message: format!("parse error: {e}"),
                    },
                );
                write_response(&mut stdout, &resp).await?;
                continue;
            }
        };

        // `initialized` is a notification (no response).
        if request.method == "initialized" {
            continue;
        }

        let response = handle_request(&registry, request).await;
        write_response(&mut stdout, &response).await?;
    }

    Ok(())
}

/// Dispatch one request against the registry. Pure async function
/// extracted from `serve_stdio` so unit tests can call it directly
/// without routing through real stdio.
pub async fn handle_request(registry: &McpToolRegistry, request: RpcRequest) -> RpcResponse {
    match request.method.as_str() {
        "initialize" => {
            let result = json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "gadgetron-knowledge",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            });
            RpcResponse::ok(request.id, result)
        }
        "tools/list" => {
            let tools: Vec<Value> = registry
                .all_schemas()
                .iter()
                .map(schema_to_mcp_tool_value)
                .collect();
            RpcResponse::ok(request.id, json!({ "tools": tools }))
        }
        "tools/call" => {
            let name = request
                .params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let arguments = request
                .params
                .get("arguments")
                .cloned()
                .unwrap_or(Value::Null);
            if name.is_empty() {
                return RpcResponse::err(
                    request.id,
                    RpcError {
                        code: -32602,
                        message: "missing 'name' parameter".to_string(),
                    },
                );
            }
            match registry.dispatch(name, arguments).await {
                Ok(tool_result) => {
                    let wrapped = json!({
                        "content": [{
                            "type": "text",
                            "text": tool_result.content.to_string()
                        }],
                        "isError": tool_result.is_error
                    });
                    RpcResponse::ok(request.id, wrapped)
                }
                Err(e) => RpcResponse::ok(request.id, mcp_error_as_tool_result(&e)),
            }
        }
        other => RpcResponse::err(
            request.id,
            RpcError {
                code: -32601,
                message: format!("method not found: {other}"),
            },
        ),
    }
}

/// Translate an `McpError` into an MCP `{ content, isError: true }`
/// payload. Keeps tool-level errors as successful JSON-RPC responses
/// with `isError: true`, matching the MCP spec's tool error model.
fn mcp_error_as_tool_result(err: &McpError) -> Value {
    let text = match err {
        McpError::UnknownTool(name) => format!("unknown tool: {name}"),
        McpError::Denied { reason } => format!("denied: {reason}"),
        McpError::RateLimited {
            tool,
            remaining,
            limit,
        } => format!("rate limited for {tool}: {remaining}/{limit} remaining this hour"),
        McpError::ApprovalTimeout { secs } => format!("approval timed out after {secs}s"),
        McpError::InvalidArgs(msg) => format!("invalid arguments: {msg}"),
        McpError::Execution(msg) => format!("execution failed: {msg}"),
    };
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": true
    })
}

/// Shape a `ToolSchema` into the MCP `tools/list` wire format.
fn schema_to_mcp_tool_value(schema: &ToolSchema) -> Value {
    json!({
        "name": schema.name,
        "description": schema.description,
        "inputSchema": schema.input_schema,
    })
}

/// Write a single response + newline, then flush.
async fn write_response(
    stdout: &mut tokio::io::Stdout,
    response: &RpcResponse,
) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(response)
        .map_err(|e| std::io::Error::other(format!("serialize response: {e}")))?;
    stdout.write_all(&bytes).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use gadgetron_core::agent::tools::{McpToolProvider, Tier, ToolResult};
    use std::sync::Arc;

    struct FakeProvider;

    #[async_trait]
    impl McpToolProvider for FakeProvider {
        fn category(&self) -> &'static str {
            "knowledge"
        }
        fn tool_schemas(&self) -> Vec<ToolSchema> {
            vec![
                ToolSchema {
                    name: "wiki.get".to_string(),
                    tier: Tier::Read,
                    description: "fetch a wiki page".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": { "name": { "type": "string" } },
                        "required": ["name"]
                    }),
                    idempotent: Some(true),
                },
                ToolSchema {
                    name: "wiki.write".to_string(),
                    tier: Tier::Write,
                    description: "write a wiki page".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "content": { "type": "string" }
                        },
                        "required": ["name", "content"]
                    }),
                    idempotent: Some(false),
                },
            ]
        }
        async fn call(&self, name: &str, _args: Value) -> Result<ToolResult, McpError> {
            match name {
                "wiki.get" => Ok(ToolResult {
                    content: json!({ "name": "home", "content": "# Home\n" }),
                    is_error: false,
                }),
                "wiki.write" => Err(McpError::Denied {
                    reason: "test denial".to_string(),
                }),
                _ => Err(McpError::UnknownTool(name.to_string())),
            }
        }
    }

    fn fresh_registry() -> Arc<McpToolRegistry> {
        let mut builder = crate::registry::McpToolRegistryBuilder::new();
        builder.register(Arc::new(FakeProvider)).unwrap();
        Arc::new(builder.freeze())
    }

    fn request(method: &str, params: Value) -> RpcRequest {
        RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: method.to_string(),
            params,
        }
    }

    // ---- initialize ----

    #[tokio::test]
    async fn initialize_returns_protocol_version() {
        let reg = fresh_registry();
        let resp = handle_request(&reg, request("initialize", json!({}))).await;
        assert!(resp.result.is_some());
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert!(result["capabilities"]["tools"].is_object());
        assert_eq!(result["serverInfo"]["name"], "gadgetron-knowledge");
    }

    // ---- tools/list ----

    #[tokio::test]
    async fn tools_list_returns_flattened_schemas() {
        let reg = fresh_registry();
        let resp = handle_request(&reg, request("tools/list", json!({}))).await;
        let result = resp.result.expect("result");
        let tools = result["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 2);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"wiki.get"));
        assert!(names.contains(&"wiki.write"));
        // Each entry must have description + inputSchema.
        assert!(tools[0]["description"].is_string());
        assert!(tools[0]["inputSchema"].is_object());
    }

    #[tokio::test]
    async fn tools_list_does_not_leak_tier() {
        // `tier` is an internal taxonomy; the MCP wire format does
        // not include it. Agents see only name + description + schema.
        let reg = fresh_registry();
        let resp = handle_request(&reg, request("tools/list", json!({}))).await;
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        assert!(tools.iter().all(|t| t.get("tier").is_none()));
    }

    // ---- tools/call ----

    #[tokio::test]
    async fn tools_call_happy_path_returns_content_array() {
        let reg = fresh_registry();
        let resp = handle_request(
            &reg,
            request(
                "tools/call",
                json!({
                    "name": "wiki.get",
                    "arguments": { "name": "home" }
                }),
            ),
        )
        .await;
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], false);
        let content = result["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        // The inner text is the JSON-encoded ToolResult.content.
        assert!(content[0]["text"].as_str().unwrap().contains("home"));
    }

    #[tokio::test]
    async fn tools_call_denied_returns_tool_error_not_rpc_error() {
        // McpError::Denied is an agent-visible "tool failed" signal,
        // not a protocol error. Per MCP spec, tool errors are
        // successful JSON-RPC responses with isError=true in the
        // result payload.
        let reg = fresh_registry();
        let resp = handle_request(
            &reg,
            request(
                "tools/call",
                json!({
                    "name": "wiki.write",
                    "arguments": { "name": "x", "content": "y" }
                }),
            ),
        )
        .await;
        assert!(resp.error.is_none(), "denied ≠ RPC error");
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("denied"));
        assert!(text.contains("test denial"));
    }

    #[tokio::test]
    async fn tools_call_unknown_tool_returns_tool_error_not_rpc_error() {
        let reg = fresh_registry();
        let resp = handle_request(
            &reg,
            request(
                "tools/call",
                json!({
                    "name": "does.not.exist",
                    "arguments": {}
                }),
            ),
        )
        .await;
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("unknown tool"));
    }

    #[tokio::test]
    async fn tools_call_missing_name_returns_invalid_params() {
        // Structural error — no "name" field at all. This IS a JSON-RPC
        // protocol error, distinct from a tool-level error.
        let reg = fresh_registry();
        let resp = handle_request(&reg, request("tools/call", json!({ "arguments": {} }))).await;
        let err = resp.error.expect("must be RPC error");
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("name"));
    }

    // ---- method not found ----

    #[tokio::test]
    async fn unknown_method_returns_32601() {
        let reg = fresh_registry();
        let resp = handle_request(&reg, request("ghost.method", json!({}))).await;
        let err = resp.error.expect("must be RPC error");
        assert_eq!(err.code, -32601);
        assert!(err.message.contains("ghost.method"));
    }

    // ---- id round trip ----

    #[tokio::test]
    async fn response_echoes_request_id() {
        let reg = fresh_registry();
        let mut req = request("tools/list", json!({}));
        req.id = Some(json!(42));
        let resp = handle_request(&reg, req).await;
        assert_eq!(resp.id, Some(json!(42)));
    }

    #[tokio::test]
    async fn response_handles_null_id() {
        // Notifications have id = null. We shouldn't emit a response
        // for notifications, but if somehow we do, it should echo
        // null cleanly.
        let reg = fresh_registry();
        let mut req = request("tools/list", json!({}));
        req.id = None;
        let resp = handle_request(&reg, req).await;
        assert!(resp.id.is_none());
    }
}
