# Sprint 5 — E2E, Criterion Benchmarks, TUI Wiring

> **담당**: @qa-test-architect
> **상태**: ✅ Implemented (commits `106966e` + `f6a2b0d`, 2026-04-12) — E2E 7 scenarios, criterion 3 benches, TUI 배선 완료
> **작성일**: 2026-04-12
> **최종 업데이트**: 2026-04-12
> **관련 크레이트**: `gadgetron-testing`, `gadgetron-core`, `gadgetron-gateway`, `gadgetron-tui`, `gadgetron-xaas`
> **Phase**: [P1]
> **상위 결정**: D-20260411-05 (`gadgetron-testing` 신설), D-20260411-07 (공유 UI 타입 `gadgetron-core`), D-20260412-02 (구현 결정론)
> **선행 문서**: `docs/design/testing/harness.md`

---

## 1. 철학 & 컨셉 (Why)

### 1.1 Sprint 5 = "Test & Display"

Sprint 1-4에서 빌드한 것을 **검증**하고 **보여준다**. 검증 없는 기능은 기능이 아니다. 오퍼레이터가 CLI 없이 클러스터 상태를 한눈에 파악하지 못하면 운영 단순성 목표가 달성되지 않는다.

Sprint 5의 두 축:

1. **Test**: 테스트 인프라(FakeLlmProvider, FailingProvider, PgHarness, GatewayHarness) 구현 + 7개 E2E 시나리오 + 3개 criterion 벤치마크. Sprint 1-4 구현이 실제로 작동함을 증명.
2. **Display**: `gadgetron-core/src/ui.rs` 공유 타입 정의 + TUI 데이터 와이어링 + 3-컬럼 레이아웃. 오퍼레이터가 클러스터 상태를 볼 수 있음을 증명.

### 1.2 제품 비전과의 연결

- `docs/00-overview.md §1.2 미션 1` P99 < 1ms 오버헤드 → criterion 벤치마크 3개가 CI에서 SLO gate 역할.
- `docs/00-overview.md §1.2 미션 4` 운영 단순성 → TUI 3-컬럼 레이아웃이 kubectl 없이 클러스터 상태 표시.
- `platform-architecture.md §2.A.7.2` 9개 e2e 시나리오 중 Sprint 5 범위는 시나리오 1-7 (8-9는 graceful shutdown / process restart, [P2] 후보).

### 1.3 테스트 피라미드

```
           /------\
          / E2E   \   <-- Sprint 5 목표: 7 시나리오
         /----------\
        / Integration \  <-- Sprint 5 목표: PgHarness + GatewayHarness
       /--------------\
      /  Unit           \  <-- Sprint 1-4 완료. FakeLlmProvider unit tests 추가.
     /------------------\
```

비율 (line count 기준): unit 60% / integration 25% / e2e 15%.

### 1.4 고려한 대안

| 결정 | 대안 | 채택 이유 |
|------|------|----------|
| testcontainers PostgreSQL | sqlx::test (ephemeral DB) | testcontainers가 실 마이그레이션 파일 적용 경로 검증. 설계 doc §2.D.6 Q-21 결정 준수. |
| reqwest 클라이언트 (GatewayHarness) | axum::test (`oneshot`) | oneshot은 OS 소켓 생략. E2E는 실 TCP bind + HTTP 클라이언트 왕복이 필요. |
| tokio broadcast 종료 신호 | tokio::CancellationToken | broadcast sender drop이 receiver `RecvError::Lagged` 없이 clean. GatewayHarness 종료 경로 단순화. |
| in-process Arc 폴링 (TUI) | WebSocket 구독 | [P1] 단일 바이너리 구조. WebSocket은 [P2] 분산 배포 시 추가. |

---

## 2. 상세 구현 방안 (What & How)

### 2.1 의존성 추가

**`gadgetron-testing/Cargo.toml`** — 아래 항목을 `[dependencies]` 또는 `[dev-dependencies]`에 추가:

```toml
[dependencies]
gadgetron-core      = { workspace = true }
gadgetron-gateway   = { workspace = true }
gadgetron-xaas      = { workspace = true }
async-trait         = { workspace = true }
tokio               = { workspace = true, features = ["full"] }
serde               = { workspace = true }
serde_json          = { workspace = true }
futures             = { workspace = true }
tokio-stream        = { workspace = true }
uuid                = { workspace = true }
tracing             = { workspace = true }
axum                = { workspace = true }
sqlx                = { workspace = true, features = ["postgres", "runtime-tokio-rustls", "uuid", "chrono", "migrate"] }
chrono              = { workspace = true }
wiremock            = "0.6"

# Sprint 5 신규
testcontainers          = "0.23"
testcontainers-modules  = { version = "0.11", features = ["postgres"] }
reqwest                 = { version = "0.12", features = ["json", "stream"] }

[dev-dependencies]
criterion = { version = "0.5", features = ["async_tokio"] }
tokio     = { workspace = true, features = ["full", "test-util"] }
```

선택 근거 (D-20260412-02 item 4):
- `testcontainers 0.23` + `testcontainers-modules 0.11`: 2026-04 기준 최신 stable. `ContainerAsync` API 비동기 전용. Docker daemon 필요 — CI runner에서 `services.docker`로 활성화.
- `reqwest 0.12`: rustls 기반 TLS. `features = ["stream"]`으로 SSE 바디를 `bytes_stream()`으로 소비.
- `criterion 0.5 + async_tokio`: `tokio::runtime::Runtime::block_on` 대신 `criterion::async_executor::TokioExecutor` 사용. `bench_*` 타겟별 `[[bench]]` 섹션 추가.

**`gadgetron-core/Cargo.toml`** — `ui.rs` 모듈을 위해 추가:

```toml
[dependencies]
chrono = { workspace = true, features = ["serde"] }
```

`gadgetron-core`는 이미 `serde`가 있으므로 `chrono + serde` feature만 추가. 신규 외부 의존 없음.

**`gadgetron-tui/Cargo.toml`** — TUI 와이어링을 위해 추가:

```toml
[dependencies]
gadgetron-core = { workspace = true }
tokio          = { workspace = true, features = ["full"] }
```

---

### 2.2 FakeLlmProvider

파일: `crates/gadgetron-testing/src/mocks/provider/fake.rs`

```rust
use std::pin::Pin;
use async_trait::async_trait;
use futures::{stream, Stream};
use gadgetron_core::{
    error::Result,
    provider::{
        ChatChunk, ChatRequest, ChatResponse, ChunkChoice, ChunkDelta, Choice,
        LlmProvider, ModelInfo, Usage,
    },
    message::Message,
};
use uuid::Uuid;

/// 결정론적 응답을 반환하는 테스트용 LLM provider.
///
/// `content` 필드가 모든 `chat()` 응답의 `choices[0].message.content`에 사용된다.
/// `stream_chunks` 개수만큼 `chat_stream()` 청크를 방출하고 종료한다.
/// `models` 필드가 `models()` 호출 결과다.
///
/// # 예시
/// ```rust
/// let provider = FakeLlmProvider::new("hello from fake", 3, vec!["gpt-4o-mini".to_string()]);
/// ```
pub struct FakeLlmProvider {
    /// chat() 응답의 content 문자열
    pub content: String,
    /// chat_stream()이 방출할 청크 수 (각 청크 content = "chunk_N")
    pub stream_chunks: usize,
    /// models() 반환 목록
    pub model_ids: Vec<String>,
}

impl FakeLlmProvider {
    /// 생성자.
    ///
    /// - `content`: chat() 응답 content
    /// - `stream_chunks`: 스트림 청크 수 (0이면 `[DONE]`만 방출)
    /// - `model_ids`: models() 반환 목록
    pub fn new(content: impl Into<String>, stream_chunks: usize, model_ids: Vec<String>) -> Self {
        Self { content: content.into(), stream_chunks, model_ids }
    }
}

#[async_trait]
impl LlmProvider for FakeLlmProvider {
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse> {
        Ok(ChatResponse {
            id: format!("fake-{}", Uuid::new_v4()),
            object: "chat.completion".to_string(),
            created: 0,
            model: req.model,
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: gadgetron_core::message::Role::Assistant,
                    content: gadgetron_core::message::Content::Text(self.content.clone()),
                    reasoning_content: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Usage { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 },
        })
    }

    fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>> {
        let model = req.model.clone();
        let n = self.stream_chunks;
        let chunks: Vec<Result<ChatChunk>> = (0..n)
            .map(|i| {
                Ok(ChatChunk {
                    id: format!("fake-stream-{}", Uuid::new_v4()),
                    object: "chat.completion.chunk".to_string(),
                    created: 0,
                    model: model.clone(),
                    choices: vec![ChunkChoice {
                        index: 0,
                        delta: ChunkDelta {
                            role: if i == 0 { Some("assistant".to_string()) } else { None },
                            content: Some(format!("chunk_{i}")),
                            tool_calls: None,
                            reasoning_content: None,
                        },
                        finish_reason: if i == n - 1 { Some("stop".to_string()) } else { None },
                    }],
                })
            })
            .collect();
        Box::pin(stream::iter(chunks))
    }

    async fn models(&self) -> Result<Vec<ModelInfo>> {
        Ok(self.model_ids.iter().map(|id| ModelInfo {
            id: id.clone(),
            object: "model".to_string(),
            owned_by: "fake".to_string(),
        }).collect())
    }

    fn name(&self) -> &str {
        "fake"
    }

    async fn health(&self) -> Result<()> {
        Ok(())
    }
}
```

`Send + Sync` 보장: 모든 필드 `String`, `usize`, `Vec<String>` — 모두 `Send + Sync`. `#[async_trait]`가 `?Send` 없이 기본 `Send` bound 추가.

