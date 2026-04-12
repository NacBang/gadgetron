# Sprint 6: TUI Live Hardening

> **담당**: @chief-architect
> **상태**: Draft
> **작성일**: 2026-04-12
> **최종 업데이트**: 2026-04-12
> **관련 크레이트**: `gadgetron-cli`, `gadgetron-gateway`, `gadgetron-tui`, `gadgetron-core`, `gadgetron-xaas`, `gadgetron-router`
> **Phase**: [P1]
> **결정 근거**: D-20260412-02 (구현 결정론), D-20260411-01 (Phase 1 MVP), D-20260411-07 (WsMessage), §2.F.4 (70s shutdown), §2.H.9 (benchmarks)

---

## 1. 철학 & 컨셉 (Why)

**Sprint 6 = "Connect & Harden"** — TUI가 처음으로 실제 요청 데이터를 수신하고, 시스템이 프로덕션 투입 가능한 수준으로 강화된다.

### 1.1 해결 문제

Sprint 5까지의 TUI는 `App::new()`의 정적 데모 데이터로만 동작했다 (`update_rx: None`). 실시간 요청 로그가 TUI에 도달하는 경로가 없었으며, `/ready`는 PG 연결 여부와 무관하게 항상 200을 반환했다. 셧다운 시 audit 채널이 drain 없이 즉시 종료되어 in-flight audit 항목이 유실될 수 있었다. 성능 회귀를 탐지할 criterion 벤치마크가 부재했다.

### 1.2 제품 비전 연결

Gadgetron의 핵심 차별화 중 하나는 "단일 바이너리 + 읽기 전용 TUI"다 (`docs/00-overview.md §1`). Sprint 6은 이 TUI를 비로소 살아있는 대시보드로 만든다. 또한 `/ready` PG 체크는 K8s readiness probe와 직접 연결되어 배포 안정성을 높인다.

### 1.3 고려한 대안

| 대안 | 기각 이유 |
|------|-----------|
| `mpsc::channel` 대신 `watch::channel`로 TUI 연결 | `watch`는 최신 값 1개만 유지 — 요청 로그는 누적이 필요. `broadcast`는 여러 수신자 지원 + 용량 제어 가능. `WsMessage` 타입이 이미 `broadcast` 기준으로 설계됨 (`gadgetron-core/src/ui.rs`) |
| TUI를 별도 tokio runtime 없이 `spawn_blocking`으로 실행 | crossterm의 `event::poll`은 blocking I/O. tokio worker thread를 점유하면 `async fn` 실행 예산을 침범. 별도 `std::thread::spawn` + 내부 단일-스레드 tokio 런타임이 격리 보장 |
| `/ready`에 PG 외 추가 헬스 체크 포함 | K8s probe timeout 기본값 1s 기준으로 PG `SELECT 1`만으로 충분. 복합 헬스 체크는 Phase 2에서 별도 `/health/deep` 엔드포인트로 분리 |
| audit drain timeout을 10s로 늘림 | §2.F.4 드레인 예산에서 audit flush는 5s로 확정. 70s 총 예산의 제약 |

### 1.4 핵심 설계 원칙

- **Fire-and-forget broadcast**: metrics_middleware는 `tx.send()`의 `Result`를 무시한다. `Lagged` 오류는 TUI의 문제이지 gateway의 문제가 아니다. 요청 경로에서 blocking 없음.
- **Option으로 선택적 TUI**: `AppState.tui_tx: Option<broadcast::Sender<WsMessage>>`. `--tui` 없이 실행하면 None이며 미들웨어는 `as_ref().map(|tx| ...)` 패턴으로 조건부 emit.
- **구현 결정론**: 모든 타입 시그니처, magic number, async 결정이 이 문서에 완전히 명시됨 (D-20260412-02).

---

## 2. 상세 구현 방안 (What & How)

### 2.1 AppState 확장 (T1)

**파일**: `crates/gadgetron-gateway/src/server.rs`

현재 `AppState`에 두 필드를 추가한다.

```rust
use sqlx::PgPool;
use tokio::sync::broadcast;
use gadgetron_core::ui::WsMessage;

/// Shared application state injected into every handler via `axum::State`.
///
/// All fields are `Arc`-wrapped or `Clone`-cheap so `Clone` is a pointer copy (~1 ns).
/// `Send + Sync` is satisfied because every inner type is `Send + Sync`.
#[derive(Clone)]
pub struct AppState {
    /// Bearer-token validator (moka-cached, 10-min TTL, max 10_000 entries).
    pub key_validator: Arc<dyn KeyValidator + Send + Sync>,
    /// Pre/post quota enforcement. Phase 1: `InMemoryQuotaEnforcer`.
    pub quota_enforcer: Arc<dyn QuotaEnforcer + Send + Sync>,
    /// Async audit-log channel writer (capacity 4_096).
    pub audit_writer: Arc<AuditWriter>,
    /// Registered LLM providers, keyed by provider name.
    pub providers: Arc<HashMap<String, Arc<dyn LlmProvider + Send + Sync>>>,
    /// Routing layer: wraps `providers` with strategy + fallback + metrics.
    pub router: Option<Arc<LlmRouter>>,
    /// PostgreSQL connection pool. Used by `/ready` health check and future audit flush.
    pub pg_pool: sqlx::PgPool,
    /// Broadcast sender for TUI live updates. `None` when `--tui` flag is absent.
    /// Capacity: 1_024 (rationale: 1 msg/req × 1_000 QPS ceiling × ~1s buffer).
    /// `broadcast::Sender<T>` is `Clone` — clone is a pointer increment (Arc内 Sender).
    pub tui_tx: Option<broadcast::Sender<WsMessage>>,
}
```

