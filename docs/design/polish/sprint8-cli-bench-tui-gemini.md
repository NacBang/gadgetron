# Sprint 8: CLI Completion, Criterion Benchmarks, TUI Scroll/Focus, Gemini Adapter

> **담당**: @chief-architect
> **상태**: ✅ Implemented (commit `4170891`, 2026-04-12) — CLI completion, criterion benches (auth_cache/middleware_chain/router), TUI nav, Gemini provider 474 LOC 전부 머지
> **작성일**: 2026-04-12
> **최종 업데이트**: 2026-04-12
> **관련 크레이트**: `gadgetron-cli`, `gadgetron-gateway`, `gadgetron-tui`, `gadgetron-provider`, `gadgetron-xaas`
> **Phase**: [P1]

---

## 1. 철학 & 컨셉 (Why)

### 1.1 이 스프린트가 해결하는 문제

Sprint 8은 Phase 1 MVP의 네 가지 관찰된 공백을 닫는다.

1. **CLI 완성**: `gadgetron tenant list`와 `gadgetron key list/revoke`가 `bail!("not yet implemented")`로 막혀 있어 운영자가 CLI만으로 멀티테넌트 설정을 완결할 수 없다. 이 세 명령을 구현하면 D-20260411-01에서 확정된 "기본 API 키 + 기본 쿼터 (경량) XaaS" CLI 루프가 완성된다.

2. **Criterion 벤치마크**: 설계 문서(gateway, router, xaas)에서 P99 < 1ms/sub-ms 목표를 선언했으나 어떤 bench harness도 없다. D-20260411-05에서 각 크레이트 `benches/`에 Criterion을 두기로 확정했으며 (`gadgetron-testing` dev-dep 재사용), 이번 스프린트에서 gateway 3종을 추가한다.

3. **TUI 방송 채널 검증 + 스크롤/포커스**: `App::with_channel`은 구현됐으나 `serve()` 내부에서 실제로 `tx.subscribe()`를 호출해 `with_channel`에 넘기는 경로가 코드상으로만 확인되고 통합 테스트가 없다. 또한 패널 내부 스크롤과 Tab 포커스 전환이 없어 긴 목록에서 내용 확인이 불가하다.

4. **Gemini 어댑터**: D-20260411-01에서 Gemini를 Phase 1 6종 provider에 포함 확정. D-20260411-02에서 "Week 5-7 공통 normalizer 추출 후 Gemini polling adapter"로 승인. OpenAI adapter 패턴을 기반으로 Gemini 고유 API 구조(query param 인증, `candidates[].content.parts[].text` 응답, `?alt=sse` SSE 스트리밍)를 처리한다.

### 1.2 제품 비전 연결

`docs/00-overview.md §1`에서 "sub-millisecond P99 overhead"와 "Rust-native GPU/LLM orchestration"을 핵심 목표로 명시한다. 이 스프린트는 그 목표를 측정 가능하게 만드는 벤치마크 인프라와, 운영자 경험을 닫는 CLI, 그리고 6종 provider 완성을 통해 Phase 1 범위를 충족한다.

### 1.3 고려한 대안과 채택하지 않은 이유

| 항목 | 대안 | 채택하지 않은 이유 |
|------|------|------|
| CLI 출력 형식 | tui-table / prettytable 크레이트 추가 | 의존성 추가 없이 `format!` 기반 고정폭 열로 충분. D-20260411-05의 crate bloat 최소화 원칙 |
| 벤치마크 위치 | `gadgetron-testing/benches/` 중앙화 | Criterion은 각 크레이트 `benches/`를 cargo가 직접 인식. cross-crate harness는 통합 테스트용 |
| TUI 포커스 상태 | ratatui의 `StatefulWidget` + `ListState` | `ListState`는 ratatui 0.29에서 `selected` 반환이 `Option<usize>` — 그대로 채택 |
| Gemini 스트리밍 구현 | `eventsource-stream` 크레이트 (OpenAI와 동일) | Gemini SSE는 `?alt=sse` 쿼리 파라미터로 활성화되고 응답 구조가 다름. `bytes_stream()` + 수동 파싱으로 OpenAI 패턴과 구조 동일하게 유지 |
| Gemini 인증 | Bearer header에 key 포함 | Gemini API 사양은 `?key=` query param 전용. Bearer를 보내도 401 반환 |

### 1.4 핵심 설계 원칙

- **결정론적 구현**: 모든 SQL 쿼리, 출력 포맷, 타입 시그니처가 이 문서에 완전히 명시된다. 두 번째 구현자가 같은 코드를 작성할 수 있어야 한다.
- **기존 패턴 준수**: CLI는 `tenant_create` / `key_create` 기존 패턴을 그대로 따른다. Gemini는 `OpenAiProvider` 패턴을 따른다.
- **D-12 경계 준수**: `gadgetron-xaas`가 DB를, `gadgetron-provider`가 LLM을, `gadgetron-cli`가 진입점을 담당. 크레이트 간 타입 이동 없음.

---

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API

#### 2.1.1 Item 1: CLI 완성 — `gadgetron-cli/src/main.rs`

세 함수를 추가한다. 기존 `bail!` 분기를 교체한다.

```rust
// ── tenant_list ──────────────────────────────────────────────────────────────

/// Print all tenants as a fixed-width table.
///
/// Query:
///   SELECT id, name, status, created_at FROM tenants ORDER BY created_at DESC
///
/// Output format (stdout):
/// ```text
/// ID                                   Name             Status    Created
/// ──────────────────────────────────── ──────────────── ──────── ──────────────────────
/// 550e8400-e29b-41d4-a716-446655440000 acme             active   2026-04-11 09:00:00 UTC
/// ```
async fn tenant_list(pool: &sqlx::PgPool) -> Result<()>;

// ── key_list ─────────────────────────────────────────────────────────────────