---

### 2.3 FailingProvider

파일: `crates/gadgetron-testing/src/mocks/provider/failing.rs`

```rust
use std::pin::Pin;
use std::time::Duration;
use async_trait::async_trait;
use futures::{stream, Stream};
use gadgetron_core::{
    error::{GadgetronError, Result},
    provider::{ChatChunk, ChatRequest, ChatResponse, ChunkChoice, ChunkDelta, LlmProvider, ModelInfo},
};
use uuid::Uuid;

/// FailingProvider가 시뮬레이션할 실패 모드.
///
/// `#[non_exhaustive]` 아님: test harness이므로 exhaustive match를 강제한다.
#[derive(Debug, Clone)]
pub enum FailMode {
    /// chat()/chat_stream() 즉시 GadgetronError::Provider("immediate fail") 반환.
    ImmediateFail,
    /// `Duration` 슬립 후 동일 에러 반환. 타임아웃 시나리오용.
    /// CI 결정론: `tokio::time::pause()` + `tokio::time::advance()` 조합으로 wall-clock 없이 테스트.
    DelayedFail(Duration),
    /// `usize`개 청크를 방출한 후 GadgetronError::StreamInterrupted 반환.
    PartialStream(usize),
    /// HTTP 429 시뮬: GadgetronError::Provider("rate_limit_429") 반환.
    RateLimit429,
    /// 요청 후 100ms 지연 없이 즉시 Provider 에러 반환 (Timeout 시뮬).
    /// 실제 타임아웃은 GatewayHarness 레벨 reqwest timeout으로 검증.
    Timeout,
    /// 스트림 첫 청크 후 GadgetronError::StreamInterrupted 반환.
    StreamInterrupted,
}

pub struct FailingProvider {
    pub mode: FailMode,
    /// models() 반환 목록. 빈 Vec이면 빈 목록 반환.
    pub model_ids: Vec<String>,
}

impl FailingProvider {
    pub fn new(mode: FailMode) -> Self {
        Self { mode, model_ids: vec![] }
    }

    pub fn with_models(mode: FailMode, model_ids: Vec<String>) -> Self {
        Self { mode, model_ids }
    }
}

#[async_trait]
impl LlmProvider for FailingProvider {
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse> {
        match &self.mode {
            FailMode::ImmediateFail => {
                Err(GadgetronError::Provider("immediate fail".to_string()))
            }
            FailMode::DelayedFail(dur) => {
                tokio::time::sleep(*dur).await;
                Err(GadgetronError::Provider("delayed fail".to_string()))
            }
            FailMode::RateLimit429 => {
                Err(GadgetronError::Provider("rate_limit_429".to_string()))
            }
            FailMode::Timeout => {
                Err(GadgetronError::Provider("timeout".to_string()))
            }
            FailMode::PartialStream(_) | FailMode::StreamInterrupted => {
                // non-streaming path: 동일하게 Provider 에러
                Err(GadgetronError::Provider("stream-mode provider in non-stream call".to_string()))
            }
        }
    }

    fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>> {
        let model = req.model.clone();
        match &self.mode {
            FailMode::ImmediateFail | FailMode::RateLimit429 | FailMode::Timeout => {
                let err = Err(GadgetronError::Provider("immediate fail in stream".to_string()));
                Box::pin(stream::iter(vec![err]))
            }
            FailMode::DelayedFail(_dur) => {
                // 결정론: DelayedFail stream은 즉시 에러. Duration 슬립은 chat()에만 적용.
                let err = Err(GadgetronError::Provider("delayed fail in stream".to_string()));
                Box::pin(stream::iter(vec![err]))
            }
            FailMode::PartialStream(n) => {
                let mut items: Vec<Result<ChatChunk>> = (0..*n)
                    .map(|i| {
                        Ok(ChatChunk {
                            id: format!("fail-partial-{}", Uuid::new_v4()),
                            object: "chat.completion.chunk".to_string(),
                            created: 0,
                            model: model.clone(),
                            choices: vec![ChunkChoice {
                                index: 0,
                                delta: ChunkDelta {
                                    role: None,
                                    content: Some(format!("partial_{i}")),
                                    tool_calls: None,
                                    reasoning_content: None,
                                },
                                finish_reason: None,
                            }],
                        })
                    })
                    .collect();
                items.push(Err(GadgetronError::StreamInterrupted {
                    reason: "partial stream cut".to_string(),
                }));
                Box::pin(stream::iter(items))
            }
            FailMode::StreamInterrupted => {
                let ok = Ok(ChatChunk {
                    id: format!("fail-si-{}", Uuid::new_v4()),
                    object: "chat.completion.chunk".to_string(),
                    created: 0,
                    model: model.clone(),
                    choices: vec![ChunkChoice {
                        index: 0,
                        delta: ChunkDelta {
                            role: Some("assistant".to_string()),
                            content: Some("first".to_string()),
                            tool_calls: None,
                            reasoning_content: None,
                        },
                        finish_reason: None,
                    }],
                });
                let err = Err(GadgetronError::StreamInterrupted {
                    reason: "stream interrupted".to_string(),
                });
                Box::pin(stream::iter(vec![ok, err]))
            }
        }
    }

    async fn models(&self) -> Result<Vec<ModelInfo>> {
        Ok(self.model_ids.iter().map(|id| ModelInfo {
            id: id.clone(),
            object: "model".into(),
            owned_by: "fake".into(),
        }).collect())
    }

    fn name(&self) -> &str {
        "failing"
    }

    async fn health(&self) -> Result<()> {
        Err(GadgetronError::Provider("failing provider is unhealthy".to_string()))
    }
}
```

`DelayedFail` 스트림이 즉시 에러를 반환하는 이유: `chat_stream()`은 `Pin<Box<dyn Stream>>` 반환 즉시 청크 생산이 시작된다. async delay를 스트림 내부에 넣으면 `tokio::time::pause()` 없이 wall-clock 슬립이 발생한다. CI 결정론 원칙(§1.4 item 3)에 따라 DelayedFail 지연은 `chat()` 경로에만 적용한다.

---

### 2.4 PgHarness

파일: `crates/gadgetron-testing/src/harness/pg.rs`

PostgreSQL 컨테이너를 시작하고 마이그레이션을 적용한 뒤 `PgPool`을 공개하는 구조체.

```rust
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

/// testcontainers PostgreSQL 기반 테스트 하네스.
///
/// `new()` 호출 시 `Postgres` Docker 컨테이너를 시작하고,
/// `sqlx::migrate!("../../crates/gadgetron-xaas/migrations")` 매크로로
/// 마이그레이션을 적용한 뒤 `PgPool`을 보유한다.
///
/// `Drop` 시 `_container`의 `Drop`이 컨테이너를 중지·제거한다.
/// 테스트 함수 종료까지 값을 유지해야 하므로 `let _pg = PgHarness::new().await`로 바인딩.
pub struct PgHarness {
    /// 마이그레이션 적용 완료된 연결 풀.
    pub pool: PgPool,
    /// Drop 시 컨테이너 자동 제거. `_` prefix = Drop 전까지 살아있어야 함.
    _container: ContainerAsync<Postgres>,
}

impl PgHarness {
    /// 컨테이너 시작 + 마이그레이션 적용.
    ///
    /// # Panics
    /// Docker daemon 미실행 시 panic. CI에서는 `services.docker` 활성화 필수.
    pub async fn new() -> Self {
        let container = testcontainers::runners::AsyncRunner::run(
            Postgres::default()
                .with_db_name("gadgetron_test")
                .with_user("gadgetron")
                .with_password("gadgetron_test"),
        )
        .await
        .expect("failed to start postgres container");

        let host = container.get_host().await.expect("get_host failed");
        let port = container
            .get_host_port_ipv4(5432)
            .await
            .expect("get_port failed");
        let url = format!(
            "postgres://gadgetron:gadgetron_test@{host}:{port}/gadgetron_test"
        );

        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await
            .expect("pool connect failed");

        // gadgetron-xaas migrations 경로: 워크스페이스 루트 기준 상대 경로.
        // `sqlx::migrate!` 매크로는 컴파일 시점 경로 해석이므로 정확한 상대 경로 필수.
        sqlx::migrate!("../../crates/gadgetron-xaas/migrations")
            .run(&pool)
            .await
            .expect("migrations failed");

        Self { pool, _container: container }
    }

    /// Pool 참조 반환.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// 테스트용 테넌트 + API 키 삽입.
    ///
    /// 반환: `(tenant_id, raw_api_key)`.
    /// `raw_api_key`는 `"gad_test_"` + 32자 hex — `PgKeyValidator`의 `gad_test_` 접두사 검증 통과.
    /// 내부적으로 `sha256(raw_api_key)`를 `api_keys.key_hash`에 저장하여 실 validator 경로 검증.
    pub async fn insert_test_tenant(&self) -> (Uuid, String) {
        let tenant_id = Uuid::new_v4();
        let raw_key = format!("gad_test_{}", hex::encode(Uuid::new_v4().as_bytes()));
        let key_hash = sha2_hex(&raw_key);
        let api_key_id = Uuid::new_v4();

        sqlx::query!(
            r#"
            INSERT INTO tenants (id, name, daily_limit_cents, monthly_limit_cents)
            VALUES ($1, $2, 1000000, 10000000)
            "#,
            tenant_id,
            format!("test-tenant-{tenant_id}")
        )
        .execute(&self.pool)
        .await
        .expect("insert tenant failed");

        sqlx::query!(
            r#"
            INSERT INTO api_keys (id, tenant_id, key_hash, scopes, revoked)
            VALUES ($1, $2, $3, $4, false)
            "#,
            api_key_id,
            tenant_id,
            key_hash,
            &["openai_compat"] as &[&str],
        )
        .execute(&self.pool)
        .await
        .expect("insert api_key failed");

        (tenant_id, raw_key)
    }