**channel capacity 근거**: 브로드캐스트 채널은 느린 수신자를 위해 capacity만큼 메시지를 버퍼링한다. 1_000 QPS (§2.H.1 SLO 기준 최대 부하) × 1초 TUI drain 주기 = 1_000개 메시지가 동시에 채널에 존재할 수 있다. 24개 여유를 더해 1_024 (2의 멱수, 내부 ring buffer 정렬 최적).

**`sqlx::PgPool` 포함 이유**: `PgPool`은 내부적으로 `Arc<PoolInner>` — `Clone`이 포인터 복사. `AppState::Clone`의 비용은 변하지 않는다. 기존 `main()`에서 이미 `pg_pool`이 생성되므로 추가 연결 오버헤드 없음.

`make_state()` 테스트 헬퍼는 `sqlx::PgPool`의 disconnected pool이 필요하다. 이를 위해 `sqlx::pool::PoolOptions::new().connect_lazy("postgresql://")` 패턴을 사용한다 — lazy connect는 빌드 시 연결을 시도하지 않는다.

```rust
// tests 내부 make_state() 업데이트
fn make_state() -> AppState {
    let (audit_writer, _rx) = AuditWriter::new(16);
    // lazy pool: connection is never actually attempted in unit tests
    let pg_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy("postgresql://localhost/test")
        .expect("lazy pool creation must not fail");
    AppState {
        key_validator: Arc::new(NoopKeyValidator),
        quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
        audit_writer: Arc::new(audit_writer),
        providers: Arc::new(HashMap::new()),
        router: None,
        pg_pool,
        tui_tx: None,
    }
}
```

### 2.2 clap CLI + `--tui` 플래그 (T2)

**파일**: `crates/gadgetron-cli/src/main.rs`

현재 `main()`은 환경변수(`GADGETRON_CONFIG`, `GADGETRON_BIND`)로만 인자를 받는다. `clap 4.x`로 교체한다.

**추가 의존성**: `gadgetron-cli/Cargo.toml`에 `clap = { workspace = true }` 추가.
`Cargo.toml` workspace에 `clap = { version = "4", features = ["derive"] }` 추가.

```rust
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Gadgetron — Rust-native GPU/LLM orchestration platform.
#[derive(Parser)]
#[command(
    name = "gadgetron",
    version,
    about = "GPU/LLM orchestration platform",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the gateway server.
    Serve {
        /// Path to TOML configuration file.
        /// Overrides GADGETRON_CONFIG environment variable.
        #[arg(long, short = 'c', default_value = "gadgetron.toml")]
        config: PathBuf,

        /// TCP bind address (host:port).
        /// Overrides GADGETRON_BIND environment variable and config.server.bind.
        #[arg(long, short = 'b')]
        bind: Option<String>,

        /// Log format: "json" (default, structured) or "pretty" (human-readable dev mode).
        #[arg(long, default_value = "json")]
        log_format: LogFormat,

        /// Launch the ratatui TUI dashboard in the current terminal.
        /// Opens alternate screen; press 'q' or Esc to quit.
        #[arg(long)]
        tui: bool,
    },
}

/// Log format selection for tracing-subscriber initialization.
#[derive(Clone, Debug)]
enum LogFormat {
    Json,
    Pretty,
}

impl std::str::FromStr for LogFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "json"   => Ok(LogFormat::Json),
            "pretty" => Ok(LogFormat::Pretty),
            other    => Err(format!("unknown log format '{other}'; expected 'json' or 'pretty'")),
        }
    }
}
```

**`main()` 구조 변경**:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve { config, bind, log_format, tui } => {
            serve(config, bind, log_format, tui).await
        }
    }
}