/// Print all non-revoked API keys for a tenant as a fixed-width table.
///
/// Query:
///   SELECT id, prefix, kind, scopes, created_at
///   FROM api_keys
///   WHERE tenant_id = $1 AND revoked_at IS NULL
///   ORDER BY created_at DESC
///
/// Output format (stdout):
/// ```text
/// ID                                   Prefix    Kind  Scopes         Created
/// ──────────────────────────────────── ──────── ───── ────────────── ──────────────────────
/// 7c9e6679-7425-40de-944b-e07fc1f90ae7 gad_live live  OpenAiCompat  2026-04-11 09:01:00 UTC
/// ```
async fn key_list(pool: &sqlx::PgPool, tenant_id: Uuid) -> Result<()>;

// ── key_revoke ────────────────────────────────────────────────────────────────

/// Revoke an API key by UUID and invalidate its cache entry if a validator
/// is available.
///
/// SQL:
///   UPDATE api_keys SET revoked_at = NOW() WHERE id = $1 RETURNING key_hash
///
/// Output (stdout):
///   Key revoked: <uuid>
///
/// If zero rows are affected, returns error:
///   "Key not found or already revoked: <uuid>"
async fn key_revoke(
    pool: &sqlx::PgPool,
    key_id: Uuid,
    validator: Option<&(dyn gadgetron_xaas::auth::validator::KeyValidator + Send + Sync)>,
) -> Result<()>;
```

`main()` 내부 교체 지점:

```rust
// Before (lines 194-198):
Some(Commands::Tenant { command: TenantCmd::List }) => {
    anyhow::bail!("gadgetron tenant list is not yet implemented. ...")
}

// After:
Some(Commands::Tenant { command: TenantCmd::List }) => {
    let pool = connect_pg().await?;
    tenant_list(&pool).await
}

// Before (lines 217-223):
Some(Commands::Key { command: KeyCmd::List { tenant_id } }) => {
    anyhow::bail!("gadgetron key list is not yet implemented. ...")
}

// After:
Some(Commands::Key { command: KeyCmd::List { tenant_id } }) => {
    let pool = connect_pg().await?;
    key_list(&pool, tenant_id).await
}

// Before (lines 225-233):
Some(Commands::Key { command: KeyCmd::Revoke { key_id } }) => {
    anyhow::bail!("gadgetron key revoke is not yet implemented. ...")
}

// After:
Some(Commands::Key { command: KeyCmd::Revoke { key_id } }) => {
    let pool = connect_pg().await?;
    key_revoke(&pool, key_id, None).await
}
```

#### 2.1.2 Item 2: Criterion 벤치마크 — `gadgetron-gateway/benches/`

세 벤치마크 파일은 별도 `[[bench]]` 섹션으로 `gadgetron-gateway/Cargo.toml`에 등록된다.

```toml
# gadgetron-gateway/Cargo.toml에 추가

[dev-dependencies]
criterion = { workspace = true }
gadgetron-testing = { workspace = true }
tokio = { workspace = true }

[[bench]]
name = "middleware_chain"
harness = false

[[bench]]
name = "auth_cache"
harness = false

[[bench]]
name = "router"
harness = false
```

각 벤치의 공개 진입점 시그니처:

```rust
// benches/middleware_chain.rs
use criterion::{criterion_group, criterion_main, Criterion};

fn bench_middleware_chain(c: &mut Criterion);
criterion_group!(benches, bench_middleware_chain);
criterion_main!(benches);

// benches/auth_cache.rs
fn bench_auth_cache_hit(c: &mut Criterion);
criterion_group!(benches, bench_auth_cache_hit);
criterion_main!(benches);

// benches/router.rs
fn bench_router_resolve_roundrobin(c: &mut Criterion);
criterion_group!(benches, bench_router_resolve_roundrobin);
criterion_main!(benches);
```

#### 2.1.3 Item 3: TUI 스크롤 + 포커스 — `gadgetron-tui/src/app.rs`, `gadgetron-tui/src/ui.rs`

`App` 구조체에 포커스 및 스크롤 상태를 추가한다:

```rust
/// Which of the three body panels currently has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusedPanel {
    Nodes,    // index 0
    Models,   // index 1
    Requests, // index 2
}

impl FocusedPanel {
    /// Rotate focus to the next panel (wraps from Requests → Nodes).
    pub fn next(self) -> Self {
        match self {
            Self::Nodes => Self::Models,
            Self::Models => Self::Requests,
            Self::Requests => Self::Nodes,
        }
    }
}

/// Scroll offsets for each panel.  `usize` rows from the top of the list.
#[derive(Debug, Default, Clone, Copy)]
pub struct ScrollState {
    pub nodes: usize,
    pub models: usize,
    pub requests: usize,
}

// App struct: add two fields (no breaking change to existing fields)
pub struct App {
    pub running: bool,
    pub health: Arc<RwLock<ClusterHealth>>,
    pub gpu_metrics: Arc<RwLock<Vec<GpuMetrics>>>,
    pub model_statuses: Arc<RwLock<Vec<ModelStatus>>>,
    pub request_log: Arc<RwLock<VecDeque<RequestEntry>>>,
    update_rx: Option<broadcast::Receiver<WsMessage>>,
    /// Currently focused panel.
    pub focused: FocusedPanel,
    /// Scroll offsets for each panel.
    pub scroll: ScrollState,
}
```

`App::new()` 초기화 추가:

```rust
// in Self { ... } block:
focused: FocusedPanel::Nodes,
scroll: ScrollState::default(),
```

`App::with_channel()` 변경 없음 — `new()` 위임이므로 자동 반영.

`App::handle_key()` 신규 메서드:

```rust
/// Process a single crossterm `KeyEvent` and update state.
///
/// Bindings:
///   Tab           → focused = focused.next()
///   KeyCode::Up   → scroll[focused] = scroll[focused].saturating_sub(1)
///   KeyCode::Down → scroll[focused] = scroll[focused].saturating_add(1)
///   'q' / Esc     → running = false
pub fn handle_key(&mut self, code: crossterm::event::KeyCode) {
    use crossterm::event::KeyCode;
    match code {
        KeyCode::Tab => {
            self.focused = self.focused.next();
        }
        KeyCode::Up => match self.focused {
            FocusedPanel::Nodes => {
                self.scroll.nodes = self.scroll.nodes.saturating_sub(1);
            }
            FocusedPanel::Models => {
                self.scroll.models = self.scroll.models.saturating_sub(1);
            }
            FocusedPanel::Requests => {
                self.scroll.requests = self.scroll.requests.saturating_sub(1);
            }
        },
        KeyCode::Down => match self.focused {
            FocusedPanel::Nodes => self.scroll.nodes = self.scroll.nodes.saturating_add(1),
            FocusedPanel::Models => self.scroll.models = self.scroll.models.saturating_add(1),
            FocusedPanel::Requests => {
                self.scroll.requests = self.scroll.requests.saturating_add(1)
            }
        },
        KeyCode::Char('q') | KeyCode::Esc => {
            self.running = false;
        }
        _ => {}
    }
}
```

`App::run()` 이벤트 루프 교체:

```rust
// Before (lines 258-265):
if event::poll(Duration::from_millis(100))? {
    if let Event::Key(key) = event::read()? {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.running = false,
            _ => {}
        }
    }
}