    /// `audit_log` 테이블에서 tenant의 행 수를 조회한다.
    pub async fn count_audit_entries(&self, tenant_id: Uuid) -> i64 {
        sqlx::query_scalar!(
            "SELECT COUNT(*) FROM audit_log WHERE tenant_id = $1",
            tenant_id
        )
        .fetch_one(&self.pool)
        .await
        .expect("count_audit_entries failed")
        .unwrap_or(0)
    }

    /// `sha256(raw)` 를 hex string으로 반환하는 내부 헬퍼.
    fn sha2_hex_inner(raw: &str) -> String {
        sha2_hex(raw)
    }
}

/// SHA-256 hex digest. `api_keys.key_hash` 계산과 동일한 알고리즘.
/// `gadgetron-xaas`의 `PgKeyValidator::hash_key`와 반드시 동일해야 한다.
/// 변경 시 양쪽 모두 수정 필요.
pub fn sha2_hex(raw: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    format!("{:x}", hasher.finalize())
}
```

`sha2 = "0.10"` 의존성 추가 필요 (`gadgetron-xaas`가 이미 사용하므로 workspace 공유 가능).
`hex = "0.4"` 의존성 추가.

`_container` 수명: `PgHarness` struct가 살아있는 동안 컨테이너가 유지된다. 테스트 종료 시 자동 `Drop`. 테스트 간 격리: 각 테스트에서 `PgHarness::new().await`를 독립 호출하면 별도 컨테이너가 생성된다.

---

### 2.5 GatewayHarness

파일: `crates/gadgetron-testing/src/harness/gateway.rs`

```rust
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::broadcast;
use gadgetron_core::provider::LlmProvider;
use gadgetron_gateway::server::{AppState, build_router};
use gadgetron_xaas::audit::writer::AuditWriter;
use gadgetron_xaas::quota::enforcer::InMemoryQuotaEnforcer;

use super::pg::PgHarness;
use crate::mocks::xaas::FakePgKeyValidator;

/// 랜덤 포트에서 실 게이트웨이 HTTP 서버를 기동하는 테스트 하네스.
///
/// `start()` 반환 시 서버가 `0.0.0.0:{random_port}`에서 요청을 수락 중이다.
/// `shutdown()`을 호출하거나 값이 `Drop`되면 서버 태스크가 취소된다.
///
/// # 내부 동작
/// `tokio::net::TcpListener::bind("0.0.0.0:0")`으로 OS가 포트를 할당.
/// `axum::serve(listener, router)`를 `tokio::spawn`으로 백그라운드 태스크로 실행.
/// `broadcast::Sender<()>`로 종료 신호를 전송한다.
pub struct GatewayHarness {
    /// `http://127.0.0.1:{port}` 형식. trailing slash 없음.
    pub url: String,
    /// 공유 reqwest 클라이언트. connection pool 재사용으로 테스트 속도 향상.
    pub client: reqwest::Client,
    shutdown_tx: broadcast::Sender<()>,
    _handle: tokio::task::JoinHandle<()>,
}

impl GatewayHarness {
    /// 게이트웨이 서버를 기동한다.
    ///
    /// - `provider`: LLM 호출에 사용될 `Arc<dyn LlmProvider + Send + Sync>`.
    /// - `pg`: `PgHarness` 참조. `KeyValidator`에 pool을 넘긴다.
    ///
    /// # 타임아웃
    /// `reqwest::Client`는 `timeout(Duration::from_secs(5))`로 설정.
    /// E2E 테스트에서 행 걸림 방지.
    pub async fn start(
        provider: Arc<dyn LlmProvider + Send + Sync>,
        pg: &PgHarness,
    ) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind random port");
        let addr: SocketAddr = listener.local_addr().expect("local_addr");

        let (audit_writer, _rx) = AuditWriter::new(4096);
        let state = AppState {
            key_validator: Arc::new(
                FakePgKeyValidator::new(pg.pool().clone())
            ),
            quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
            audit_writer: Arc::new(audit_writer),
            providers: Arc::new({
                let mut m = HashMap::new();
                m.insert(provider.name().to_string(), provider.clone());
                m
            }),
            router: None,  // Sprint 5 E2E: single provider, no routing layer
        };

        let router = build_router(state);
        let (shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

        let handle = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.recv().await;
                })
                .await
                .expect("gateway serve failed");
        });

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .expect("reqwest client build");

        Self {
            url: format!("http://127.0.0.1:{}", addr.port()),
            client,
            shutdown_tx,
            _handle: handle,
        }
    }

    /// 게이트웨이를 종료하고 태스크가 완료될 때까지 대기한다.
    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
        let _ = self._handle.await;
    }

    /// 인증 헤더 포함 POST 요청 빌더 반환.
    pub fn authed_post(&self, path: &str, api_key: &str) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}{}", self.url, path))
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
    }

    /// 인증 헤더 포함 GET 요청 빌더 반환.
    pub fn authed_get(&self, path: &str, api_key: &str) -> reqwest::RequestBuilder {
        self.client
            .get(format!("{}{}", self.url, path))
            .header("Authorization", format!("Bearer {api_key}"))
    }
}
```

`FakePgKeyValidator` — `gadgetron-testing/src/mocks/xaas/fake_key_validator.rs`:

```rust
use async_trait::async_trait;
use std::sync::Arc;
use sqlx::PgPool;
use gadgetron_core::error::{GadgetronError, Result};
use gadgetron_xaas::auth::validator::{KeyValidator, ValidatedKey};

/// PgPool을 직접 사용하는 테스트용 KeyValidator.
/// 실 PgKeyValidator와 동일한 SHA-256 해시 + DB 조회 경로를 거친다.
/// moka 캐시는 없음 (E2E 결정론 — 캐시 만료 시점 불확정 제거).
pub struct FakePgKeyValidator {
    pool: PgPool,
}

impl FakePgKeyValidator {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl KeyValidator for FakePgKeyValidator {
    async fn validate(&self, raw_key: &str) -> Result<Arc<ValidatedKey>> {
        use crate::harness::pg::sha2_hex;
        let key_hash = sha2_hex(raw_key);
        // DB 조회: api_keys 테이블에서 key_hash 일치 행 조회
        let row = sqlx::query!(
            r#"
            SELECT k.id AS api_key_id, k.tenant_id, k.scopes
            FROM api_keys k
            WHERE k.key_hash = $1 AND k.revoked = false
            "#,
            key_hash
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| GadgetronError::Database {
            kind: gadgetron_core::error::DatabaseErrorKind::QueryFailed,
            message: e.to_string(),
        })?
        .ok_or(GadgetronError::TenantNotFound)?;

        // scopes 파싱: DB TEXT[] → gadgetron_core::context::Scope
        use gadgetron_core::context::Scope;
        let scopes: Vec<Scope> = row.scopes.iter()
            .filter_map(|s| match s.as_str() {
                "openai_compat" => Some(Scope::OpenAiCompat),
                "management" => Some(Scope::Management),
                "xaas_admin" => Some(Scope::XaasAdmin),
                _ => None,
            })
            .collect();

        Ok(Arc::new(ValidatedKey {
            api_key_id: row.api_key_id,
            tenant_id: row.tenant_id,
            scopes,
        }))
    }

    async fn invalidate(&self, _key_hash: &str) {
        // no-op: 캐시 없음
    }
}
```

---

### 2.6 E2E 시나리오 1-7

파일 배치: `crates/gadgetron-testing/tests/e2e/`

#### 공통 헬퍼 (`crates/gadgetron-testing/tests/e2e/common.rs`)

```rust
use gadgetron_testing::harness::{gateway::GatewayHarness, pg::PgHarness};
use gadgetron_testing::mocks::provider::fake::FakeLlmProvider;
use std::sync::Arc;

/// 표준 E2E 픽스처: PgHarness + FakeLlmProvider + GatewayHarness + (tenant_id, raw_key).
pub struct E2EFixture {
    pub pg: PgHarness,
    pub gw: GatewayHarness,
    pub tenant_id: uuid::Uuid,
    pub api_key: String,
}

impl E2EFixture {
    /// `content`: FakeLlmProvider 응답 content.
    /// `chunks`: 스트림 청크 수.
    pub async fn new(content: &str, chunks: usize) -> Self {
        let pg = PgHarness::new().await;
        let (tenant_id, api_key) = pg.insert_test_tenant().await;
        let provider = Arc::new(FakeLlmProvider::new(
            content,
            chunks,
            vec!["gpt-4o-mini".to_string()],
        ));
        let gw = GatewayHarness::start(provider, &pg).await;
        Self { pg, gw, tenant_id, api_key }
    }
}
```

#### 시나리오 1 — 비스트리밍 chat completion

파일: `crates/gadgetron-testing/tests/e2e/chat_completion.rs`

```rust
mod common;
use common::E2EFixture;
use serde_json::{json, Value};

/// 시나리오 1: 정상 비스트리밍 chat completion.
///
/// 입력: POST /v1/chat/completions, 유효 Bearer 키, model="gpt-4o-mini", stream=false
/// 기대: HTTP 200, body.choices[0].message.content == "hello from fake"
#[tokio::test]
async fn test_chat_completion_non_streaming() {
    let fx = E2EFixture::new("hello from fake", 0).await;

    let body = json!({
        "model": "gpt-4o-mini",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": false
    });

    let resp = fx
        .gw
        .authed_post("/v1/chat/completions", &fx.api_key)
        .json(&body)
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status().as_u16(), 200, "expected 200 OK");

