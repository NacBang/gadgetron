//! Job-scoped OpenAI Responses compatibility for Codex custom providers.
//!
//! Current Codex versions expose MCP catalogs as `type="namespace"` tools.
//! Some otherwise-compatible Local endpoints only implement ordinary function
//! tools. This loopback-only bridge flattens namespaces on the upstream request
//! and restores namespace identity on function-call responses so Codex's own
//! MCP router still performs dispatch, policy, and audit work.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderName, Method, Request, Response, StatusCode, Uri};
use axum::Router;
use futures::StreamExt;
use reqwest::Url;
use serde_json::Value;
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

type NamespaceMap = HashMap<String, (String, String)>;
const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Error)]
pub(crate) enum ResponsesBridgeError {
    #[error("invalid OpenAI-compatible base URL")]
    InvalidBaseUrl,
    #[error("failed to bind loopback Responses bridge: {0}")]
    Bind(#[source] std::io::Error),
    #[error("failed to build Responses bridge client: {0}")]
    Client(#[source] reqwest::Error),
}

#[derive(Clone)]
struct BridgeState {
    client: reqwest::Client,
    target_base_url: Url,
    shutdown: watch::Sender<bool>,
}

/// Listener handle retained for exactly one Codex subprocess invocation.
pub(crate) struct ResponsesBridge {
    base_url: String,
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl ResponsesBridge {
    pub(crate) async fn start(target_base_url: &str) -> Result<Self, ResponsesBridgeError> {
        let target_base_url = Url::parse(target_base_url)
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https"))
            .ok_or(ResponsesBridgeError::InvalidBaseUrl)?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .map_err(ResponsesBridgeError::Client)?;
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(ResponsesBridgeError::Bind)?;
        let address = listener.local_addr().map_err(ResponsesBridgeError::Bind)?;
        let (shutdown, _) = watch::channel(false);
        let state = Arc::new(BridgeState {
            client,
            target_base_url,
            shutdown: shutdown.clone(),
        });
        let app = Router::new().fallback(proxy_request).with_state(state);
        let task = tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, app).await {
                tracing::warn!(
                    target: "penny_responses_bridge",
                    error = %error,
                    "Local Responses bridge stopped unexpectedly"
                );
            }
        });
        Ok(Self {
            base_url: format!("http://{address}/v1"),
            shutdown,
            task,
        })
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }
}

impl Drop for ResponsesBridge {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        self.task.abort();
    }
}

async fn proxy_request(
    State(state): State<Arc<BridgeState>>,
    request: Request<Body>,
) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let body = match axum::body::to_bytes(body, MAX_REQUEST_BYTES).await {
        Ok(body) => body,
        Err(_) => return bridge_error(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let (body, namespaces) = if parts.method == Method::POST
        && parts.uri.path().trim_end_matches('/') == "/v1/responses"
    {
        match rewrite_request(&body) {
            Ok(rewritten) => rewritten,
            Err(_) => return bridge_error(StatusCode::BAD_REQUEST, "invalid Responses request"),
        }
    } else {
        (body.to_vec(), NamespaceMap::new())
    };

    let mut upstream = state.client.request(
        parts.method.clone(),
        target_url(&state.target_base_url, &parts.uri),
    );
    for (name, value) in &parts.headers {
        if !is_request_hop_header(name) {
            upstream = upstream.header(name, value);
        }
    }
    if !body.is_empty() {
        upstream = upstream.body(body);
    }
    let mut shutdown = state.shutdown.subscribe();
    let upstream = tokio::select! {
        response = upstream.send() => match response {
            Ok(response) => response,
            Err(_) => return bridge_error(StatusCode::BAD_GATEWAY, "compatible LLM endpoint unavailable"),
        },
        _ = shutdown.changed() => {
            return bridge_error(StatusCode::BAD_GATEWAY, "Responses bridge stopped");
        }
    };
    let status = upstream.status();
    let headers = upstream.headers().clone();
    let mut chunks = upstream.bytes_stream();
    let mut body = Vec::new();
    loop {
        let next = tokio::select! {
            next = chunks.next() => next,
            _ = shutdown.changed() => {
                return bridge_error(StatusCode::BAD_GATEWAY, "Responses bridge stopped");
            }
        };
        let Some(next) = next else {
            break;
        };
        let chunk = match next {
            Ok(chunk) => chunk,
            Err(_) => {
                return bridge_error(StatusCode::BAD_GATEWAY, "compatible LLM response failed")
            }
        };
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return bridge_error(StatusCode::BAD_GATEWAY, "compatible LLM response too large");
        }
        body.extend_from_slice(&chunk);
    }
    let body = rewrite_response(&Bytes::from(body), &namespaces);

    let mut response = Response::builder().status(status);
    for (name, value) in &headers {
        if !is_response_hop_header(name) {
            response = response.header(name, value);
        }
    }
    response
        .body(Body::from(body))
        .unwrap_or_else(|_| bridge_error(StatusCode::BAD_GATEWAY, "invalid upstream response"))
}