// After:
if event::poll(Duration::from_millis(100))? {
    if let Event::Key(key) = event::read()? {
        self.handle_key(key.code);
    }
}
```

`ui::draw_*_panel()` 변경 — 포커스 강조 + 스크롤 적용:

```rust
// draw_nodes_panel: 시그니처 변경
fn draw_nodes_panel(f: &mut Frame, area: Rect, app: &App);

// draw_models_panel: 시그니처 변경
fn draw_models_panel(f: &mut Frame, area: Rect, app: &App);

// draw_requests_panel: 시그니처 변경
fn draw_requests_panel(f: &mut Frame, area: Rect, app: &App);
```

포커스된 패널의 border 색은 `Color::White`(기존 `Color::Green`/`Color::Yellow`/`Color::Blue` 유지, 포커스 시에만 `Color::White`로 override). footer 업데이트:

```rust
// Before: " q/Esc: quit "
// After:  " Tab: next panel  ↑↓: scroll  q/Esc: quit "
```

스크롤 적용 방식: `items.iter().skip(scroll_offset).take(visible_rows)`. `visible_rows`는 `area.height.saturating_sub(2)` (border 2줄 제외).

#### 2.1.4 Item 4: Gemini 어댑터 — `gadgetron-provider/src/gemini.rs`

```rust
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use gadgetron_core::error::{GadgetronError, Result};
use gadgetron_core::provider::{
    ChatChunk, ChatRequest, ChatResponse, LlmProvider, ModelInfo,
};

/// Gemini API adapter.
///
/// Authentication: `?key=<api_key>` query parameter (not Bearer).
/// Base URL: `https://generativelanguage.googleapis.com/v1`
/// Non-streaming: POST `/models/{model}:generateContent?key=<api_key>`
/// Streaming:     POST `/models/{model}:streamGenerateContent?alt=sse&key=<api_key>`
pub struct GeminiProvider {
    client: Client,
    api_key: String,
    base_url: String,
    models: Vec<String>,
}