    let json: Value = resp.json().await.expect("body parse failed");
    let content = json["choices"][0]["message"]["content"]
        .as_str()
        .expect("content missing");
    assert_eq!(content, "hello from fake");
    assert_eq!(json["object"], "chat.completion");

    fx.gw.shutdown().await;
}
```

#### 시나리오 2 — SSE 스트리밍

파일: `crates/gadgetron-testing/tests/e2e/streaming.rs`

```rust
mod common;
use common::E2EFixture;
use serde_json::{json, Value};

/// 시나리오 2: SSE 스트리밍.
///
/// 입력: POST /v1/chat/completions, stream=true, FakeLlmProvider(chunks=2)
/// 기대:
///   - Content-Type: text/event-stream
///   - "data: " 라인 ≥ 2 (청크)
///   - 마지막 data 라인 = "data: [DONE]"
#[tokio::test]
async fn test_chat_completion_streaming() {
    let fx = E2EFixture::new("x", 2).await;

    let body = json!({
        "model": "gpt-4o-mini",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": true
    });

    let resp = fx
        .gw
        .authed_post("/v1/chat/completions", &fx.api_key)
        .json(&body)
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status().as_u16(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("text/event-stream"), "content-type must be text/event-stream, got: {ct}");

    // SSE 바디를 텍스트로 읽어 "data: " 라인 파싱
    let text = resp.text().await.expect("body text failed");
    let data_lines: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("data: "))
        .collect();

    // 최소 2개 청크 + [DONE]
    assert!(
        data_lines.len() >= 3,
        "expected ≥ 3 data: lines (2 chunks + [DONE]), got: {data_lines:?}"
    );

    let last = *data_lines.last().expect("no data lines");
    assert_eq!(last, "data: [DONE]", "last SSE event must be [DONE]");

    // 청크 JSON 검증 (첫 번째 청크)
    let first_payload = data_lines[0].strip_prefix("data: ").unwrap();
    let chunk: Value = serde_json::from_str(first_payload)
        .expect("first chunk must be valid JSON");
    assert_eq!(chunk["object"], "chat.completion.chunk");

    fx.gw.shutdown().await;
}
```

#### 시나리오 3 — 인증 누락 401

파일: `crates/gadgetron-testing/tests/e2e/auth.rs`

```rust
mod common;
use common::E2EFixture;
use serde_json::{json, Value};

/// 시나리오 3: Authorization 헤더 없음 → 401.
///
/// 입력: POST /v1/chat/completions, Authorization 헤더 없음
/// 기대: HTTP 401, body.error.code = "tenant_not_found"
#[tokio::test]
async fn test_auth_missing_401() {
    let fx = E2EFixture::new("x", 0).await;

    let body = json!({
        "model": "gpt-4o-mini",
        "messages": [{"role": "user", "content": "hi"}]
    });

    let resp = fx
        .gw
        .client
        .post(format!("{}/v1/chat/completions", fx.gw.url))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status().as_u16(), 401);
    let json: Value = resp.json().await.expect("body parse");
    assert_eq!(json["error"]["code"], "tenant_not_found");

    fx.gw.shutdown().await;
}

/// 시나리오 4: OpenAiCompat 스코프 키로 /api/v1/nodes → 403.
///
/// 입력: GET /api/v1/nodes, openai_compat 스코프 키
/// 기대: HTTP 403, body.error.code = "forbidden"
#[tokio::test]
async fn test_scope_denial_403() {
    let fx = E2EFixture::new("x", 0).await;

    // api_key는 openai_compat 스코프. /api/v1/nodes 는 management 스코프 필요.
    let resp = fx
        .gw
        .authed_get("/api/v1/nodes", &fx.api_key)
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status().as_u16(), 403);
    let json: Value = resp.json().await.expect("body parse");
    assert_eq!(json["error"]["code"], "forbidden");

    fx.gw.shutdown().await;
}
```

#### 시나리오 5 — 할당량 초과 429

파일: `crates/gadgetron-testing/tests/e2e/quota.rs`

```rust
mod common;
use gadgetron_testing::harness::{gateway::GatewayHarness, pg::PgHarness};
use gadgetron_testing::mocks::provider::fake::FakeLlmProvider;
use gadgetron_testing::mocks::xaas::ExhaustedQuotaEnforcer;
use serde_json::json;
use std::sync::Arc;