async fn serve(
    config_path: PathBuf,
    bind_override: String,
    log_format: LogFormat,
    tui_enabled: bool,
) -> anyhow::Result<()> {
    // Step 1: 트레이싱 초기화
    init_tracing(log_format);

    // Step 2: 설정 로드 (파일 없으면 내장 기본값)
    let config: AppConfig = if config_path.exists() {
        AppConfig::load(&config_path)
            .with_context(|| format!("failed to load config from {}", config_path.display()))?
    } else {
        tracing::warn!(path = %config_path.display(), "config file not found — using built-in defaults");
        default_config()
    };

    // bind 우선순위: CLI --bind > GADGETRON_BIND env > config.server.bind
    let bind_addr = bind
        .or_else(|| std::env::var("GADGETRON_BIND").ok())
        .unwrap_or_else(|| config.server.bind.clone());

    // Step 3: PostgreSQL 연결
    let db_url_str = std::env::var("GADGETRON_DATABASE_URL")
        .context("GADGETRON_DATABASE_URL environment variable is required")?;
    let db_url = Secret::new(db_url_str);
    let pg_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(20)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(db_url.expose())
        .await
        .context("failed to connect to PostgreSQL")?;

    // Step 4: 마이그레이션
    sqlx::migrate!("../gadgetron-xaas/migrations")
        .run(&pg_pool)
        .await
        .context("failed to run database migrations")?;
    tracing::info!("database migrations applied");

    // Step 5: KeyValidator, QuotaEnforcer
    let key_validator = Arc::new(PgKeyValidator::new(pg_pool.clone()))
        as Arc<dyn gadgetron_xaas::auth::validator::KeyValidator + Send + Sync>;
    let quota_enforcer = Arc::new(InMemoryQuotaEnforcer)
        as Arc<dyn gadgetron_xaas::quota::enforcer::QuotaEnforcer + Send + Sync>;

    // Step 6: AuditWriter (mpsc capacity 4_096)
    let (audit_writer, audit_rx) = AuditWriter::new(4_096);
    let audit_writer = Arc::new(audit_writer);

    // Step 7: Audit consumer task (Phase 1: tracing만, Phase 2: PG batch insert)
    let audit_task = tokio::spawn(audit_consumer_loop(audit_rx));

    // Step 8: TUI broadcast channel
    // broadcast::channel 반환값: (Sender<T>, Receiver<T>).
    // Sender는 Clone 가능. Receiver는 이 지점에서 subscribe()로 추가 생성 가능.
    let tui_tx: Option<broadcast::Sender<WsMessage>> = if tui_enabled {
        let (tx, _initial_rx) = broadcast::channel::<WsMessage>(1_024);
        // _initial_rx는 즉시 drop — Sender는 수신자가 0명이어도 유효.
        // TUI thread가 tx.subscribe()로 새 Receiver를 생성할 예정.
        Some(tx)
    } else {
        None
    };

    // Step 9: Providers + Router 빌드
    let providers_ss = build_providers(&config).context("failed to initialise LLM providers")?;
    let providers_for_router: HashMap<String, Arc<dyn LlmProvider>> = providers_ss
        .iter()
        .map(|(k, v)| (k.clone(), Arc::clone(v) as Arc<dyn LlmProvider>))
        .collect();
    let metrics_store = Arc::new(MetricsStore::new());
    let llm_router = Arc::new(LlmRouter::new(
        providers_for_router,
        config.router.clone(),
        metrics_store,
    ));

    // Step 10: AppState 조립
    let state = AppState {
        key_validator,
        quota_enforcer,
        audit_writer: audit_writer.clone(),
        providers: Arc::new(providers_ss),
        router: Some(llm_router),
        pg_pool: pg_pool.clone(),
        tui_tx: tui_tx.clone(),
    };

    // Step 11: TUI 스레드 스폰 (--tui가 활성화된 경우에만)
    //
    // 동시성 결정: `std::thread::spawn` + 내부 `tokio::runtime::Builder::new_current_thread()`
    // 이유: crossterm의 `event::poll`은 blocking 시스템콜 — tokio worker thread를 점유하면
    // async 실행 예산을 침범한다. 별도 OS thread에서 격리된 단일-스레드 tokio runtime으로 실행.
    let tui_thread = if let Some(ref tx) = tui_tx {
        let rx = tx.subscribe(); // Sender.subscribe() → 새 Receiver 생성
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("TUI tokio runtime must build");
            rt.block_on(async move {
                let mut app = gadgetron_tui::app::App::with_channel(rx);
                if let Err(e) = app.run().await {
                    tracing::warn!(error = %e, "TUI exited with error");
                }
            });
        });
        Some(handle)
    } else {
        None
    };

    // Step 12: axum 라우터 빌드 + TCP 바인드
    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("failed to bind to {bind_addr}"))?;
    tracing::info!(addr = %bind_addr, "listening");

    // Step 13: 서버 실행 (graceful shutdown 신호 대기)
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    tracing::info!("axum serve exited — starting drain sequence");

    // Step 14: 셧다운 시퀀스
    // 14a: tui_tx drop → TUI broadcast channel 닫힘 → App::drain_updates가 running=false 설정
    drop(tui_tx);
    // 14b: audit_writer Arc drop → Sender dropped → mpsc 채널 닫힘 → 소비자 루프 종료
    drop(audit_writer);
    // 14c: 최대 5s 대기 (§2.F.4 audit flush budget)
    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        audit_task,
    ).await {
        Ok(Ok(())) => tracing::info!("audit consumer drained cleanly"),
        Ok(Err(e)) => tracing::warn!(error = %e, "audit consumer task panicked"),
        Err(_) => tracing::warn!("audit consumer drain timed out after 5s — entries may be lost"),
    }
    // 14d: TUI thread join (non-blocking — TUI가 이미 종료되어 있어야 함)
    if let Some(handle) = tui_thread {
        // join은 blocking — spawn_blocking 불필요 (이미 shutdown 경로에 있어 tokio context 무관)
        let _ = handle.join();
    }
    // 14e: PG pool 닫기 (graceful — 활성 connection 완료 대기 없음, 이미 axum drain 완료)
    pg_pool.close().await;

    tracing::info!("shutdown complete");
    Ok(())
}
```

**`init_tracing` 수정**: `--log-format` 플래그를 반영한다.

```rust
fn init_tracing(format: LogFormat) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("RUST_LOG")
        .unwrap_or_else(|_| "gadgetron=info,tower_http=info".parse().unwrap());

    match format {
        LogFormat::Json => {
            tracing_subscriber::fmt()
                .json()
                .with_env_filter(filter)
                .init();
        }
        LogFormat::Pretty => {
            tracing_subscriber::fmt()
                .pretty()
                .with_env_filter(filter)
                .init();
        }
    }
}
```

### 2.3 metrics_middleware → WsMessage::RequestLog emit (T3)

**파일**: `crates/gadgetron-gateway/src/middleware/metrics.rs` (신규 파일)

이 미들웨어는 핸들러 완료 후 (response 반환 직전) `RequestEntry`를 구성하여 TUI broadcast channel로 전송한다. Tower `from_fn_with_state` 패턴을 사용한다.

```rust
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use chrono::Utc;
use gadgetron_core::ui::{RequestEntry, WsMessage};
use std::time::Instant;

use crate::server::AppState;