impl GeminiProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            base_url: "https://generativelanguage.googleapis.com/v1".to_string(),
            models: Vec::new(),
        }
    }

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }

    pub fn with_models(mut self, models: Vec<String>) -> Self {
        self.models = models;
        self
    }

    /// Build the non-streaming endpoint URL for a model.
    /// Format: `{base_url}/models/{model}:generateContent?key={api_key}`
    fn generate_url(&self, model: &str) -> String {
        format!(
            "{}/models/{}:generateContent?key={}",
            self.base_url, model, self.api_key
        )
    }

    /// Build the streaming endpoint URL for a model.
    /// Format: `{base_url}/models/{model}:streamGenerateContent?alt=sse&key={api_key}`
    fn stream_url(&self, model: &str) -> String {
        format!(
            "{}/models/{}:streamGenerateContent?alt=sse&key={}",
            self.base_url, model, self.api_key
        )
    }

    /// Convert a `ChatRequest` (OpenAI format) to Gemini `GenerateContentRequest`.
    fn to_gemini_request(req: &ChatRequest) -> GeminiRequest {
        let contents: Vec<GeminiContent> = req
            .messages
            .iter()
            .map(|m| GeminiContent {
                role: match m.role.as_str() {
                    "assistant" => "model".to_string(),
                    other => other.to_string(),
                },
                parts: vec![GeminiPart { text: m.content.clone() }],
            })
            .collect();
        GeminiRequest { contents }
    }

    /// Extract the text from a non-streaming `GeminiResponse`.
    ///
    /// Returns `GadgetronError::Provider` if the response has no candidates or parts.
    fn extract_text(resp: &GeminiResponse) -> Result<String> {
        resp.candidates
            .first()
            .and_then(|c| c.content.parts.first())
            .map(|p| p.text.clone())
            .ok_or_else(|| {
                GadgetronError::Provider(
                    "Gemini response contained no candidates or parts".to_string(),
                )
            })
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse> {
        let url = self.generate_url(&req.model);
        let gemini_req = Self::to_gemini_request(&req);

        let resp = self
            .client
            .post(&url)
            .json(&gemini_req)
            .send()
            .await
            .map_err(|e| GadgetronError::Provider(format!("Gemini request failed: {}", e)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(GadgetronError::Provider(format!(
                "Gemini error {}: {}",
                status, body
            )));
        }

        let gemini_resp: GeminiResponse = resp
            .json()
            .await
            .map_err(|e| GadgetronError::Provider(format!("Gemini parse error: {}", e)))?;

        let text = Self::extract_text(&gemini_resp)?;

        Ok(ChatResponse {
            id: format!("gemini-{}", uuid::Uuid::new_v4()),
            object: "chat.completion".to_string(),
            model: req.model.clone(),
            choices: vec![gadgetron_core::provider::Choice {
                index: 0,
                message: gadgetron_core::provider::Message {
                    role: Role::Assistant,
                    content: Content::Text(text),
                    reasoning_content: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
            },
        })
    }

    fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> std::pin::Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>> {
        let client = self.client.clone();
        let url = self.stream_url(&req.model);
        let model = req.model.clone();
        let gemini_req = Self::to_gemini_request(&req);

        Box::pin(async_stream::stream! {
            let resp = match client
                .post(&url)
                .json(&gemini_req)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    yield Err(GadgetronError::Provider(
                        format!("Gemini stream request failed: {}", e)
                    ));
                    return;
                }
            };

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                yield Err(GadgetronError::Provider(
                    format!("Gemini stream error {}: {}", status, body)
                ));
                return;
            }

            let mut stream = resp.bytes_stream();
            let mut buffer = String::new();

            while let Some(chunk) = stream.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        yield Err(GadgetronError::Provider(
                            format!("Gemini stream read error: {}", e)
                        ));
                        return;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&bytes));

                // SSE frames are delimited by "\n\n".
                // Each frame matching "data: {...}" contains a GeminiStreamChunk.
                while let Some(pos) = buffer.find("\n\n") {
                    let frame = buffer[..pos].to_string();
                    buffer = buffer[pos + 2..].to_string();

                    let frame = frame.trim();
                    if frame.is_empty() || frame == "data: [DONE]" {
                        continue;
                    }

                    if let Some(data) = frame.strip_prefix("data: ") {
                        match serde_json::from_str::<GeminiStreamChunk>(data) {
                            Ok(sc) => {
                                let text = sc
                                    .candidates
                                    .first()
                                    .and_then(|c| c.content.parts.first())
                                    .map(|p| p.text.clone())
                                    .unwrap_or_default();
                                yield Ok(ChatChunk {
                                    id: format!("gemini-chunk-{}", uuid::Uuid::new_v4()),
                                    object: "chat.completion.chunk".to_string(),
                                    model: model.clone(),
                                    choices: vec![gadgetron_core::provider::ChunkChoice {
                                        index: 0,
                                        delta: gadgetron_core::provider::ChunkDelta {
                                            role: None,
                                            content: Some(text),
                                        },
                                        finish_reason: sc
                                            .candidates
                                            .first()
                                            .and_then(|c| c.finish_reason.clone()),
                                    }],
                                });
                            }
                            Err(e) => {
                                yield Err(GadgetronError::Provider(
                                    format!("Gemini chunk parse error: {}", e)
                                ));
                                return;
                            }
                        }
                    }
                }
            }
        })
    }

    async fn models(&self) -> Result<Vec<ModelInfo>> {
        // Use configured model list if provided; do not call Gemini's models endpoint
        // (it requires project-scoped auth beyond a simple API key).
        Ok(self
            .models
            .iter()
            .map(|m| ModelInfo {
                id: m.clone(),
                object: "model".to_string(),
                owned_by: "google".to_string(),
            })
            .collect())
    }

    fn name(&self) -> &str {
        "gemini"
    }

    async fn health(&self) -> Result<()> {
        // A minimal text-only request to verify the API key is accepted.
        let url = self.generate_url("gemini-1.5-flash");
        let probe = GeminiRequest {
            contents: vec![GeminiContent {
                role: "user".to_string(),
                parts: vec![GeminiPart { text: "ping".to_string() }],
            }],
        };
        let resp = self
            .client
            .post(&url)
            .json(&probe)
            .send()
            .await
            .map_err(|e| GadgetronError::Provider(format!("gemini: {}", e)))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(GadgetronError::Provider(format!(
                "gemini: Status {}",
                resp.status()
            )))
        }
    }
}
```

### 2.2 내부 구조

#### 2.2.1 CLI 내부 구조

**`tenant_list` 내부 SQL 및 출력**:

```sql
SELECT id, name, status, created_at
FROM tenants
ORDER BY created_at DESC
```

`sqlx::query()` + `Row::get::<Uuid, _>("id")`, `get::<String, _>("name")`, `get::<String, _>("status")`, `get::<chrono::DateTime<chrono::Utc>, _>("created_at")` 사용.

**Note**: `status`는 DB의 CHECK constraint (`'Active'`, `'Suspended'`, `'Deleted'`)와 일치하는 title-case 문자열로 저장된다. 출력 시 변환 없이 그대로 표시한다 (예: `Active`, `Suspended`, `Deleted`).

출력 헤더/구분선/행을 `println!` 호출 3세트로 구성한다:

```
ID                                   Name             Status    Created
──────────────────────────────────── ──────────────── ──────── ──────────────────────
{id:<36} {name:<16} {status:<8} {created_at}
```

행이 0개이면 `"No tenants found. Run: gadgetron tenant create --name <name>"` 메시지 출력 후 `Ok(())`.

**`key_list` 내부 SQL**:

```sql
SELECT id, prefix, kind, scopes, created_at
FROM api_keys
WHERE tenant_id = $1 AND revoked_at IS NULL
ORDER BY created_at DESC
```

`scopes`는 `Vec<String>`으로 수신하여 쉼표로 join.

**`key_revoke` 내부**:

```sql
UPDATE api_keys
SET revoked_at = NOW()
WHERE id = $1
RETURNING key_hash
```

`fetch_optional()`로 반환행 존재 여부를 확인한다. 반환행이 `None`이면 `"Key not found or already revoked: {key_id}"` 에러. `key_hash`가 반환되면 `validator` 인자가 `Some`일 경우 `validator.invalidate(&key_hash).await`를 호출한다. `main()`에서는 revoke 전용 pool만 사용하므로 `validator`는 `None`으로 전달한다.

#### 2.2.2 벤치마크 내부 구조

**`benches/middleware_chain.rs`**:

- `tokio::runtime::Builder::new_current_thread().enable_all().build()` 로 단일 스레드 런타임 생성.
- `FakeLlmProvider` (from `gadgetron-testing::mocks::provider`) + `gadgetron-gateway::server::AppState` 빌드.
- `tower::ServiceBuilder`로 전체 미들웨어 스택 조립: `request_id_middleware` → `tenant_context_middleware` → `auth_middleware` → `scope_guard_middleware` → `metrics_middleware` → handler.
- `VALID_TOKEN` (from `gadgetron-gateway::test_helpers`) 포함 `http::Request<Body>` 직접 생성.
- `criterion::BenchmarkId::new("middleware_chain", "full_stack")`.
- 목표 P99 < 1000µs는 bench 출력의 `time` 컬럼으로 수동 검증.

**`benches/auth_cache.rs`**:

- `PgKeyValidator::new(lazy_pool())` 생성 후 `cache.insert(key_hash, validated)` 로 캐시 워밍.
- `c.bench_function("auth_cache_hit", |b| b.to_async(&rt).iter(|| async { validator.validate(&key_hash).await }))`.
- `lazy_pool()`은 `connect_lazy`이므로 실제 DB 연결 없음 — 캐시 경로만 측정.
- 목표 P99 < 50µs.

**`benches/router.rs`**:

- 동기 컨텍스트에서 실행 가능 (`resolve()`가 `async` 아님).
- `FakeLlmProvider` 2개 + `RoutingConfig { default_strategy: RoutingStrategy::RoundRobin, .. }`.
- `c.bench_function("router_resolve_roundrobin", |b| b.iter(|| router.resolve(&req)))`.
- 목표 P99 < 20µs.

#### 2.2.3 TUI 상태머신

포커스 전환은 Tab 단일 키 → `FocusedPanel::next()` 호출. 스크롤은 클램핑 없이 `saturating_add/sub`로 처리한다. 렌더 시 `skip(offset).take(visible)` 적용이 자동 클램핑으로 작동한다 (리스트가 짧으면 빈 항목이 나타나지 않음).

**시각 강조**: `draw_*_panel`에서 `FocusedPanel`과 매칭하여 해당 패널의 `Block::border_style`을 `Style::default().fg(Color::White).add_modifier(Modifier::BOLD)`로 설정. 비포커스 패널은 기존 색상 유지.

#### 2.2.4 Gemini API 내부 타입 (serde)

```rust
// 요청 타입
#[derive(Serialize)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
}

