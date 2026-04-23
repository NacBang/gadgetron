//! LLM fallback for log lines that look error-ish but didn't match
//! any rule. We call our OWN gateway (`POST /v1/chat/completions`)
//! with a tightly-constrained prompt asking for `{severity, category,
//! summary}` JSON. Result quality varies; we cap retries at 1 and
//! treat any parse failure as "skip — surface the line as-is with
//! `severity=info`".
//!
//! Rate limits: per scan tick we send at most `MAX_LLM_PER_TICK`
//! lines so a noisy host doesn't blow the token budget. Lines beyond
//! the cap fall through with `severity=info, category=unknown`.

use crate::model::{Classification, Severity};
use serde::Deserialize;
use std::time::Duration;

pub const MAX_LLM_PER_TICK: usize = 5;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Minimal interface so the scanner can swap implementations in tests.
#[async_trait::async_trait]
pub trait Classifier: Send + Sync {
    async fn classify(&self, line: &str) -> Option<Classification>;
}

/// Production classifier: POSTs to our own gateway. Requires a
/// gadgetron API key with `OpenAiCompat` scope for the chat call.
pub struct GatewayClassifier {
    pub gateway_url: String, // e.g. "http://127.0.0.1:18080"
    pub api_key: String,
    pub model: String, // "penny" by default
    pub client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct LlmJson {
    severity: String,
    category: String,
    summary: String,
    #[serde(default)]
    cause: Option<String>,
    #[serde(default)]
    solution: Option<String>,
    /// Optional structured remediation. UI checks `tool` against an
    /// allowlist (`server.systemctl`, `server.apt`) before exposing
    /// the "승인 실행" button. `args` must include `verb` (and `unit`
    /// for systemctl, `packages` for apt).
    #[serde(default)]
    remediation: Option<serde_json::Value>,
}

#[async_trait::async_trait]
impl Classifier for GatewayClassifier {
    async fn classify(&self, line: &str) -> Option<Classification> {
        let prompt = format!(
            "Classify this single log line. Reply with JSON ONLY (no \
             code fences, no prose). Schema:\n\
             {{\n\
               \"severity\": \"critical|high|medium|info\",\n\
               \"category\": \"<short_snake_case>\",\n\
               \"summary\": \"<<= 80 chars one-line label>\",\n\
               \"cause\":   \"<<= 300 chars on WHY this happened>\",\n\
               \"solution\":\"<<= 300 chars step-by-step fix>\",\n\
               \"remediation\": null OR {{\n\
                 \"tool\": \"server.systemctl\" | \"server.apt\",\n\
                 \"args\": {{ … }},\n\
                 \"label\": \"<button text, KR ok>\"\n\
               }}\n\
             }}\n\
             Severity rule: critical=HW fault / data loss / service \
             totally down; high=service crash / repeated; medium=warning; \
             info=notable but ok.\n\
             remediation: ONLY when a server.systemctl restart/start/\
             enable on a SPECIFIC unit OR a server.apt install of \
             SPECIFIC packages would resolve it. For server.systemctl \
             use args={{verb:'restart'|'start'|'enable',unit:'<name>'}}. \
             For server.apt use args={{verb:'install'|'update', \
             packages:['<pkg>',…]}}. If no safe automated fix exists, \
             set remediation=null.\n\nLog line:\n{line}"
        );
        let body = serde_json::json!({
            "model": self.model,
            "stream": false,
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0.0,
            "max_tokens": 200,
        });
        let url = format!("{}/v1/chat/completions", self.gateway_url);
        let resp = match tokio::time::timeout(
            REQUEST_TIMEOUT,
            self.client
                .post(&url)
                .bearer_auth(&self.api_key)
                .json(&body)
                .send(),
        )
        .await
        {
            Ok(Ok(r)) => r,
            _ => return None,
        };
        if !resp.status().is_success() {
            return None;
        }
        let v: serde_json::Value = resp.json().await.ok()?;
        let text = v
            .get("choices")?
            .get(0)?
            .get("message")?
            .get("content")?
            .as_str()?
            .to_string();
        // Strip code fences / surrounding prose — pick the first
        // {...} block.
        let json_str = extract_json_object(&text)?;
        let parsed: LlmJson = serde_json::from_str(&json_str).ok()?;
        Some(Classification {
            severity: Severity::parse(&parsed.severity).unwrap_or(Severity::Info),
            category: sanitize_category(&parsed.category),
            summary: truncate(&parsed.summary, 200),
            cause: parsed.cause.map(|s| truncate(&s, 600)),
            solution: parsed.solution.map(|s| truncate(&s, 600)),
            remediation: validate_remediation(parsed.remediation),
        })
    }
}

/// Whitelist guard: drop the remediation entirely if `tool` isn't in
/// the allowed set or `args` is missing required fields. Stops Penny
/// from hallucinating a `bash` tool that we don't expose.
fn validate_remediation(v: Option<serde_json::Value>) -> Option<serde_json::Value> {
    let v = v?;
    let tool = v.get("tool").and_then(|t| t.as_str())?;
    if tool != "server.systemctl" && tool != "server.apt" {
        return None;
    }
    let args = v.get("args")?.as_object()?;
    let verb = args.get("verb").and_then(|x| x.as_str())?;
    match tool {
        "server.systemctl" => {
            if !matches!(
                verb,
                "start" | "stop" | "restart" | "reload" | "enable" | "disable" | "status"
            ) {
                return None;
            }
            args.get("unit").and_then(|x| x.as_str())?;
        }
        "server.apt" => {
            if !matches!(verb, "install" | "update" | "upgrade" | "autoremove") {
                return None;
            }
            // install/upgrade/remove require packages list (excluding update/autoremove).
            if matches!(verb, "install") {
                let pkgs = args.get("packages").and_then(|x| x.as_array())?;
                if pkgs.is_empty() {
                    return None;
                }
            }
        }
        _ => return None,
    }
    Some(v)
}

fn extract_json_object(s: &str) -> Option<String> {
    let start = s.find('{')?;
    let mut depth = 0usize;
    let mut end = None;
    for (i, ch) in s[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(start + i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    end.map(|e| s[start..e].to_string())
}

fn sanitize_category(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        .take(64)
        .collect()
}

fn truncate(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= max {
            break;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extracts_json_from_prose() {
        let s = "Here you go: {\"severity\":\"high\",\"category\":\"oom\",\"summary\":\"…\"} done.";
        let j = extract_json_object(s).unwrap();
        assert!(j.starts_with('{') && j.ends_with('}'));
    }
}