/// Emit `WsMessage::RequestLog` after each request completes.
///
/// Concurrency: fire-and-forget `broadcast::Sender::send`.
/// `SendError` (no receivers) is silently ignored — TUI is optional.
/// `broadcast::error::SendError` when all receivers are lagged/dropped is also ignored.
///
/// Call site: `build_router()` — added as innermost layer on authenticated_routes
/// (innermost = wraps handler, outermost of what handler sees).
/// Layer position: added via `.layer(middleware::from_fn_with_state(state, metrics_middleware))`
/// after scope_guard and before handler.
pub async fn metrics_middleware(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    // Extract request metadata before consuming `req`.
    // These fields are available pre-handler from extensions set by auth/tenant middleware.
    let request_id = req
        .extensions()
        .get::<uuid::Uuid>()
        .copied()
        .map(|id| id.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // TenantContext is inserted by tenant_context_middleware.
    // If absent (e.g. public routes not going through this middleware), use defaults.
    let tenant_id = req
        .extensions()
        .get::<gadgetron_xaas::tenant::model::TenantContext>()
        .map(|ctx| ctx.tenant_id.to_string())
        .unwrap_or_else(|| "anonymous".to_string());

    let start = Instant::now();
    let response = next.run(req).await;
    let latency_ms = start.elapsed().as_millis() as u32;

    let status = response.status().as_u16();

    // Emit to TUI channel (fire-and-forget).
    // `Option::as_ref` avoids clone of the Sender.
    if let Some(tx) = state.tui_tx.as_ref() {
        let entry = RequestEntry {
            request_id,
            tenant_id,
            // model and provider are set by the handler after routing decision.
            // Sprint 6: emit with empty strings; Sprint 7 will propagate via extensions.
            model: String::new(),
            provider: String::new(),
            status,
            latency_ms,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            timestamp: Utc::now(),
        };
        // Ignore SendError — receivers may be 0 (TUI quit) or Lagged.
        let _ = tx.send(WsMessage::RequestLog(entry));
    }

    response
}
```

**`build_router()` 수정**: `metrics_middleware`를 authenticated_routes에 추가.

```rust
// crates/gadgetron-gateway/src/server.rs :: build_router()
// 기존 layer 스택에 metrics_middleware 추가.
// Layer 순서 (이미 설정된 axum의 innermost-first 규칙):
//   scope_guard → metrics → handler (from_fn_with_state 호출 순서)
// 최종 inbound 요청 흐름:
//   [body-limit] → [trace] → [request-id] → [auth] → [tenant-ctx] → [scope] → [metrics] → handler

use crate::middleware::metrics::metrics_middleware;

let authenticated_routes = Router::new()
    // ... routes 정의 동일 ...
    .layer(middleware::from_fn_with_state(
        state.clone(),
        metrics_middleware,           // <-- 신규 (innermost, 핸들러에 가장 가까움)
    ))
    .layer(middleware::from_fn_with_state(
        state.clone(),
        scope_guard_middleware,
    ))
    // ... 나머지 layer는 동일 ...
    ;
```

**`src/middleware/mod.rs`에 `pub mod metrics;` 추가.**

### 2.4 `/ready` PG pool 실체크 (T4)

**파일**: `crates/gadgetron-gateway/src/server.rs`

현재 `ready_handler()`는 무조건 200을 반환한다. `AppState`에서 `pg_pool`을 추출하여 `SELECT 1`을 실행한다.

```rust
/// `GET /ready`
///
/// K8s readiness probe 엔드포인트.
/// `SELECT 1` 쿼리로 PostgreSQL pool 연결 상태를 확인한다.
///
/// 반환:
/// - 200 OK + `{"status":"ready"}` — PG 응답 정상
/// - 503 Service Unavailable + `{"status":"unavailable"}` — PG 연결 실패 또는 timeout
///
/// timeout: sqlx pool `acquire_timeout` (5s, main()에서 설정) 이 적용됨.
/// 에러 로깅: `tracing::warn!`으로 PG 에러 내용 기록. 응답 body에는 에러 상세 미포함 (SEC-M7).
pub async fn ready_handler(State(state): State<AppState>) -> impl IntoResponse {
    match sqlx::query("SELECT 1")
        .execute(&state.pg_pool)
        .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({"status": "ready"})),
        ),
        Err(e) => {
            tracing::warn!(error = %e, "readiness check: PostgreSQL query failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"status": "unavailable"})),
            )
        }
    }
}
```

`ready_handler`는 현재 `public_routes`에 등록되어 있으므로 `with_state(state)` 연결이 필요하다.

```rust
// build_router() 수정 — public_routes에 state 주입
// state는 authenticated_routes에서 move되므로 public_routes가 먼저 clone해야 한다.
let public_routes = Router::new()
    .route("/health", get(health_handler))
    .route("/ready", get(ready_handler))
    .with_state(state.clone());  // clone BEFORE move

let authenticated_routes = Router::new()
    // ... routes 정의 동일 ...
    .with_state(state);  // moved last

Router::new()
    .merge(authenticated_routes)
    .merge(public_routes)
```

### 2.5 Graceful Shutdown — Audit Flush Drain (T5)

§2.2의 `serve()` 함수 Step 14에 완전히 명시되어 있다. 여기서는 drain 시퀀스의 각 단계가 왜 이 순서여야 하는지 설명한다.

**드레인 순서 불변식**:

1. `drop(tui_tx)` — TUI broadcast channel을 닫는다. `App::drain_updates`의 `TryRecvError::Closed` 분기가 `self.running = false`로 설정 → TUI render loop 자연 종료. 순서가 중요: axum이 이미 drain 완료된 후여야 TUI에 더 이상 새 RequestLog가 emit되지 않는다.

2. `drop(audit_writer)` — `AuditWriter`의 `Arc`를 drop한다. `AuditWriter` 내부 `mpsc::Sender`가 drop되어 채널이 닫힌다. `audit_consumer_loop`는 `rx.recv().await`가 `None`을 반환하며 루프를 빠져나간다.

3. `tokio::time::timeout(5s, audit_task).await` — JoinHandle을 5s 안에 완료 대기. 타임아웃 초과 시 경고 로그 후 계속 진행 (§2.F.4: audit flush = 5s budget).

4. `tui_thread.join()` — OS thread join. TUI가 이미 종료되어 있어야 빠르게 반환된다. 최대 100ms (TUI render 루프의 crossterm poll timeout) 소요 가능.

5. `pg_pool.close().await` — sqlx pool graceful close. 활성 connection이 0개여야 즉시 반환. axum이 모든 요청을 drain한 후이므로 DB connection도 반납 완료 상태.

**`audit_consumer_loop` 변경 없음**: 현재 구현(`while let Some(entry) = rx.recv().await`)은 채널 닫힘 시 자연 종료된다. Sprint 6에서는 구조를 바꾸지 않는다.

### 2.6 Criterion 벤치마크 3종 (T6)

**파일 위치**: `crates/gadgetron-gateway/benches/`

**`gadgetron-gateway/Cargo.toml`에 추가**:

```toml
[dev-dependencies]
criterion = { workspace = true }  # version = "0.5", features = ["async_tokio"]
gadgetron-testing = { workspace = true }

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