#[derive(Serialize)]
struct GeminiContent {
    role: String,
    parts: Vec<GeminiPart>,
}

#[derive(Serialize, Deserialize)]
struct GeminiPart {
    text: String,
}

// 비스트리밍 응답
#[derive(Deserialize)]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: GeminiContent,
    #[serde(rename = "finishReason")]
    finish_reason: Option<String>,
}

// 스트리밍 SSE 청크 (비스트리밍 응답과 동일 구조)
#[derive(Deserialize)]
struct GeminiStreamChunk {
    candidates: Vec<GeminiCandidate>,
}

// GeminiContent Deserialize impl 추가 필요
// (Serialize는 이미 위에 있음)
#[derive(Deserialize)]
struct GeminiContentDeser {
    role: String,
    parts: Vec<GeminiPart>,
}
```

`GeminiContent`는 요청(`Serialize`) + 응답(`Deserialize`)에 모두 쓰이므로 `#[derive(Serialize, Deserialize)]` 양쪽에 붙인다.

완성된 derive 목록:

```rust
#[derive(Serialize)]
struct GeminiRequest { ... }

#[derive(Serialize, Deserialize)]
struct GeminiContent { ... }

#[derive(Serialize, Deserialize)]
struct GeminiPart { ... }

#[derive(Deserialize)]
struct GeminiResponse { ... }

#[derive(Deserialize)]
struct GeminiCandidate {
    content: GeminiContent,
    #[serde(rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct GeminiStreamChunk {
    candidates: Vec<GeminiCandidate>,
}
```

`gadgetron-provider/src/lib.rs`에 `pub mod gemini;` 추가.

### 2.3 설정 스키마

Gemini provider를 `gadgetron.toml`에서 사용할 때 형식:

```toml
[providers.gemini]
type = "gemini"
api_key = "${GEMINI_API_KEY}"
models = ["gemini-1.5-pro", "gemini-1.5-flash", "gemini-2.0-flash"]
```

`gadgetron-core/src/config.rs`의 `ProviderConfig` enum에 `Gemini` variant 추가가 필요하다. 현재 `ProviderConfig`를 확인해서 기존 패턴(Vllm, OpenAi 등)을 따른다:

```rust
// gadgetron-core/src/config.rs 추가
ProviderConfig::Gemini {
    api_key: Option<String>,
    models: Option<Vec<String>>,
}
```

**Note**: `build_providers()` is in `crates/gadgetron-cli/src/main.rs`. The Gemini match arm:

```rust
ProviderConfig::Gemini { api_key, models: _ } =>
    Arc::new(gadgetron_provider::gemini::GeminiProvider::new(
        "https://generativelanguage.googleapis.com/v1".to_string(),
        api_key.clone(),
    )),
```

`gadgetron-cli/src/main.rs`의 `build_providers()`에서 `ProviderConfig::Gemini` 매칭 추가:

```rust
ProviderConfig::Gemini { api_key, models } => {
    let key = api_key
        .as_deref()
        .or_else(|| std::env::var("GEMINI_API_KEY").ok().as_deref())
        // 위 line은 임시 문자열 참조 문제가 있으므로 아래처럼 작성:
        .unwrap_or_default()
        .to_string();
    let mut provider = gadgetron_provider::gemini::GeminiProvider::new(key);
    if let Some(m) = models {
        provider = provider.with_models(m.clone());
    }
    Arc::new(provider) as Arc<dyn LlmProvider + Send + Sync>
}
```

정확한 구현:

```rust
ProviderConfig::Gemini { api_key, models } => {
    let key = api_key
        .clone()
        .unwrap_or_else(|| std::env::var("GEMINI_API_KEY").unwrap_or_default());
    let mut provider = gadgetron_provider::gemini::GeminiProvider::new(key);
    if let Some(m) = models {
        provider = provider.with_models(m.clone());
    }
    Arc::new(provider) as Arc<dyn LlmProvider + Send + Sync>
}
```

CLI 벤치마크 없음. `gadgetron-cli` 바이너리 크레이트이므로 `benches/` 불필요.