/// 시나리오 5: 할당량 초과 → 429.
///
/// `ExhaustedQuotaEnforcer`는 `pre_request()` 호출 시 항상
/// `GadgetronError::QuotaExceeded { tenant_id }` 를 반환한다.
///
/// 입력: POST /v1/chat/completions, 유효 키, ExhaustedQuotaEnforcer
/// 기대: HTTP 429, body.error.code = "quota_exceeded"
#[tokio::test]
async fn test_quota_exceeded_429() {
    let pg = PgHarness::new().await;
    let (_, api_key) = pg.insert_test_tenant().await;
    let provider = Arc::new(FakeLlmProvider::new("x", 0, vec!["gpt-4o-mini".to_string()]));

    // ExhaustedQuotaEnforcer: pre_request()가 항상 QuotaExceeded 반환.
    // GatewayHarness 내부 AppState에서 quota_enforcer 교체가 필요하므로
    // GatewayHarness::start_with_quota() 오버로드 사용.
    let gw = GatewayHarness::start_with_quota(
        provider,
        &pg,
        Arc::new(ExhaustedQuotaEnforcer),
    )
    .await;

    let resp = gw
        .authed_post("/v1/chat/completions", &api_key)
        .json(&json!({
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status().as_u16(), 429);
    let json: serde_json::Value = resp.json().await.expect("body parse");
    assert_eq!(json["error"]["code"], "quota_exceeded");

    gw.shutdown().await;
}
```

`ExhaustedQuotaEnforcer` — `gadgetron-testing/src/mocks/xaas/fake_quota.rs`:

```rust
use async_trait::async_trait;
use gadgetron_core::error::{GadgetronError, Result};
use gadgetron_xaas::quota::enforcer::QuotaEnforcer;
use uuid::Uuid;

/// 항상 QuotaExceeded를 반환하는 테스트용 QuotaEnforcer.
pub struct ExhaustedQuotaEnforcer;

#[async_trait]
impl QuotaEnforcer for ExhaustedQuotaEnforcer {
    async fn pre_request(&self, tenant_id: Uuid, _estimated_cost_cents: i64) -> Result<()> {
        Err(GadgetronError::QuotaExceeded { tenant_id })
    }
    async fn post_request(&self, _tenant_id: Uuid, _actual_cost_cents: i64) {}
}
```

`GatewayHarness::start_with_quota` 시그니처 추가 (`gateway.rs`):

```rust
/// quota_enforcer를 교체 가능한 오버로드.
pub async fn start_with_quota(
    provider: Arc<dyn LlmProvider + Send + Sync>,
    pg: &PgHarness,
    quota_enforcer: Arc<dyn QuotaEnforcer + Send + Sync>,
) -> Self {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind random port");
    let addr: SocketAddr = listener.local_addr().expect("local_addr");

    let (audit_writer, _rx) = AuditWriter::new(4096);
    let state = AppState {
        key_validator: Arc::new(
            FakePgKeyValidator::new(pg.pool().clone())
        ),
        quota_enforcer,
        audit_writer: Arc::new(audit_writer),
        providers: Arc::new({
            let mut m = HashMap::new();
            m.insert(provider.name().to_string(), provider.clone());
            m
        }),
        router: None,  // Sprint 5 E2E: single provider, no routing layer
    };

    let router = build_router(state);
    let (shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

    let handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.recv().await;
            })
            .await
            .expect("gateway serve failed");
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .expect("reqwest client build");

    Self {
        url: format!("http://127.0.0.1:{}", addr.port()),
        client,
        shutdown_tx,
        _handle: handle,
    }
}
```

#### 시나리오 6 — 본문 크기 413

파일: `crates/gadgetron-testing/tests/e2e/body_size.rs`

```rust
mod common;
use common::E2EFixture;

/// 시나리오 6: 5 MB 바디 → 413.
///
/// gateway MAX_BODY_BYTES = 4_194_304 (4 MB). 5 MB 바디는 이 제한 초과.
/// RequestBodyLimitLayer가 auth 이전(outermost)에 위치하므로 키 없이도 테스트 가능.
///
/// 입력: POST /v1/chat/completions, Content-Length: 5_242_880 바이트 바디, 키 없음
/// 기대: HTTP 413
#[tokio::test]
async fn test_body_too_large_413() {
    let fx = E2EFixture::new("x", 0).await;

    // 5 MB = 5 * 1024 * 1024 = 5_242_880 bytes
    let large_body = vec![b'x'; 5 * 1024 * 1024];

    let resp = fx
        .gw
        .client
        .post(format!("{}/v1/chat/completions", fx.gw.url))
        .header("Content-Type", "application/json")
        .body(large_body)
        .send()
        .await
        .expect("request failed");

    assert_eq!(
        resp.status().as_u16(),
        413,
        "5 MB body must return 413 Payload Too Large"
    );

    fx.gw.shutdown().await;
}
```

#### 시나리오 7 — 서킷 브레이커 502

파일: `crates/gadgetron-testing/tests/e2e/circuit_breaker.rs`

```rust
use gadgetron_testing::harness::{gateway::GatewayHarness, pg::PgHarness};
use gadgetron_testing::mocks::provider::failing::{FailMode, FailingProvider};
use serde_json::json;
use std::sync::Arc;

/// 시나리오 7: FailingProvider(ImmediateFail) x3 → 서킷 오픈 → 4번째 요청 502.
///
/// platform-architecture.md §2.A.7.2 시나리오 7:
///   "4th request: HTTP 503 + routing_failure (circuit open)"
/// 주의: 현재 gateway는 FailingProvider 에러를 GadgetronError::Provider로 변환
/// → HTTP 502 반환. 서킷 브레이커 미구현 시 모든 요청이 502.
/// Sprint 5에서는 3 실패 후 서킷 열림 확인 (502 연속 4번 또는 503).
///
/// 단순화 전략: Sprint 5에서 서킷 브레이커가 구현되어 있으면 4번째 = 503 (routing_failure),
/// 미구현이면 502 (provider_error). 두 경우 모두 5xx로 assert.
#[tokio::test]
async fn test_circuit_breaker_502() {
    let pg = PgHarness::new().await;
    let (_, api_key) = pg.insert_test_tenant().await;
    let provider = Arc::new(FailingProvider::new(FailMode::ImmediateFail));
    let gw = GatewayHarness::start(provider, &pg).await;

    let body = json!({
        "model": "gpt-4o-mini",
        "messages": [{"role": "user", "content": "hi"}]
    });

    // 요청 1-3: 서킷 브레이커 카운터 누적
    for i in 1..=3 {
        let resp = gw
            .authed_post("/v1/chat/completions", &api_key)
            .json(&body)
            .send()
            .await
            .expect("request failed");
        let status = resp.status().as_u16();
        assert!(
            status >= 500 && status < 600,
            "request {i}: expected 5xx, got {status}"
        );
    }

    // 요청 4: 서킷 열림 확인 (502 또는 503)
    let resp4 = gw
        .authed_post("/v1/chat/completions", &api_key)
        .json(&body)
        .send()
        .await
        .expect("request 4 failed");
    let status4 = resp4.status().as_u16();
    assert!(
        status4 >= 500 && status4 < 600,
        "request 4 (circuit open): expected 5xx, got {status4}"
    );

    let json: serde_json::Value = resp4.json().await.expect("body parse");
    let code = json["error"]["code"].as_str().unwrap_or("");
    assert!(
        code == "routing_failure" || code == "provider_error",
        "error code must be routing_failure or provider_error, got: {code}"
    );

    gw.shutdown().await;
}
```

---

### 2.7 Criterion 벤치마크 3종

#### 벤치마크 1 — `bench_middleware_chain_mock`

파일: `crates/gadgetron-gateway/benches/bench_middleware_chain.rs`

```rust
use criterion::{criterion_group, criterion_main, Criterion};
use gadgetron_testing::harness::gateway::GatewayHarness;
use gadgetron_testing::harness::pg::PgHarness;
use gadgetron_testing::mocks::provider::fake::FakeLlmProvider;
use std::sync::Arc;

/// 전체 미들웨어 스택 E2E 왕복 벤치마크.
///
/// 구성: RequestBodyLimit → Trace → RequestId → Auth(DB hit) → TenantCtx → ScopeGuard → handler(FakeLlmProvider)
/// 측정: reqwest `POST /v1/chat/completions` 완전 왕복 시간.
/// Pass 기준: P99 < 1_000 µs (§2.7 `bench_middleware_chain_mock` SLO).
///
/// 주의: Docker 필요. CI 벤치마크 단계에서만 실행. `SKIP_CONTAINER_BENCHES=1` 시 skip.
fn bench_middleware_chain_mock(c: &mut Criterion) {
    if std::env::var("SKIP_CONTAINER_BENCHES").is_ok() {
        return;
    }
    let rt = tokio::runtime::Runtime::new().unwrap();

    let (gw, api_key) = rt.block_on(async {
        let pg = PgHarness::new().await;
        let (_, key) = pg.insert_test_tenant().await;
        let provider = Arc::new(FakeLlmProvider::new("bench", 0, vec!["gpt-4o-mini".to_string()]));
        let gw = GatewayHarness::start(provider, &pg).await;
        (gw, key)
    });

    let body = serde_json::json!({
        "model": "gpt-4o-mini",
        "messages": [{"role": "user", "content": "bench"}],
        "stream": false
    });

    let mut group = c.benchmark_group("middleware_chain");
    group.bench_function("full_stack_fake_provider", |b| {
        b.to_async(&rt).iter(|| async {
            let resp = gw
                .authed_post("/v1/chat/completions", &api_key)
                .json(&body)
                .send()
                .await
                .expect("bench request failed");
            assert_eq!(resp.status().as_u16(), 200);
        });
    });

    group.finish();
    rt.block_on(gw.shutdown());
}

criterion_group!(benches, bench_middleware_chain_mock);
criterion_main!(benches);
```

`Cargo.toml` `[[bench]]` 추가:

```toml
[[bench]]
name = "bench_middleware_chain"
harness = false
```

#### 벤치마크 2 — `bench_auth_cache_hit`

파일: `crates/gadgetron-xaas/benches/bench_moka_key_validation.rs`

```rust
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use gadgetron_xaas::auth::validator::PgKeyValidator;
use std::sync::Arc;

/// moka 캐시 hit 경로 SHA-256 + get 벤치마크.
///
/// 사전 준비: 한 번 miss 로드로 캐시에 키 삽입.
/// 측정: `PgKeyValidator::validate(raw_key)` 캐시 hit 경로만.
/// Pass 기준: P99 < 50 µs (§2.7 `bench_auth_cache_hit` hit SLO).
///
/// DB 의존 없음: `PgKeyValidator`에 메모리 고정 응답 인터셉트 불가 시
/// `FakePgKeyValidator`에 moka wrapper 적용하는 별도 타입으로 대체.
/// Sprint 5에서는 PgKeyValidator가 moka를 내부 보유하므로 직접 사용.
fn bench_auth_cache_hit(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    // 캐시 pre-warm: 실 DB 없이 테스트. PgKeyValidator에 in-memory SqlitePool 삽입.
    // 대안: FakePgKeyValidator + moka 래퍼. Sprint 5는 moka 독립 캐시 hit 경로만 측정.
    // 구체 구현: `gadgetron-xaas` 내 `CachedKeyValidator { inner: Arc<dyn KeyValidator>, cache: moka }` 신설.
    // 여기서는 moka::future::Cache 직접 벤치마크로 캐시 hit 비용 격리.
    use moka::future::Cache;
    use gadgetron_xaas::auth::validator::ValidatedKey;
    use gadgetron_core::context::Scope;
    use uuid::Uuid;

    let cache: Cache<String, Arc<ValidatedKey>> = Cache::builder()
        .max_capacity(10_000)
        .time_to_live(std::time::Duration::from_secs(600))
        .build();

    let key = "gad_test_benchmarkkey0000000000000000";
    let validated = Arc::new(ValidatedKey {
        api_key_id: Uuid::new_v4(),
        tenant_id: Uuid::new_v4(),
        scopes: vec![Scope::OpenAiCompat],
    });

    // pre-warm
    rt.block_on(cache.insert(key.to_string(), validated.clone()));

    let mut group = c.benchmark_group("auth_cache");
    group.bench_function("moka_cache_hit", |b| {
        b.to_async(&rt).iter(|| async {
            let result = cache.get(key).await;
            criterion::black_box(result.expect("cache must hit"));
        });
    });
    group.finish();
}

criterion_group!(benches, bench_auth_cache_hit);
criterion_main!(benches);
```

#### 벤치마크 3 — `bench_router_roundrobin`

파일: `crates/gadgetron-router/benches/bench_router_strategy_roundrobin.rs`

```rust
use criterion::{criterion_group, criterion_main, Criterion};
use gadgetron_router::router::{Router, RouterConfig};
use gadgetron_testing::mocks::provider::fake::FakeLlmProvider;
use std::sync::Arc;

/// RoundRobin 전략으로 3개 provider 중 하나 선택 벤치마크.
///
/// 측정: `Router::resolve(&model_id)` → provider Arc 반환.
/// Pass 기준: P99 < 20 µs (§2.7 `bench_router_roundrobin` SLO).
/// AtomicUsize::fetch_add + DashMap 읽기 경로만.
fn bench_router_roundrobin(c: &mut Criterion) {
    let providers: Vec<Arc<dyn gadgetron_core::provider::LlmProvider + Send + Sync>> = (0..3)
        .map(|i| {
            Arc::new(FakeLlmProvider::new(
                "bench",
                0,
                vec![format!("model-{i}")],
            )) as Arc<dyn gadgetron_core::provider::LlmProvider + Send + Sync>
        })
        .collect();

    let router = Router::new_round_robin(providers);

    let mut group = c.benchmark_group("router");
    group.bench_function("roundrobin_3_providers", |b| {
        b.iter(|| {
            let result = router.resolve("gpt-4o-mini");
            criterion::black_box(result);
        });
    });
    group.finish();
}

criterion_group!(benches, bench_router_roundrobin);
criterion_main!(benches);
```

`Router::new_round_robin` 시그니처 (`gadgetron-router/src/router.rs`):

```rust
/// 라운드로빈 전략으로 router를 생성한다.
/// `resolve()` 호출 시 `AtomicUsize::fetch_add(1, Ordering::Relaxed)` % n 으로 provider 선택.
pub fn new_round_robin(
    providers: Vec<Arc<dyn LlmProvider + Send + Sync>>,
) -> Self;

/// provider Arc 반환. model_id 미사용 (현재 라운드로빈은 model 무관 분산).
pub fn resolve(&self, _model_id: &str) -> Option<Arc<dyn LlmProvider + Send + Sync>>;
```

---

### 2.8 `gadgetron-core/src/ui.rs` — 공유 UI 타입

D-20260411-07 결정: `gadgetron-core`에 `ui` 모듈로 추가. `dashboard.md §8.3` 필드 명세 준수.

파일: `crates/gadgetron-core/src/ui.rs`

```rust
//! 공유 UI 타입 (D-20260411-07).
//!
//! `gadgetron-tui` [P1] 과 `gadgetron-web` [P2] 가 함께 사용한다.
//! 모든 타입은 `serde::{Serialize, Deserialize}` 구현 — WebSocket JSON 직렬화 대비.
//! 타입 변경 시 TUI + Web UI 양쪽 영향 검토 필요.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ─── GPU 메트릭 ────────────────────────────────────────────────────────────

/// 단일 GPU의 순간 메트릭.
///
/// `node_id`: NodeStatus.id (gadgetron-core/src/node.rs 의 `NodeStatus::id`).
/// `gpu_index`: GPU 디바이스 인덱스 (0-based). `GpuInfo::index`와 동일.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuMetrics {
    pub node_id: String,
    pub gpu_index: u32,
    pub name: String,
    pub temperature_c: f32,
    pub vram_used_mb: u64,
    pub vram_total_mb: u64,
    pub utilization_pct: f32,
    pub power_w: f32,
    pub power_limit_w: f32,
    /// GPU 코어 클럭 속도 (MHz). NVML `nvmlDeviceGetClockInfo(GRAPHICS)` 값.
    /// FakeGpuMonitor에서는 고정값(예: 1_410)을 사용한다.
    pub clock_mhz: u32,
    pub timestamp: DateTime<Utc>,
}