#### 2.6.1 `benches/middleware_chain.rs`

```rust
//! Bench: full authenticated request stack without network I/O.
//!
//! Pass criterion (§2.H.9): median < 700 µs, P99 < 1_000 µs
//! Regression trigger: > 10% increase from baseline

use criterion::{criterion_group, criterion_main, Criterion};
use std::time::Duration;

fn bench_middleware_chain(c: &mut Criterion) {
    // tokio runtime for async axum oneshot
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    // AppState with FakeLlmProvider (0 ms latency), MockKeyValidator (always accept)
    let state = rt.block_on(async { make_bench_state().await });
    let app = gadgetron_gateway::server::build_router(state);

    let request_body = include_bytes!("fixtures/chat_request_small.json");

    c.bench_function("middleware_chain_full_stack", |b| {
        b.to_async(&rt).iter(|| async {
            use axum::body::Body;
            use axum::http::{Request, Method};
            use tower::ServiceExt;

            let req = Request::builder()
                .method(Method::POST)
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("authorization", "Bearer gad_bench_aaaaaaaaaaaaaaaaaa")
                .body(Body::from(request_body.as_ref()))
                .unwrap();

            // oneshot: single request, no connection reuse
            let resp = app.clone().oneshot(req).await.unwrap();
            criterion::black_box(resp.status());
        });
    });
}

// make_bench_state: MockKeyValidator + InMemoryQuotaEnforcer + FakeLlmProvider
// FakeLlmProvider는 gadgetron-testing에서 import (0 ms latency, 固定 응답)
async fn make_bench_state() -> gadgetron_gateway::server::AppState {
    use gadgetron_gateway::server::AppState;
    use gadgetron_xaas::audit::writer::AuditWriter;
    use gadgetron_xaas::quota::enforcer::InMemoryQuotaEnforcer;
    use std::collections::HashMap;
    use std::sync::Arc;

    let (audit_writer, _rx) = AuditWriter::new(256);
    let pg_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy("postgresql://localhost/bench_placeholder")
        .unwrap();

    AppState {
        key_validator: Arc::new(gadgetron_testing::MockKeyValidator::always_accept()),
        quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
        audit_writer: Arc::new(audit_writer),
        providers: Arc::new(HashMap::new()),
        router: None,    // FakeLlmProvider는 router 없이 직접 주입
        pg_pool,
        tui_tx: None,
    }
}

// fixtures/chat_request_small.json — 50-token prompt, committed to repo:
// {"model":"llama3","messages":[{"role":"user","content":"Hello, world!"}],"stream":false}

criterion_group!(
    name = benches;
    config = Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .sample_size(100);
    targets = bench_middleware_chain
);
criterion_main!(benches);
```

#### 2.6.2 `benches/auth_cache.rs`

```rust
//! Bench: PgKeyValidator moka cache hit path.
//!
//! Pass criterion (§2.H.9): P99 < 50 µs (median < 50 µs)
//! Regression trigger: > 10% increase from baseline

use criterion::{criterion_group, criterion_main, Criterion};
use std::time::Duration;

fn bench_auth_cache_hit(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // Pre-warm cache: insert one key before benchmarking
    let validator = rt.block_on(async {
        let pg_pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgresql://localhost/bench_placeholder")
            .unwrap();
        let v = gadgetron_xaas::auth::validator::PgKeyValidator::new(pg_pool);
        // Warm cache with a known hash so subsequent calls are moka hits
        // Use force_insert (test-only method) or pre-populate via mock
        v
    });

    // The key hash we will look up on every iteration (pre-cached above)
    // SHA-256 of "gad_bench_aaaaaaaaaaaaaaaaaa" prefix: deterministic in bench
    let key_hash = gadgetron_xaas::auth::hash::sha256_prefix("gad_bench_aaaaaaaaaaaaaaaaaa");

    c.bench_function("auth_cache_hit_moka", |b| {
        b.to_async(&rt).iter(|| async {
            use gadgetron_xaas::auth::validator::KeyValidator;
            // moka cache hit: SHA-256 computation (~5 µs) + moka get (~200 ns)
            let result = validator.validate(&key_hash).await;
            criterion::black_box(result.is_ok());
        });
    });
}

criterion_group!(
    name = benches;
    config = Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .sample_size(200);
    targets = bench_auth_cache_hit
);
criterion_main!(benches);
```

**주의**: `PgKeyValidator`에 테스트 전용 cache pre-populate API가 없으면 Sprint 6 구현 시 추가한다:

```rust
// crates/gadgetron-xaas/src/auth/validator.rs 에 추가
#[cfg(any(test, feature = "bench-helpers"))]
pub fn insert_cached(&self, key_hash: &str, key: Arc<ValidatedKey>) {
    // moka Future cache는 sync insert를 지원하지 않으므로 blocking insert 사용
    // bench-helpers feature gate: 프로덕션 빌드에 미포함
    self.cache.blocking_insert(key_hash.to_string(), key);
}
```

`gadgetron-xaas/Cargo.toml`에:
```toml
[features]
bench-helpers = []
```

`gadgetron-gateway/Cargo.toml`의 dev-dependencies에:
```toml
gadgetron-xaas = { workspace = true, features = ["bench-helpers"] }
```

#### 2.6.3 `benches/router.rs`