TUI 설정 변경 없음. `FocusedPanel`과 `ScrollState`는 런타임 상태이며 TOML 설정 아님.

### 2.4 에러 & 로깅

**CLI 에러**:

- DB 연결 실패: 기존 `connect_pg()` 에러 UX 패턴 그대로 사용 (`anyhow::Context`).
- `tenant_list` 쿼리 실패: `with_context(|| "Failed to list tenants. Cause: ...")`.
- `key_list` 쿼리 실패: `with_context(|| "Failed to list keys for tenant {tenant_id}. Cause: ...")`.
- `key_revoke` — 0행 갱신: `anyhow::bail!("Key not found or already revoked: {key_id}")`.
- `key_revoke` — DB 실패: `with_context(|| "Failed to revoke key {key_id}. Cause: ...")`.

**tracing spans — CLI**: CLI 명령은 단기 실행 프로세스이므로 `tracing::info!` 로그를 `serve()` 실행과 동일하게 init하지 않는다. 출력은 `println!` / `eprintln!` 전용.

**Gemini 에러**: 모두 `GadgetronError::Provider(String)` variant 사용 (신규 variant 불필요). 로그:

```rust
// chat():
tracing::debug!(model = %req.model, "gemini chat request");
tracing::warn!(status = %status, "gemini non-2xx response");

// chat_stream():
tracing::debug!(model = %req.model, "gemini stream request");
tracing::warn!(error = %e, "gemini stream error");
```

**벤치마크**: `tracing` 비활성 — 벤치 결과 노이즈 방지.

**TUI 에러**: `handle_key()` 자체는 infallible. `run()` 루프의 `event::poll` 실패는 `?`로 `anyhow::Error`로 전파 (기존 패턴 유지).

### 2.5 의존성

| 크레이트 | 추가 의존성 | 버전 | 이유 |
|----------|-------------|------|------|
| `gadgetron-gateway` | `criterion` (dev) | workspace (0.5) | 벤치마크. 이미 workspace에 있음 |
| `gadgetron-gateway` | `gadgetron-testing` (dev) | workspace | `FakeLlmProvider` mock |
| `gadgetron-provider` | 없음 | — | `gemini.rs`는 기존 `reqwest`, `async-stream`, `serde_json`만 사용 |
| `gadgetron-core` | 없음 | — | `ProviderConfig::Gemini` 추가는 기존 enum extension |
| `gadgetron-tui` | 없음 | — | `FocusedPanel`/`ScrollState`는 새 로컬 타입 |
| `gadgetron-cli` | 없음 | — | CLI 구현은 기존 `sqlx`, `anyhow` 사용 |

`criterion = { version = "0.5", features = ["async_tokio"] }`는 이미 workspace `[workspace.dependencies]`에 있다 (`Cargo.toml` line 93).

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 데이터 흐름 다이어그램

```
CLI Commands
├── tenant list ──────────────────────────────────────────────┐
│                                                              │
├── key list --tenant-id <uuid> ──────────────────────────────┤
│                                                              ▼
└── key revoke --key-id <uuid> ──── gadgetron-xaas/auth/ ── PostgreSQL
                                    validator.rs:invalidate()
                                    (cache invalidation)

Gemini Provider
gadgetron-cli/build_providers()
    └── gadgetron-core/config.rs:ProviderConfig::Gemini
              └── gadgetron-provider/src/gemini.rs:GeminiProvider
                        ├── POST .../generateContent?key=...       → Gemini API
                        └── POST .../streamGenerateContent?alt=sse → Gemini API (SSE)

TUI Event Loop (gadgetron-tui/src/app.rs)
    ┌── KeyCode::Tab ──────── FocusedPanel::next()
    ├── KeyCode::Up/Down ──── ScrollState.nodes/models/requests
    └── KeyCode::q/Esc ────── running = false
              │
              ▼
    ui::draw_*_panel(f, area, app)
              ├── block.border_style = if focused { Color::White } else { original }
              └── items.iter().skip(scroll_offset).take(visible_rows)

Benchmarks (gadgetron-gateway/benches/)
    middleware_chain.rs ── AppState + tower::ServiceBuilder + FakeLlmProvider
    auth_cache.rs       ── PgKeyValidator::new(lazy_pool()) + cache.insert()
    router.rs           ── Router::new() + RoundRobin + resolve()
```

### 3.2 크레이트 의존 관계

```
gadgetron-cli
  ├── gadgetron-core     (ProviderConfig::Gemini 추가)
  ├── gadgetron-xaas     (tenant_list, key_list SQL; key_revoke + invalidate)
  └── gadgetron-provider (GeminiProvider 구성)

gadgetron-provider
  └── gadgetron-core     (LlmProvider trait, ChatRequest/Response/Chunk)

gadgetron-gateway [dev]
  └── gadgetron-testing  (FakeLlmProvider for benches)

gadgetron-tui
  └── gadgetron-core     (WsMessage, GpuMetrics 등 — 기존 그대로)
```

### 3.3 D-12 크레이트 경계표 준수

| 크레이트 | 이번 변경 | D-12 준수 |
|----------|----------|-----------|
| `gadgetron-cli` | `tenant_list`, `key_list`, `key_revoke` 함수 추가 | 진입점 전용, 경계 위반 없음 |
| `gadgetron-provider` | `gemini.rs` 모듈 추가 | LLM 어댑터 전용 크레이트, 경계 위반 없음 |
| `gadgetron-core` | `ProviderConfig::Gemini` 추가 | 공유 타입/설정 크레이트, 경계 위반 없음 |
| `gadgetron-tui` | `FocusedPanel`, `ScrollState` 신규 타입 (내부 상태) | TUI 전용, `gadgetron-core` 의존 없는 신규 타입 — 경계 위반 없음 |
| `gadgetron-gateway` | `benches/` 3개 파일 추가 (dev-only) | gateway 테스트 harness 확장, 경계 위반 없음 |

`gadgetron-xaas`에 새 타입 추가 없음. `PgKeyValidator::invalidate()`는 기존 메서드 그대로 사용.

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