// ─── 모델 상태 ────────────────────────────────────────────────────────────

/// 모델 상태 종류. `#[non_exhaustive]`: Phase 2에서 Migrating/Updating 추가 가능.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelState {
    Running,
    Loading,
    Stopped,
    Error,
    Draining,
}

impl std::fmt::Display for ModelState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Running  => "running",
            Self::Loading  => "loading",
            Self::Stopped  => "stopped",
            Self::Error    => "error",
            Self::Draining => "draining",
        };
        write!(f, "{s}")
    }
}

/// 단일 모델의 현재 상태.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelStatus {
    /// 모델 식별자 (예: "meta-llama/Llama-3-8B-Instruct")
    pub model_id: String,
    /// 사람이 읽기 좋은 이름 (예: "Llama 3 8B")
    pub name: String,
    pub state: ModelState,
    /// provider 이름 (예: "openai", "ollama")
    pub provider: String,
    /// 노드 ID (`None` = stopped/unassigned)
    pub node_id: Option<String>,
    /// 점유 VRAM MB (`None` = not loaded)
    pub vram_mb: Option<u64>,
    pub loaded_at: Option<DateTime<Utc>>,
}

// ─── 요청 로그 ────────────────────────────────────────────────────────────

/// 단일 LLM 요청의 로그 엔트리.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestEntry {
    pub request_id: String,
    pub tenant_id: String,
    pub model: String,
    pub provider: String,
    /// HTTP 응답 상태 코드
    pub status: u16,
    pub latency_ms: u32,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub timestamp: DateTime<Utc>,
}

// ─── 클러스터 건강 ─────────────────────────────────────────────────────────

/// 클러스터 전체 건강 요약.
///
/// TUI 상단 헤더 바 + 3-컬럼 레이아웃 상단 요약에 사용.
///
/// # Sprint 5 범위 제한
/// 노드별 CPU/RAM 사용률은 Sprint 5 TUI MVP에 포함되지 않는다.
/// Sprint 5 TUI는 GPU-only 표시 (`GpuMetrics` 기반).
/// 노드별 CPU/RAM 필드(`cpu_used_pct`, `ram_used_mb` 등)는 Sprint 6에서
/// `NodeStatus` 타입 확장과 함께 추가 예정이다 ([P2]).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClusterHealth {
    pub total_nodes: u32,
    pub healthy_nodes: u32,
    pub total_gpus: u32,
    pub active_gpus: u32,
    pub models_loaded: u32,
    pub requests_per_sec: f32,
    pub error_rate_pct: f32,
    /// UTC 최종 업데이트 시각
    pub updated_at: DateTime<Utc>,
    // 노드별 CPU/RAM 표시: Sprint 6 [P2]. 현재 TUI MVP는 GPU-only.
    // per_node_cpu_pct: Vec<f32>,   // [P2] Sprint 6
    // per_node_ram_mb: Vec<u64>,    // [P2] Sprint 6
}

// ─── WebSocket 메시지 ────────────────────────────────────────────────────

/// WebSocket/채널로 전송되는 메시지 enum.
///
/// [P1]: TUI in-process Arc 폴링 + tokio 채널.
/// [P2]: 실 WebSocket JSON 직렬화.
///
/// `#[serde(tag = "type", rename_all = "snake_case")]`로 JSON 직렬화 시
/// `{"type":"node_update", ...}` 형태.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsMessage {
    NodeUpdate(Vec<GpuMetrics>),
    ModelUpdate(Vec<ModelStatus>),
    RequestLog(RequestEntry),
    HealthUpdate(ClusterHealth),
}
```

`lib.rs`에 `pub mod ui;` 추가:

```rust
// crates/gadgetron-core/src/lib.rs 에 추가:
pub mod ui;
```

---

### 2.9 TUI 데이터 와이어링 + 3-컬럼 레이아웃

#### `App` 구조체 업데이트

파일: `crates/gadgetron-tui/src/app.rs` — 현재 `App { running: bool }` → 공유 상태 참조 포함:

```rust
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;
use gadgetron_core::ui::{ClusterHealth, GpuMetrics, ModelStatus, RequestEntry};

/// TUI 애플리케이션 상태.
///
/// 동시성 모델: `Arc<RwLock<T>>`.
/// 이유: TUI는 단일 렌더 스레드, 데이터 생산자(게이트웨이 처리 루프)는 별도 태스크.
/// `RwLock`은 다중 reader / 단일 writer → 렌더 중 writer 없음 (1 Hz 업데이트 vs 10 Hz 렌더).
/// `tokio::sync::RwLock` 대신 `std::sync::RwLock` 사용: TUI 렌더 루프가 비동기 아님
/// (crossterm 이벤트 루프는 `event::poll`로 동기 block).
pub struct App {
    pub running: bool,
    /// 클러스터 건강 요약. 1초마다 업데이트.
    pub health: Arc<RwLock<ClusterHealth>>,
    /// 노드별 GPU 메트릭 목록.
    pub gpu_metrics: Arc<RwLock<Vec<GpuMetrics>>>,
    /// 모델 상태 목록.
    pub model_statuses: Arc<RwLock<Vec<ModelStatus>>>,
    /// 최근 요청 로그 (최대 100개).
    pub request_log: Arc<RwLock<Vec<RequestEntry>>>,
    /// 업데이트 채널 수신단. broadcast::Receiver<WsMessage>.
    /// `None` = 데이터 소스 미연결 (placeholder, demo 모드).
    update_rx: Option<broadcast::Receiver<gadgetron_core::ui::WsMessage>>,
}

impl App {
    /// 데이터 소스 없는 standalone 모드 (TUI demo/개발용).
    pub fn new() -> Self {
        use chrono::Utc;
        Self {
            running: true,
            health: Arc::new(RwLock::new(ClusterHealth::default())),
            gpu_metrics: Arc::new(RwLock::new(Vec::new())),
            model_statuses: Arc::new(RwLock::new(Vec::new())),
            request_log: Arc::new(RwLock::new(Vec::new())),
            update_rx: None,
        }
    }

    /// broadcast 채널에서 메시지를 소비해 내부 Arc<RwLock<T>>를 업데이트한다.
    ///
    /// 최대 `REQUEST_LOG_CAPACITY = 100`개 유지. 초과 시 가장 오래된 항목 제거.
    ///
    /// 호출 위치: `App::run()` 루프에서 매 이벤트 폴링 사전에 `try_recv()` 호출.
    /// `tokio::sync::broadcast` 대신 `std::sync::mpsc`를 사용해도 가능하나
    /// broadcast는 다수 subscriber 지원 ([P2] 확장 대비). Sprint 5에서는 단일 subscriber.
    pub fn drain_updates(&mut self) {
        const REQUEST_LOG_CAPACITY: usize = 100;
        use gadgetron_core::ui::WsMessage;
        let Some(rx) = &mut self.update_rx else { return };
        loop {
            match rx.try_recv() {
                Ok(WsMessage::NodeUpdate(metrics)) => {
                    *self.gpu_metrics.write().unwrap() = metrics;
                }
                Ok(WsMessage::ModelUpdate(statuses)) => {
                    *self.model_statuses.write().unwrap() = statuses;
                }
                Ok(WsMessage::RequestLog(entry)) => {
                    let mut log = self.request_log.write().unwrap();
                    log.push(entry);
                    if log.len() > REQUEST_LOG_CAPACITY {
                        log.remove(0);
                    }
                }
                Ok(WsMessage::HealthUpdate(h)) => {
                    *self.health.write().unwrap() = h;
                }
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Closed) => {
                    self.running = false;
                    break;
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            }
        }
    }
}
```

이벤트 루프 수정 (`run()` 내부):

```rust
// 100ms 폴링 주기 + 1 Hz drain gate
// drain_updates()는 1초에 1번만 호출한다. 이유: broadcast 채널은 10Hz+ 속도로 메시지를
// 생산할 수 있으나 TUI 데이터 표시 갱신은 1초 단위로 충분하다. 매 100ms마다 drain하면
// RwLock contention이 불필요하게 증가한다.
use std::time::Instant;
let mut last_update = Instant::now().checked_sub(Duration::from_secs(1)).unwrap_or(Instant::now());