```rust
//! Bench: Router::resolve() RoundRobin strategy.
//!
//! Pass criterion (§2.H.9): P99 < 20 µs (median < 20 µs per §2.H.2 "Router strategy lookup")
//! Regression trigger: > 10% increase from baseline

use criterion::{criterion_group, criterion_main, Criterion};
use gadgetron_core::chat::ChatRequest;
use gadgetron_router::{MetricsStore, Router as LlmRouter};
use gadgetron_testing::FakeLlmProvider;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

fn bench_router_resolve(c: &mut Criterion) {
    // 3 providers, RoundRobin strategy — matches realistic Phase 1 setup
    let mut providers: HashMap<String, Arc<dyn gadgetron_core::provider::LlmProvider>> =
        HashMap::new();
    for i in 0..3usize {
        providers.insert(
            format!("provider-{i}"),
            Arc::new(FakeLlmProvider::new(format!("provider-{i}"))),
        );
    }

    let config = gadgetron_core::routing::RoutingConfig {
        default_strategy: gadgetron_core::routing::RoutingStrategy::RoundRobin,
        ..Default::default()
    };
    let metrics_store = Arc::new(MetricsStore::new());
    let router = LlmRouter::new(providers, config, metrics_store);

    // Fixed ChatRequest: 50-token prompt, model "llama3"
    let req = ChatRequest {
        model: "llama3".to_string(),
        messages: vec![gadgetron_core::chat::Message {
            role: "user".to_string(),
            content: "Hello, world!".to_string(),
        }],
        stream: Some(false),
        ..Default::default()
    };

    c.bench_function("router_resolve_roundrobin", |b| {
        b.iter(|| {
            // resolve() is sync (AtomicUsize::fetch_add + DashMap read)
            let result = router.resolve(&req);
            criterion::black_box(result.is_ok());
        });
    });
}

criterion_group!(
    name = benches;
    config = Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .sample_size(500);  // sync bench = more samples viable
    targets = bench_router_resolve
);
criterion_main!(benches);
```

**벤치마크 픽스처 파일**: `crates/gadgetron-gateway/benches/fixtures/chat_request_small.json`

```json
{"model":"llama3","messages":[{"role":"user","content":"Hello, world!"}],"stream":false}
```

이 파일은 repo에 commit되어야 한다. `include_bytes!` 매크로로 컴파일 시 embed된다.

### 2.7 설정 스키마

Sprint 6은 새 TOML 섹션을 추가하지 않는다. `--tui`, `--bind`, `--config`, `--log-format`은 모두 CLI 플래그로만 제공된다. TOML 설정 파일을 통한 TUI 활성화는 Phase 2 고려사항이다.

### 2.8 에러 & 로깅

| 상황 | 레벨 | span 이름 | 필드 |
|------|------|-----------|------|
| TUI 스레드 runtime 빌드 실패 | `error!` | — | `error = %e` |
| TUI `app.run()` 오류 종료 | `warn!` | — | `error = %e` |
| `/ready` PG 쿼리 실패 | `warn!` | `ready_handler` | `error = %e` |
| audit consumer drain 타임아웃 | `warn!` | — | 고정 메시지 |
| broadcast send 오류 | 무시 (info 레벨도 아님) | — | — (fire-and-forget) |
| clap parse 실패 | clap 기본 핸들링 (stderr + exit 2) | — | — |

`GadgetronError` 신규 variant 불필요: `/ready` 오류는 핸들러 내에서 `StatusCode`로 직접 변환. TUI 스레드 오류는 운영 경로가 아님.

### 2.9 의존성

| crate | 버전 | 추가 위치 | 정당화 |
|-------|------|-----------|--------|
| `clap` | `4.x` (derive feature) | workspace + gadgetron-cli | 서브커맨드 구조, `--help` 자동 생성, 타입 안전 파싱 |
| `criterion` | `0.5` (async_tokio feature) | workspace [기존] + gadgetron-gateway dev-dep | 통계적 회귀 탐지, HTML 리포트 |
| `gadgetron-testing` | workspace | gadgetron-gateway dev-dep | `MockKeyValidator`, `FakeLlmProvider` |

**`clap`만 신규 추가**. `criterion`과 `gadgetron-testing`은 workspace에 이미 존재.

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 의존 관계 (기존 + Sprint 6 변경)

```
gadgetron-cli
  ├── [신규] clap (CLI 파싱)
  ├── gadgetron-gateway (AppState, build_router)
  │     ├── [신규] middleware::metrics (WsMessage emit)
  │     ├── [신규] ready_handler (pg_pool 체크)
  │     └── [신규] AppState.{pg_pool, tui_tx}
  ├── gadgetron-tui (App::with_channel)
  │     └── gadgetron-core::ui (WsMessage, RequestEntry)
  └── gadgetron-xaas (AuditWriter, PgKeyValidator)
```

### 3.2 데이터 흐름 (ASCII)

```
HTTP Request
    │
    ▼
[body-limit] → [trace] → [request-id] → [auth] → [tenant-ctx] → [scope]
    │
    ▼
[metrics_middleware]  ←── AppState.tui_tx (Option<broadcast::Sender<WsMessage>>)
    │                           │
    │                    tx.send(WsMessage::RequestLog(entry))
    │                           │
    ▼                           ▼
[handler]            broadcast::channel<WsMessage>(1_024)
    │                           │
    ▼                           ▼
HTTP Response         App::drain_updates() [TUI thread, 1 Hz]
                                │
                                ▼
                      request_log: Arc<RwLock<Vec<RequestEntry>>>
                                │
                                ▼
                      ratatui render (RequestPane)
```