#### 4.1.1 CLI — `gadgetron-cli/src/main.rs`

| 테스트 함수 | 검증 invariant |
|------------|----------------|
| `test_tenant_list_empty` | 행 0개 → "No tenants found." 메시지 출력, `Ok(())` 반환 |
| `test_tenant_list_rows` | 2개 행 insert 후 `tenant_list()` 호출 → stdout에 2행 포함 (ID, Name, Status, Created 열 검증) |
| `test_key_list_empty` | 해당 tenant의 active key 0개 → "No keys found." 메시지, `Ok(())` |
| `test_key_list_rows` | key 2개 insert 후 `key_list()` 호출 → 2행 포함 검증 |
| `test_key_revoke_ok` | key insert → `key_revoke()` → DB에서 `revoked_at IS NOT NULL` 확인 |
| `test_key_revoke_not_found` | 존재하지 않는 UUID → `Err` 반환, 메시지에 "not found" 포함 |
| `test_key_revoke_invalidates_cache` | `MockKeyValidator` + `key_revoke(..., Some(&mock))` → `invalidate` 1회 호출 |

이 테스트들은 PostgreSQL이 필요하므로 `#[cfg(feature = "integration")]` 게이트 또는 `testcontainers` 기반으로 작성 (D-20260411-05 결정 준수).

#### 4.1.2 TUI — `gadgetron-tui/src/app.rs`

| 테스트 함수 | 검증 invariant |
|------------|----------------|
| `focused_panel_next_rotates` | `Nodes.next() == Models`, `Models.next() == Requests`, `Requests.next() == Nodes` |
| `handle_key_tab_advances_focus` | 초기 `Nodes`, Tab → `Models`, Tab → `Requests`, Tab → `Nodes` |
| `handle_key_down_increments_scroll` | focus=Nodes, Down → `scroll.nodes == 1` |
| `handle_key_up_at_zero_stays_zero` | focus=Nodes, Up → `scroll.nodes == 0` (saturating_sub) |
| `handle_key_quit_stops_running` | 'q' → `running == false` |
| `handle_key_esc_stops_running` | Esc → `running == false` |
| `scroll_state_default_all_zero` | `ScrollState::default()` → nodes=0, models=0, requests=0 |
| `focused_panel_default_is_nodes` | `App::new().focused == FocusedPanel::Nodes` |
| `with_channel_inherits_focus_default` | `App::with_channel(rx).focused == FocusedPanel::Nodes` |

#### 4.1.3 Gemini — `gadgetron-provider/src/gemini.rs`

| 테스트 함수 | 검증 invariant |
|------------|----------------|
| `generate_url_contains_key` | `generate_url("gemini-1.5-pro")` → URL에 `?key=test_key` 포함 |
| `stream_url_contains_alt_sse` | `stream_url("gemini-1.5-flash")` → URL에 `?alt=sse&key=` 포함 |
| `to_gemini_request_maps_assistant_to_model` | `role: "assistant"` → `GeminiContent.role == "model"` |
| `to_gemini_request_maps_user_role` | `role: "user"` → `GeminiContent.role == "user"` |
| `extract_text_returns_text` | 유효한 `GeminiResponse` → `Ok(text_content)` |
| `extract_text_empty_candidates_is_err` | `candidates: []` → `Err(GadgetronError::Provider(...))` |
| `models_returns_configured_list` | `with_models(["a","b"])` + `models().await` → 2개 반환 |
| `chat_ok` (wiremock) | mock server → 200 + Gemini JSON → `Ok(ChatResponse)` |
| `chat_error_status` (wiremock) | mock server → 400 → `Err(GadgetronError::Provider(...))` |
| `chat_stream_yields_chunks` (wiremock) | mock SSE stream → `Stream` yields 2 `ChatChunk` |

### 4.2 테스트 하네스

- **CLI 테스트**: `testcontainers::GenericImage("postgres:16")` 또는 `testcontainers-modules::postgres::Postgres` + sqlx migrate. `AuditWriter`와 `KeyValidator` mock은 불필요 — SQL 직접 검증.
- **TUI 테스트**: `App::new()` / `App::with_channel()` + `handle_key()` 직접 호출. 터미널/ratatui 의존 없음.
- **Gemini 테스트**: `wiremock::MockServer` (from `gadgetron-testing` 또는 로컬 dev-dep). `GeminiProvider::with_base_url(mock_server.uri())` 패턴.

### 4.3 커버리지 목표

| 모듈 | Line 목표 | Branch 목표 |
|------|----------|------------|
| `gadgetron-cli` (신규 3 함수) | ≥ 85% | ≥ 75% |
| `gadgetron-tui/app.rs` (신규 메서드) | ≥ 90% | ≥ 85% |
| `gadgetron-provider/gemini.rs` | ≥ 80% | ≥ 70% |

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

#### 5.1.1 CLI end-to-end (PostgreSQL 필요)

시나리오 `tests/cli_tenant_key_lifecycle.rs`:

1. `connect_pg()` → PostgreSQL 연결 (testcontainers).
2. `tenant_create(&pool, "acme")` → `Uuid` 반환.
3. `tenant_list(&pool)` → stdout에 "acme" 포함 검증.
4. `key_create(&pool, tenant_id, "OpenAiCompat")` → key insert.
5. `key_list(&pool, tenant_id)` → 1행 포함 검증.
6. `key_revoke(&pool, key_id, None)` → `Ok(())`.
7. `key_list(&pool, tenant_id)` → 0행 (revoked key는 미표시).

#### 5.1.2 TUI broadcast 채널 wiring 검증

시나리오 `tests/tui_broadcast_wiring.rs`:

1. `broadcast::channel::<WsMessage>(16)` 생성.
2. `App::with_channel(rx)` 생성.
3. `tx.send(WsMessage::NodeUpdate(vec![...]))` 2회.
4. `app.drain_updates()` 호출.
5. `app.gpu_metrics.read().unwrap().len() == 1` (마지막 NodeUpdate가 반영됨).
6. `tx.send(WsMessage::RequestLog(entry))` 3회.
7. `app.drain_updates()`.
8. `app.request_log.read().unwrap().len() == 4` (기존 demo 3개 + 신규 3개가 100 cap 내).