fn bridge_error(status: StatusCode, message: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({"error": {"message": message}}).to_string(),
        ))
        .expect("static bridge error response")
}

fn target_url(base: &Url, incoming: &Uri) -> Url {
    let mut target = base.clone();
    let suffix = incoming
        .path()
        .strip_prefix("/v1")
        .unwrap_or(incoming.path());
    let base_path = target.path().trim_end_matches('/');
    target.set_path(&format!("{base_path}{suffix}"));
    target.set_query(incoming.query());
    target
}

fn is_request_hop_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "host" | "content-length" | "accept-encoding" | "connection" | "transfer-encoding"
    )
}

fn is_response_hop_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "content-length" | "content-encoding" | "connection" | "transfer-encoding"
    )
}

fn rewrite_request(body: &[u8]) -> Result<(Vec<u8>, NamespaceMap), serde_json::Error> {
    let mut request: Value = serde_json::from_slice(body)?;
    let mut namespaces = NamespaceMap::new();
    if let Some(tools) = request.get_mut("tools").and_then(Value::as_array_mut) {
        let mut rewritten = Vec::with_capacity(tools.len());
        for tool in std::mem::take(tools) {
            if tool.get("type").and_then(Value::as_str) != Some("namespace") {
                rewritten.push(tool);
                continue;
            }
            let namespace = tool.get("name").and_then(Value::as_str).unwrap_or_default();
            for child in tool
                .get("tools")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if child.get("type").and_then(Value::as_str) != Some("function") {
                    continue;
                }
                let child_name = child
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let flat_name = format!("{namespace}__{child_name}");
                let mut child = child.clone();
                child["name"] = Value::String(flat_name.clone());
                namespaces.insert(flat_name, (namespace.to_string(), child_name.to_string()));
                rewritten.push(child);
            }
        }
        *tools = rewritten;
    }

    if let Some(input) = request.get_mut("input").and_then(Value::as_array_mut) {
        for item in input {
            if item.get("type").and_then(Value::as_str) != Some("function_call") {
                continue;
            }
            let Some(namespace) = item
                .get("namespace")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                continue;
            };
            let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
            item["name"] = Value::String(format!("{namespace}__{name}"));
            item.as_object_mut()
                .expect("function call is an object")
                .remove("namespace");
        }
    }

    Ok((serde_json::to_vec(&request)?, namespaces))
}

fn rewrite_response(body: &Bytes, namespaces: &NamespaceMap) -> Vec<u8> {
    if namespaces.is_empty() {
        return body.to_vec();
    }
    let Ok(text) = std::str::from_utf8(body) else {
        return body.to_vec();
    };
    if text.lines().any(|line| line.starts_with("data:")) {
        return text
            .split('\n')
            .map(|line| rewrite_sse_line(line, namespaces))
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes();
    }
    let Ok(mut value) = serde_json::from_str::<Value>(text) else {
        return body.to_vec();
    };
    restore_namespaces(&mut value, namespaces);
    serde_json::to_vec(&value).unwrap_or_else(|_| body.to_vec())
}