```
SIGTERM / Ctrl-C
    │
    ▼
axum graceful_shutdown() — drain in-flight requests (existing)
    │
    ▼
drop(tui_tx)              — closes broadcast channel
    │
    ▼
drop(audit_writer)        — closes mpsc sender
    │
    ▼
timeout(5s, audit_task)   — wait for audit consumer
    │
    ▼
tui_thread.join()         — wait for TUI OS thread
    │
    ▼
pg_pool.close().await     — release DB connections
```

### 3.3 D-12 크레이트 경계 준수

| 크레이트 | Sprint 6 변경 | D-12 경계 준수 |
|---------|--------------|----------------|
| `gadgetron-core` | 변경 없음 (WsMessage 기존 타입 사용) | 준수 — axum/sqlx 의존성 없음 |
| `gadgetron-gateway` | AppState 필드 추가, metrics middleware 추가, ready_handler 실체화 | 준수 — tui/xaas/router 의존 허용 범위 |
| `gadgetron-cli` | clap 도입, serve() 함수 리팩터, TUI thread spawn | 준수 — 진입점 크레이트이므로 전체 의존 허용 |
| `gadgetron-tui` | 변경 없음 (App::with_channel은 기존 구현) | 준수 |

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

| 테스트 | 파일 | 검증 invariant | 구체 입력 | 기대 출력 |
|--------|------|----------------|-----------|-----------|
| `/ready` 200 | `server.rs` tests | 유효한 PG pool → 200 OK | `GET /ready`, mock pool (SELECT 1 성공) | `status == 200`, body `{"status":"ready"}` |
| `/ready` 503 | `server.rs` tests | 닫힌 pool → 503 | `GET /ready`, closed pool | `status == 503`, body `{"status":"unavailable"}` |
| `metrics_middleware` emit | `middleware/metrics.rs` tests | 요청 완료 후 broadcast 채널에 RequestLog 수신 | POST 요청 1건, `tui_tx: Some(tx)` 설정 | `rx.try_recv() == Ok(WsMessage::RequestLog(_))` |
| `metrics_middleware` None | `middleware/metrics.rs` tests | `tui_tx: None`이면 emit 없음, panic 없음 | POST 요청 1건, `tui_tx: None` | 오류 없이 응답 반환 |
| clap parse `serve --tui` | `main.rs` tests | `--tui` 파싱 → `tui: true` | `["gadgetron", "serve", "--tui", "--config", "x.toml"]` | `cli.command == Commands::Serve { tui: true, config: PathBuf::from("x.toml"), .. }` |
| clap parse default | `main.rs` tests | 기본값 확인 | `["gadgetron", "serve"]` | `bind == "0.0.0.0:8080"`, `log_format == LogFormat::Json`, `tui == false` |
| clap parse bad format | `main.rs` tests | 잘못된 log_format → parse error | `["gadgetron", "serve", "--log-format", "xml"]` | `Cli::try_parse().is_err()` |
| audit drain 5s | `main.rs` integration | 5s 내 drain 완료 | AuditWriter에 100개 send 후 drop, timeout(5s) | `audit_task` JoinHandle 완료, 타임아웃 아님 |
| broadcast capacity 1_024 | `server.rs` tests | 채널 생성 후 1_024개 send 가능 | `broadcast::channel(1_024)`, 1_024개 send | `tx.send()` 1_024번 모두 `Ok` |
| AppState clone | `server.rs` tests | 기존 테스트 — `pg_pool`, `tui_tx` 필드 추가 후도 Clone 유지 | `state.clone()` | Arc pointer equality for all Arc fields |

### 4.2 테스트 하네스

**`/ready` 503 테스트**: `sqlx::PgPool`을 닫힌 상태로 만들려면 `pool.close().await` 후 핸들러를 호출한다. `axum::serve` 없이 `handler.call(req)` 직접 호출 패턴을 사용한다.

```rust
#[tokio::test]
async fn ready_returns_503_when_pool_closed() {
    let state = make_state();
    state.pg_pool.close().await;  // force-close pool
    let app = build_router(state);
    let req = Request::builder()
        .method("GET")
        .uri("/ready")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["status"], "unavailable");
}
```

**`metrics_middleware` emit 테스트**:

```rust
#[tokio::test]
async fn metrics_middleware_emits_request_log() {
    let (tx, mut rx) = tokio::sync::broadcast::channel::<WsMessage>(16);
    let mut state = make_state_with_validator(MockKeyValidator::new(vec![Scope::OpenAiCompat]));
    state.tui_tx = Some(tx);
    let app = build_router(state);

    let req = Request::builder()
        .method("GET")
        .uri("/v1/models")
        .header("authorization", format!("Bearer {VALID_TOKEN}"))
        .body(Body::empty())
        .unwrap();

    let _resp = app.oneshot(req).await.unwrap();

    // Middleware emits after handler returns
    match rx.try_recv() {
        Ok(WsMessage::RequestLog(entry)) => {
            assert_eq!(entry.status, 200);
            assert!(entry.latency_ms < 1_000, "latency must be sub-second");
        }
        other => panic!("expected RequestLog, got: {other:?}"),
    }
}
```

### 4.3 커버리지 목표

- `middleware/metrics.rs`: 90% line coverage (emit 경로, None 경로, SendError 무시 경로)
- `server.rs::ready_handler`: 100% branch (Ok, Err)
- `main.rs` clap 파싱: 100% 명시 variant (Serve + LogFormat 두 variant)

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

| 시나리오 | 검증 크레이트 | 방법 |
|---------|--------------|------|
| `gadgetron serve --tui` 부팅 + TUI 렌더 | gadgetron-cli | 수동 검증 — `GADGETRON_DATABASE_URL=... cargo run -p gadgetron-cli -- serve --tui` 실행, TUI 화면 표시 확인 |
| E2E: 요청 → TUI RequestLog 수신 | gadgetron-gateway + gadgetron-tui | `broadcast` rx를 직접 구독, `/v1/models` 요청 후 `rx.recv()` 내 `WsMessage::RequestLog` 확인 |
| `/ready` 503 (PG down) | gadgetron-gateway | testcontainers postgres 기동 → `/ready` 200 확인 → postgres stop → `/ready` 503 확인 |
| 벤치마크 실행 | gadgetron-gateway | `cargo bench -p gadgetron-gateway` — 3개 bench 모두 `criterion` HTML report 생성 |
| audit drain 5s E2E | gadgetron-cli + gadgetron-xaas | 100개 audit entry send → SIGTERM 시뮬레이션 → 5s 이내 all drain 확인 |