이 테스트는 `gadgetron-tui`의 `#[cfg(test)]` 내부에 이미 있는 `drain_updates_*` 테스트들과 보완 관계이며, `serve()` 경로를 직접 재현한다.

#### 5.1.3 Gemini 통합 (wiremock)

시나리오 `gadgetron-testing/tests/gemini_integration.rs`:

1. `wiremock::MockServer::start()`.
2. 비스트리밍: `POST .../gemini-1.5-pro:generateContent` mock → 유효한 Gemini JSON 응답.
3. `GeminiProvider::new("test_key").with_base_url(mock_server.uri())`.
4. `provider.chat(req).await` → `Ok(ChatResponse { choices[0].message.content == "Hello" })`.
5. 스트리밍: `POST .../gemini-1.5-pro:streamGenerateContent` mock → SSE stream ("data: {...}\n\n" 2개).
6. `provider.chat_stream(req).collect::<Vec<_>>().await` → 2개 `Ok(ChatChunk)`.
7. 에러: 500 응답 → `Err(GadgetronError::Provider(...))`.

#### 5.1.4 벤치마크 CI 통과 기준

```
cargo bench -p gadgetron-gateway
```

P99 수치를 `criterion` 출력에서 확인. CI에서는 `--noplot --output-format bencher` 플래그로 숫자만 수집. 목표 미달은 CI 실패가 아닌 경고로 처리 (flaky 방지).

### 5.2 테스트 환경

| 의존성 | 버전 | 용도 |
|--------|------|------|
| `testcontainers-modules::postgres::Postgres` | 0.11 | CLI + lifecycle 통합 테스트 |
| `wiremock` | `gadgetron-testing`에서 re-export | Gemini HTTP mock |
| `broadcast::channel` | tokio 내장 | TUI wiring 검증 |
| Criterion | workspace 0.5 | gateway 벤치마크 |

`docker` 또는 `podman`은 testcontainers가 자동 감지. CI 환경에서 `TESTCONTAINERS_RYUK_DISABLED=true` 설정.

### 5.3 회귀 방지

| 변경 | 실패시켜야 할 테스트 |
|------|---------------------|
| `tenant_list` SQL 변경 (컬럼 제거) | `test_tenant_list_rows` |
| `key_revoke` RETURNING 제거 | `test_key_revoke_invalidates_cache` |
| `FocusedPanel::next()` 순환 변경 | `handle_key_tab_advances_focus` |
| `ScrollState` 필드명 변경 | `handle_key_down_increments_scroll` |
| Gemini URL 구조 변경 | `generate_url_contains_key`, `stream_url_contains_alt_sse` |
| `GeminiCandidate.finish_reason` serde rename 제거 | `chat_stream_yields_chunks` |
| `broadcast::Receiver`를 `with_channel` 대신 직접 `update_rx`에 삽입 | `tui_broadcast_wiring` 통합 테스트 |

---

## 6. Phase 구분

| 항목 | Phase |
|------|-------|
| CLI: `tenant_list`, `key_list`, `key_revoke` | [P1] — D-20260411-01 MVP 범위 |
| Criterion 벤치마크 3종 | [P1] — D-20260411-05 테스트 인프라 |
| TUI `FocusedPanel` + `ScrollState` + `handle_key()` | [P1] — 운영자 UX |
| TUI Tab/↑↓ 키 바인딩 | [P1] |
| Gemini adapter `GeminiProvider` | [P1] — D-20260411-01 6-provider 범위, D-20260411-02 Week 5-7 |
| `ProviderConfig::Gemini` in `gadgetron-core` | [P1] |
| Gemini `functionCall` ↔ `tool_calls` 변환 | [P2] — D-20260411-02 Week 7 |
| Gemini character 과금 → token 환산 | [P2] — D-20260411-02 Week 7 |
| TUI Phase 2 Web Socket live-push (WebSocket 서버) | [P2] |
| CLI `key revoke --all-for-tenant` 배치 커맨드 | [P2] |

---

## 7. 오픈 이슈 / 의사결정 필요

| ID | 내용 | 옵션 | 추천 | 상태 |
|----|------|------|------|------|
| Q-1 | `key_revoke`에서 `validator.invalidate`를 `main()`에서 직접 호출할 수 없음 — revoke 시 `PgKeyValidator` 인스턴스가 `serve()` 내부에만 존재. CLI는 별도 프로세스로 실행됨 | A. validator=None 전달 (캐시는 10분 TTL로 자연 만료) / B. `serve`와 `revoke`를 같은 프로세스에서 실행하는 admin API 추가 | A — Phase 1에서 10분 TTL은 허용 가능한 보안 창. Phase 2에서 admin API 경로로 해결 | 설계 결정 완료 (A 채택) |
| Q-2 | `GeminiContent`의 `Serialize + Deserialize` 양쪽 derive — 요청은 `role: "user"/"model"`, 응답도 동일. 단일 타입으로 충분한가? | A. 단일 `GeminiContent` (#[derive(Serialize, Deserialize)]) / B. 요청용 `GeminiContentReq`, 응답용 `GeminiContentResp` 분리 | A — 구조 동일, 중복 불필요 | 설계 결정 완료 (A 채택) |
| Q-3 | 벤치마크 P99 목표 달성 실패 시 CI 정책 | A. warning only / B. CI fail | A — 초기 벤치 baseline 확보 후 B로 전환 | 설계 결정 완료 (A 채택) |

---

## 리뷰 로그 (append-only)

### Round 1 — 2026-04-12 — 대기 중
**결론**: 미진행

**체크리스트**: (`03-review-rubric.md §1` 기준)
- [ ] 인터페이스 계약
- [ ] 크레이트 경계
- [ ] 타입 중복 검사
- [ ] 에러 처리 완결성

**Action Items**: 없음 (리뷰 전)

**다음 라운드 조건**: Round 1 Pass 후 Round 2 진행.