fn rewrite_sse_line(line: &str, namespaces: &NamespaceMap) -> String {
    let Some(data) = line.strip_prefix("data:") else {
        return line.to_string();
    };
    let data = data.trim_start();
    if data == "[DONE]" {
        return line.to_string();
    }
    let Ok(mut value) = serde_json::from_str::<Value>(data) else {
        return line.to_string();
    };
    restore_namespaces(&mut value, namespaces);
    format!(
        "data: {}",
        serde_json::to_string(&value).unwrap_or_else(|_| data.to_string())
    )
}

fn restore_namespaces(value: &mut Value, namespaces: &NamespaceMap) {
    match value {
        Value::Object(object) => {
            let mapping = if object.get("type").and_then(Value::as_str) == Some("function_call") {
                object
                    .get("name")
                    .and_then(Value::as_str)
                    .and_then(|name| namespaces.get(name))
                    .cloned()
            } else {
                None
            };
            if let Some((namespace, name)) = mapping {
                object.insert("name".to_string(), Value::String(name));
                object.insert("namespace".to_string(), Value::String(namespace));
            }
            for child in object.values_mut() {
                restore_namespaces(child, namespaces);
            }
        }
        Value::Array(items) => {
            for item in items {
                restore_namespaces(item, namespaces);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_flattens_namespace_tools_and_history() {
        let body = serde_json::to_vec(&json!({
            "tools": [
                {"type":"namespace","name":"mcp__knowledge","description":"Knowledge","tools":[
                    {"type":"function","name":"wiki_list","description":"List","strict":false,"parameters":{"type":"object"}}
                ]},
                {"type":"function","name":"update_plan","description":"Plan","parameters":{"type":"object"}},
                {"type":"web_search"}
            ],
            "input": [
                {"type":"function_call","namespace":"mcp__knowledge","name":"wiki_list","arguments":"{}","call_id":"call-1"},
                {"type":"function_call_output","call_id":"call-1","output":"ok"}
            ]
        }))
        .unwrap();

        let (rewritten, namespaces) = rewrite_request(&body).unwrap();
        let rewritten: Value = serde_json::from_slice(&rewritten).unwrap();
        assert_eq!(rewritten["tools"][0]["name"], "mcp__knowledge__wiki_list");
        assert_eq!(rewritten["tools"][1]["name"], "update_plan");
        assert_eq!(rewritten["tools"][2]["type"], "web_search");
        assert_eq!(rewritten["input"][0]["name"], "mcp__knowledge__wiki_list");
        assert!(rewritten["input"][0].get("namespace").is_none());
        assert_eq!(
            namespaces.get("mcp__knowledge__wiki_list"),
            Some(&("mcp__knowledge".to_string(), "wiki_list".to_string()))
        );
    }

    #[test]
    fn response_restores_namespace_in_stream_items_and_completed_output() {
        let namespaces = HashMap::from([(
            "mcp__knowledge__wiki_list".to_string(),
            ("mcp__knowledge".to_string(), "wiki_list".to_string()),
        )]);
        let body = Bytes::from(
            "event: response.output_item.done\n\
data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"name\":\"mcp__knowledge__wiki_list\",\"arguments\":\"{}\",\"call_id\":\"call-1\"}}\n\n\
data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"function_call\",\"name\":\"mcp__knowledge__wiki_list\",\"arguments\":\"{}\",\"call_id\":\"call-1\"}]}}\n\n",
        );

        let rewritten = String::from_utf8(rewrite_response(&body, &namespaces)).unwrap();
        assert!(rewritten.contains("\"namespace\":\"mcp__knowledge\""));
        assert!(rewritten.contains("\"name\":\"wiki_list\""));
        assert!(!rewritten.contains("\"name\":\"mcp__knowledge__wiki_list\""));
    }

    #[test]
    fn target_url_preserves_base_path_and_query() {
        let base = Url::parse("http://127.0.0.1:11434/v1/").unwrap();
        let uri: Uri = "/v1/models?client_version=1".parse().unwrap();
        assert_eq!(
            target_url(&base, &uri).as_str(),
            "http://127.0.0.1:11434/v1/models?client_version=1"
        );
    }
}