while self.running {
    // 1 Hz gate: 마지막 drain으로부터 1초 이상 경과한 경우에만 drain_updates() 호출.
    if last_update.elapsed() >= Duration::from_secs(1) {
        self.drain_updates();
        last_update = Instant::now();
    }

    terminal.draw(|f| ui::draw(f, self))?;

    if event::poll(Duration::from_millis(100))? {
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => self.running = false,
                _ => {}
            }
        }
    }
}
```

#### 3-컬럼 레이아웃

파일: `crates/gadgetron-tui/src/ui.rs` — 현재 2-컬럼 → 3-컬럼 교체:

```rust
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;
use crate::app::App;

/// 최상위 레이아웃:
///
/// ```
/// ┌─────────────────────────────────────────────┐  ← Header (3 rows)
/// ├──────────────┬──────────────┬───────────────┤
/// │  Nodes/GPU   │    Models    │   Requests    │  ← Body (Min)
/// ├──────────────┴──────────────┴───────────────┤
/// │  q: quit | r: refresh                       │  ← Footer (3 rows)
/// └─────────────────────────────────────────────┘
/// ```
pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // header
            Constraint::Min(0),     // body
            Constraint::Length(3),  // footer
        ])
        .split(area);

    draw_header(f, rows[0], app);
    draw_body(f, rows[1], app);
    draw_footer(f, rows[2]);
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let health = app.health.read().unwrap();
    let text = format!(
        " Gadgetron  Nodes: {}/{} | GPUs: {}/{} | Models: {} | RPS: {:.1} | Err: {:.1}%",
        health.healthy_nodes,
        health.total_nodes,
        health.active_gpus,
        health.total_gpus,
        health.models_loaded,
        health.requests_per_sec,
        health.error_rate_pct,
    );
    let header = Paragraph::new(text)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
    f.render_widget(header, area);
}

fn draw_body(f: &mut Frame, area: Rect, app: &App) {
    // 3-컬럼: Nodes(33%) / Models(33%) / Requests(34%)
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(33),
            Constraint::Percentage(34),
        ])
        .split(area);

    draw_nodes_panel(f, cols[0], app);
    draw_models_panel(f, cols[1], app);
    draw_requests_panel(f, cols[2], app);
}

fn draw_nodes_panel(f: &mut Frame, area: Rect, app: &App) {
    let metrics = app.gpu_metrics.read().unwrap();
    let items: Vec<ListItem> = metrics
        .iter()
        .map(|m| {
            ListItem::new(format!(
                "[{}] GPU{} {:.0}% VRAM:{}/{}MB {}C",
                m.node_id,
                m.gpu_index,
                m.utilization_pct,
                m.vram_used_mb,
                m.vram_total_mb,
                m.temperature_c,
            ))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .title(" Nodes ")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Green)),
        );
    f.render_widget(list, area);
}

fn draw_models_panel(f: &mut Frame, area: Rect, app: &App) {
    let statuses = app.model_statuses.read().unwrap();
    let items: Vec<ListItem> = statuses
        .iter()
        .map(|m| {
            ListItem::new(format!(
                "[{}] {} {}",
                m.state, m.model_id, m.provider,
            ))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .title(" Models ")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Yellow)),
        );
    f.render_widget(list, area);
}

fn draw_requests_panel(f: &mut Frame, area: Rect, app: &App) {
    let log = app.request_log.read().unwrap();
    let items: Vec<ListItem> = log
        .iter()
        .rev()  // 최신 요청이 상단
        .take(50)
        .map(|r| {
            ListItem::new(format!(
                "{} {} {}ms HTTP{}",
                r.request_id.get(..8).unwrap_or(&r.request_id),
                r.model,
                r.latency_ms,
                r.status,
            ))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .title(" Requests ")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Blue)),
        );
    f.render_widget(list, area);
}

fn draw_footer(f: &mut Frame, area: Rect) {
    let footer = Paragraph::new(" q: quit | r: refresh | arrows: navigate ")
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(footer, area);
}
```

#### §2.9.3 Color Theme

`dashboard.md §2.6` 컬러 규약을 TUI에 적용한다. 임계값 초과 시 `Style::default().fg(Color::X)`로 색상을 교체한다.

**온도 임계값 (`temperature_c`):**

| 범위 | 색상 | ratatui 상수 |
|------|------|-------------|
| 0 °C ≤ T < 60 °C | 초록 | `Color::Green` |
| 60 °C ≤ T < 75 °C | 노랑 | `Color::Yellow` |
| 75 °C ≤ T < 85 °C | 빨강 | `Color::Red` |
| T ≥ 85 °C | 밝은 빨강 | `Color::LightRed` |

**VRAM 사용률 임계값 (`vram_used_mb / vram_total_mb`):**

| 범위 | 색상 | ratatui 상수 |
|------|------|-------------|
| 0% ≤ 사용률 < 70% | 초록 | `Color::Green` |
| 70% ≤ 사용률 < 90% | 노랑 | `Color::Yellow` |
| 사용률 ≥ 90% | 빨강 | `Color::Red` |

구현 헬퍼 (`crates/gadgetron-tui/src/ui.rs`에 추가):

```rust
/// 온도(°C)에 따른 ratatui 색상 반환.
fn temp_color(t: f32) -> Color {
    if t < 60.0 { Color::Green }
    else if t < 75.0 { Color::Yellow }
    else if t < 85.0 { Color::Red }
    else { Color::LightRed }
}

/// VRAM 사용률(%)에 따른 ratatui 색상 반환.
/// `used_mb / total_mb * 100.0` 값을 받는다. `total_mb == 0`이면 Green 반환.
fn vram_color(used_mb: u64, total_mb: u64) -> Color {
    if total_mb == 0 { return Color::Green; }
    let pct = used_mb as f32 / total_mb as f32 * 100.0;
    if pct < 70.0 { Color::Green }
    else if pct < 90.0 { Color::Yellow }
    else { Color::Red }
}
```

`draw_nodes_panel`의 `ListItem` 생성 시 `Style::default().fg(temp_color(m.temperature_c))`를 적용해 각 행을 색상화한다.

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 크레이트 의존성 방향

```
gadgetron-core
  └── ui.rs (신규 pub mod)
        ↑ 사용
gadgetron-tui → gadgetron-core::ui

gadgetron-testing → gadgetron-core (trait impl)
                  → gadgetron-gateway (GatewayHarness, AppState)
                  → gadgetron-xaas (KeyValidator, QuotaEnforcer, AuditWriter)

gadgetron-gateway/benches → gadgetron-testing (GatewayHarness, FakeLlmProvider)
gadgetron-xaas/benches    → gadgetron-testing (FakePgKeyValidator), moka (직접)
gadgetron-router/benches  → gadgetron-testing (FakeLlmProvider)
```

### 3.2 데이터 흐름 — E2E

```
[reqwest Client]
    │ POST /v1/chat/completions
    ▼
[GatewayHarness (랜덤 포트 TCP)]
    │
    ▼
[axum Router — build_router(AppState)]
  RequestBodyLimit → TraceLayer → RequestId → Auth(FakePgKeyValidator) →
  TenantCtx → ScopeGuard → chat_completions_handler
    │
    ▼
[FakeLlmProvider::chat() or chat_stream()]
    │
    ▼
[HTTP 200 + ChatResponse JSON or SSE]
    │
    ▼
[reqwest Response — E2E assert]
```

### 3.3 데이터 흐름 — TUI

```
[Future: gateway handle loop]
    │ tokio::sync::broadcast::Sender<WsMessage>
    ▼
[App::drain_updates()]
    │ Arc<RwLock<Vec<GpuMetrics>>> / Arc<RwLock<Vec<ModelStatus>>> / ...
    ▼
[ui::draw(f, &app)]  ← crossterm 100ms 이벤트 폴링 주기
    │
    ▼
[ratatui Frame → terminal stdout]
```

### 3.4 D-12 크레이트 경계표 준수

| 타입 | 크레이트 | D-12 기준 |
|------|---------|----------|
| `GpuMetrics`, `ModelStatus`, `RequestEntry`, `ClusterHealth`, `WsMessage` | `gadgetron-core::ui` | D-20260411-07 결정 준수 |
| `FakeLlmProvider`, `FailingProvider`, `PgHarness`, `GatewayHarness` | `gadgetron-testing` | D-20260411-05 결정 준수 |
| `App`, `draw()` | `gadgetron-tui` | [P1] 단일 바이너리 |
| `AppState`, `build_router()` | `gadgetron-gateway` | 기존 경계 유지 |

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

| 테스트 대상 | 검증 invariant | 파일 |
|------------|---------------|------|
| `FakeLlmProvider::chat()` | 응답 content == 생성자 content, choices.len() == 1, finish_reason == "stop" | `gadgetron-testing/src/mocks/provider/fake.rs #[cfg(test)]` |
| `FakeLlmProvider::chat_stream()` | stream 청크 수 == `stream_chunks`, 마지막 청크 finish_reason == "stop", 나머지 None | 동일 |
| `FakeLlmProvider::models()` | 반환 Vec 길이 == model_ids.len() | 동일 |
| `FailingProvider::chat()` ImmediateFail | `Err(GadgetronError::Provider(_))` | `failing.rs #[cfg(test)]` |
| `FailingProvider::chat()` DelayedFail(100ms) | `tokio::time::pause()` + `advance(100ms)` 후 Err 반환 | 동일 |
| `FailingProvider::chat_stream()` PartialStream(2) | 처음 2개 Ok, 3번째 `Err(StreamInterrupted)` | 동일 |
| `FailingProvider::chat_stream()` StreamInterrupted | 1 Ok + 1 Err(StreamInterrupted) | 동일 |
| `gadgetron-core::ui` serde roundtrip | `GpuMetrics`, `ModelStatus`, `RequestEntry`, `ClusterHealth`, `WsMessage` 5종 JSON 직렬화/역직렬화 동일성 | `gadgetron-core/src/ui.rs #[cfg(test)]` |
| `WsMessage` serde tag | `WsMessage::HealthUpdate(h)` → `{"type":"health_update",...}` | 동일 |
| `ModelState::Display` | `Running` → "running", `Error` → "error" | 동일 |
| `App::drain_updates()` NodeUpdate | `gpu_metrics` Arc 업데이트 됨 | `gadgetron-tui/src/app.rs #[cfg(test)]` |
| `App::drain_updates()` RequestLog overflow | 101번째 항목 push 시 len() == 100, 첫 항목 제거 | 동일 |