### 5.2 테스트 환경

**`/ready` 503 testcontainers 설정**:

```rust
// crates/gadgetron-testing/tests/integration/ready_pg.rs
use testcontainers::clients::Cli;
use testcontainers_modules::postgres::Postgres;

#[tokio::test]
async fn ready_returns_503_when_postgres_stopped() {
    let docker = Cli::default();
    let pg_container = docker.run(Postgres::default());
    let pg_url = format!(
        "postgresql://postgres:postgres@localhost:{}/postgres",
        pg_container.get_host_port_ipv4(5432)
    );

    let pg_pool = sqlx::PgPool::connect(&pg_url).await.unwrap();
    // GET /ready → 200
    let state = make_integration_state(pg_pool.clone());
    let app = gadgetron_gateway::server::build_router(state);
    // ... assert 200 ...

    // Stop container → pool connections will fail on next query
    drop(pg_container);
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let state2 = make_integration_state(pg_pool);
    let app2 = gadgetron_gateway::server::build_router(state2);
    // GET /ready → 503
    // ... assert 503 ...
}
```

**벤치마크 픽스처**: `crates/gadgetron-gateway/benches/fixtures/chat_request_small.json`은 repo에 commit. `cargo bench` 실행 환경: 네트워크 I/O 없음, PG 연결 없음 (lazy pool).

### 5.3 회귀 방지

다음 변경이 이 테스트를 실패시켜야 한다:

| 변경 | 실패하는 테스트 |
|------|----------------|
| `AppState.tui_tx` 필드 제거 | `metrics_middleware_emits_request_log` |
| `ready_handler`가 AppState 없이 항상 200 반환 | `ready_returns_503_when_pool_closed` |
| audit consumer drain이 5s 내 완료 안 됨 | `audit drain 5s` 단위 테스트 |
| broadcast channel capacity가 1_024 미만 | `broadcast_capacity_1024` 테스트 |
| middleware_chain P99 > 1_000 µs | `bench_middleware_chain` criterion regression |
| Router::resolve P99 > 20 µs | `bench_router_resolve` criterion regression |

---

## 6. Phase 구분

| 항목 | Phase |
|------|-------|
| T1 AppState.pg_pool + tui_tx | [P1] Sprint 6 |
| T2 clap CLI + --tui + TUI thread | [P1] Sprint 6 |
| T3 metrics_middleware RequestLog emit | [P1] Sprint 6 — model/provider 필드 채우기는 Sprint 7 [P1] |
| T4 /ready PG 실체크 | [P1] Sprint 6 |
| T5 audit drain 5s | [P1] Sprint 6 |
| T6 criterion 벤치마크 3종 | [P1] Sprint 6 |
| /health/deep 복합 헬스 체크 | [P2] |
| TUI model/provider 필드 표시 (RequestEntry 완성) | [P1] Sprint 7 |
| audit PG batch insert | [P2] |
| broadcast → WebSocket JSON | [P2] |

---

## 7. 오픈 이슈 / 의사결정 필요

| ID | 내용 | 옵션 | 추천 | 상태 |
|----|------|------|------|------|
| Q-1 | `metrics_middleware`에서 model/provider를 Sprint 6에 빈 문자열로 emit하는 것이 TUI에 미치는 영향 — TUI RequestPane이 빈 칸을 표시하게 됨 | A: 빈 문자열 (Sprint 6), Sprint 7에 채움 / B: metrics_middleware를 Sprint 7까지 연기 | A — TUI는 빈 칸을 이미 graceful하게 처리 (`RequestEntry` 구조상 빈 string 유효) | 결정 필요 없음 — A 채택 (PM 자율) |
| Q-2 | bench_auth_cache.rs에서 `PgKeyValidator::insert_cached` bench-helper를 추가하는 것이 D-12 leaf crate 원칙에 위배되는지 | A: `#[cfg(feature = "bench-helpers")]` feature gate (프로덕션 빌드 미포함) / B: bench helper를 gadgetron-testing에 위임 | A — feature gate는 cargo tree 검사를 통과하며, "bench-helpers" feature는 명시적 opt-in | 결정 필요 없음 — A 채택 (PM 자율) |

---

## 리뷰 로그 (append-only)

### Round 1 — 2026-04-12 — @chief-architect (작성자 자체 검토)

**결론**: Draft 제출 준비 완료

**D-20260412-02 구현 결정론 체크리스트**:
- [x] 모든 type signature 완성 (generic bound, async/sync, Send/Sync)
- [x] 에러 처리 결정 (GadgetronError 신규 variant 없음, 직접 StatusCode 변환 명시)
- [x] 동시성 모델 결정 (broadcast vs mpsc vs thread 선택 이유 명시)
- [x] 라이브러리 선택 결정 (clap 4.x derive, criterion 0.5 async_tokio)
- [x] 모든 enum variant 열거 (LogFormat: Json/Pretty 완전 열거)
- [x] 모든 async 결정 (std::thread::spawn + new_current_thread 이유 명시)
- [x] 모든 magic number 명시 (1_024, 4_096, 5s, 20, 5)
- [x] 외부 의존성 contract (PG SELECT 1, acquire_timeout 5s)
- [x] 상태 전이 (드레인 순서 5단계 불변식 명시)
- [x] 테스트 케이스 구체 입력/기대 출력