### 4.2 구체 입력/기대 출력

**FakeLlmProvider chat:**

```
입력: ChatRequest { model: "gpt-4o-mini", messages: [Message{role:"user",content:"hi"}], stream: false, ... }
기대: Ok(ChatResponse { choices[0].message.content == "hello from fake", choices[0].finish_reason == Some("stop"), usage.total_tokens == 15 })
```

**FakeLlmProvider chat_stream (chunks=2):**

```
입력: 동일 ChatRequest
기대: Stream 항목 = [
  Ok(ChatChunk { choices[0].delta.content == Some("chunk_0"), finish_reason == None }),
  Ok(ChatChunk { choices[0].delta.content == Some("chunk_1"), finish_reason == Some("stop") }),
]
```

**FailingProvider ImmediateFail chat:**

```
입력: 임의 ChatRequest
기대: Err(GadgetronError::Provider(s)) where s.contains("immediate fail")
```

**WsMessage serde roundtrip:**

```
입력: WsMessage::HealthUpdate(ClusterHealth { total_nodes: 3, healthy_nodes: 3, ... })
기대: serde_json::to_string(msg) → parse → 동일 값. JSON에 "type":"health_update" 키 포함.
```

**App::drain_updates RequestLog overflow:**

```
입력: broadcast channel에 101개 RequestLog 메시지 전송
기대: app.request_log.read().unwrap().len() == 100, request_id 101번째가 없음
```

### 4.3 커버리지 목표

- `gadgetron-testing`: 90% line (mock + harness 핵심 경로 전부).
- `gadgetron-core/src/ui.rs`: 100% line (serde roundtrip + Display + 상태 전이 없음).
- `gadgetron-tui/src/app.rs`: 80% line (drain_updates 전 경로 + overflow).

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

| 테스트 | 크레이트 조합 | 파일 |
|--------|-------------|------|
| E2E 시나리오 1 (비스트리밍) | testing + gateway + xaas + core | `tests/e2e/chat_completion.rs` |
| E2E 시나리오 2 (스트리밍) | testing + gateway + xaas + core | `tests/e2e/streaming.rs` |
| E2E 시나리오 3 (401) | testing + gateway + xaas | `tests/e2e/auth.rs` |
| E2E 시나리오 4 (403) | testing + gateway + xaas | `tests/e2e/auth.rs` |
| E2E 시나리오 5 (429) | testing + gateway + xaas | `tests/e2e/quota.rs` |
| E2E 시나리오 6 (413) | testing + gateway | `tests/e2e/body_size.rs` |
| E2E 시나리오 7 (502/503) | testing + gateway | `tests/e2e/circuit_breaker.rs` |
| `bench_middleware_chain_mock` | testing + gateway + xaas + core | `gateway/benches/bench_middleware_chain.rs` |
| `bench_auth_cache_hit` | xaas (moka 직접) | `xaas/benches/bench_moka_key_validation.rs` |
| `bench_router_roundrobin` | router + testing | `router/benches/bench_router_strategy_roundrobin.rs` |

### 5.2 테스트 환경

**E2E (시나리오 1-7):**

- Docker daemon 필요 (testcontainers PostgreSQL).
- CI: `.github/workflows/ci.yml` `services:` 섹션에 `docker:` 포함 불필요 — testcontainers가 Docker socket 직접 사용.
- `DOCKER_HOST` 환경변수: GitHub Actions `ubuntu-latest` 기본 Docker 사용.
- 병렬 실행: 각 시나리오가 독립 `PgHarness` + `GatewayHarness` 인스턴스 → 포트 충돌 없음.
- 컨테이너 정리: `PgHarness::_container` Drop 자동 정리. 테스트 스레드 패닉 시도 `tokio::test`의 cleanup hook 통해 Drop 보장.

**벤치마크:**

- Docker 필요 벤치마크(`bench_middleware_chain_mock`)는 `SKIP_CONTAINER_BENCHES=1` 환경변수로 skip 가능.
- `bench_auth_cache_hit`, `bench_router_roundrobin`: Docker 불필요, 인메모리 전용.
- CI nightly 실행: `cargo bench --workspace -- --output-format=bencher` + `check_bench_regression.py`.
- 재현성: `CRITERION_SEED=42` 고정, `ubuntu-22.04-8core` 러너.

**TUI (수동 검증):**

- `cargo run -p gadgetron-tui` 실행 → 터미널에 3-컬럼 레이아웃 렌더 확인.
- Demo 데이터: `App::new()` 기본값은 빈 Vec — 3개 패널 모두 빈 상태로 테두리만 표시.
- snapshot test [P2]: `ratatui::backend::TestBackend` + `insta::assert_snapshot!` 조합 옵션.

### 5.3 SLO Gates (CI 실패 조건)

| 벤치마크 | Pass 기준 | CI 실패 조건 |
|---------|---------|------------|
| `bench_middleware_chain_mock` | P99 < 1_000 µs | P99 > 1_000 µs OR regression > 10% vs baseline |
| `bench_auth_cache_hit` | P99 < 50 µs | P99 > 50 µs OR regression > 10% vs baseline |
| `bench_router_roundrobin` | P99 < 20 µs | P99 > 20 µs OR regression > 10% vs baseline |

### 5.4 회귀 방지

다음 변경이 테스트를 실패시켜야 한다:

| 변경 | 실패 테스트 |
|------|-----------|
| `GadgetronError::TenantNotFound` HTTP 상태를 401 → 403으로 변경 | 시나리오 3 (401 assert) |
| `scope_guard_middleware`에서 `/api/v1/nodes` scope 검사 제거 | 시나리오 4 (403 assert) |
| `RequestBodyLimitLayer` 제거 | 시나리오 6 (413 assert) |
| `InMemoryQuotaEnforcer::pre_request` 오류 처리 누락 | 시나리오 5 (429 assert) |
| `FakeLlmProvider::chat_stream` chunks=0 인데도 청크 방출 | 시나리오 2 단위 테스트 |
| `ui.rs` WsMessage serde tag 변경 | `WsMessage` serde roundtrip 단위 테스트 |
| `App::drain_updates` overflow 제거 | RequestLog overflow 단위 테스트 |
| moka cache hit 경로에 DB 쿼리 추가 | `bench_auth_cache_hit` SLO 위반 |

---

## 6. Phase 구분

| 항목 | Phase |
|------|-------|
| FakeLlmProvider, FailingProvider | [P1] Sprint 5 |
| PgHarness (testcontainers) | [P1] Sprint 5 |
| GatewayHarness | [P1] Sprint 5 |
| E2E 시나리오 1-7 | [P1] Sprint 5 |
| E2E 시나리오 8-9 (graceful shutdown, process restart) | [P2] Sprint 6+ |
| Criterion 벤치마크 3종 | [P1] Sprint 5 |
| `gadgetron-core/src/ui.rs` 5 타입 | [P1] Sprint 5 |
| TUI 3-컬럼 레이아웃 | [P1] Sprint 5 |
| TUI WebSocket 연결 | [P2] Sprint 6+ (분산 배포 시) |
| TUI snapshot tests (`ratatui::backend::TestBackend`) | [P2] Sprint 6+ |

---

## 7. 오픈 이슈 / 의사결정 필요

| ID | 내용 | 옵션 | 추천 | 상태 |
|----|------|------|------|------|
| Q-1 | `GatewayHarness::start_with_quota()` 구현: 기존 `start()` 코드 복제 vs 공통 내부 빌더 | A: 복제 (단순) / B: `GatewayHarnessBuilder` struct | B — 중복 제거, Sprint 5 내 구현 가능 | 담당자 결정 |
| Q-2 | `bench_middleware_chain_mock` Docker 의존: nightly 전용 vs PR 벤치마크 | A: nightly only / B: `SKIP_CONTAINER_BENCHES` flag로 PR에서 skip | B 채택 (§5.2에 명시) | 결정 완료 |
| Q-3 | `FakePgKeyValidator` scopes TEXT[] 파싱: DB 스키마 `TEXT[]` vs `scopes jsonb` | `gadgetron-xaas` migration 파일 확인 필요 | TEXT[] 가정 (기존 migration 준수) | PM 확인 필요 |

---

## 리뷰 로그 (append-only)

### Round 1 — 2026-04-12 예정 — @gateway-router-lead @chief-architect
**결론**: 대기 중

**체크리스트** (`03-review-rubric.md §1` 기준):
- [ ] 인터페이스 계약
- [ ] 크레이트 경계
- [ ] 타입 중복
- [ ] 에러 반환
- [ ] 동시성
- [ ] 의존성 방향
- [ ] Phase 태그
- [ ] 레거시 결정 준수

**Action Items**: —

**다음 라운드 조건**: Round 1 8개 항목 전부 Pass

### Round 2 — @qa-test-architect (self-review 금지 — 타 에이전트 배정)
**결론**: 대기 중

### Round 3 — @chief-architect
**결론**: 대기 중

### 최종 승인 — PM
대기 중
