# Sprint 4: vLLM + SGLang Provider Activation

> **담당**: @inference-engine-lead
> **상태**: ✅ Implemented (commit `d53aa60`, 2026-04-12) — 코드/테스트/문서 동기화 완료
> **작성일**: 2026-04-12
> **최종 업데이트**: 2026-04-12
> **관련 크레이트**: `gadgetron-cli`, `gadgetron-provider`, `gadgetron-core`
> **Phase**: [P1]

---

## 1. 철학 & 컨셉 (Why)

### 해결 문제

`build_providers()` 내 `ProviderConfig::Vllm` / `ProviderConfig::Sglang` 분기가 `anyhow::bail!("unsupported provider type in Phase 1: {kind}")` 를 반환한다. `VllmProvider`와 `SglangProvider` 구현체는 이미 완성되어 있고, `ProviderConfig` 스키마도 확정되어 있다. 유일한 장애물은 `build_providers()` 에서 두 분기가 오류를 반환하는 것이다.

이 Sprint의 단일 목표: **"Activate, Don't Invent"** — 새 코드를 발명하지 않고 이미 존재하는 코드를 연결한다.

### 결정론 준수 (D-20260412-02)

이 문서는 어떤 코더가 읽어도 동일한 구현 결과가 나오도록 작성한다. 모든 타입 시그니처, 에러 처리, 테스트 입력/기대 출력을 완전히 명시한다.

### 제품 비전 연결

`docs/00-overview.md §1` 에서 Gadgetron은 "6 provider 어댑터를 통합"하는 플랫폼임을 정의한다. D-20260411-01 B (Phase 1 12주 중간 집합)에 vLLM + SGLang이 포함 확정됨. 이 Sprint는 그 약속의 첫 검증 지점이다.

### 고려한 대안

| 대안 | 채택하지 않은 이유 |
|------|-------------------|
| Gemini 동시 활성화 | D-20260411-02에서 Gemini는 Week 5-7에 `SseToChunkNormalizer` 추출 후 별도 처리로 확정. 이 Sprint에서 Gemini는 변경 없음. |
| `bail!` 제거 후 warning log만 | 등록된 provider가 silent하게 무시되면 운영자가 설정 오류를 감지할 수 없음. 활성화 vs. 에러는 명시적 이진 결정이어야 한다. |
| SGLang `reasoning_content` 전용 파싱 | SGLang GLM-5.1 모델이 `reasoning_content` 필드를 응답에 포함하나, `ChatChunk` serde 역직렬화가 알 수 없는 필드를 오류로 처리하는 경우 스트림이 중단된다. 이를 방어적으로 처리한다. |

### 핵심 설계 원칙

- **최소 변경**: `build_providers()` 두 분기 교체 + `ChunkDelta`에 필드 1개 추가. 그 외는 건드리지 않는다.
- **수신 측 방어**: SGLang이 `reasoning_content`를 보내도 기존 소비자가 영향받지 않도록 `Option<String>` + `skip_serializing_if` 처리.
- **관측 가능성**: 기존 `tracing::info!(name = %name, "provider registered")` 로그가 이미 있으므로 추가 로그 불필요.

---

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API — 변경되는 타입

#### 2.1.1 `ChunkDelta` 필드 추가

파일: `crates/gadgetron-core/src/provider.rs`

변경 전:
```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChunkDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallChunk>>,
}
```

변경 후:
```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChunkDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallChunk>>,
    /// SGLang (GLM-5.1 등 reasoning 모델)이 반환하는 추론 과정 텍스트.
    /// OpenAI 호환 엔진은 이 필드를 보내지 않는다. serde(default)로 누락 시 None.
    /// skip_serializing_if로 Gadgetron → 클라이언트 응답에서도 None이면 생략.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}
```

이유: SGLang GLM-5.1이 SSE 스트림 `delta` 오브젝트에 `reasoning_content` 키를 포함한다. `serde_json::from_str::<ChatChunk>(data)` 가 `deny_unknown_fields` 없이 컴파일되었으므로 현재 알 수 없는 필드는 무시된다. 그러나 명시적 필드로 선언해야 Gadgetron이 클라이언트에게 해당 필드를 pass-through할 수 있고, 미래 소비자(TUI, billing)가 타입 안전하게 접근할 수 있다.

#### 2.1.2 `Choice.message` — `reasoning_content` 필드 추가

파일: `crates/gadgetron-core/src/provider.rs` — `Choice` 구조체는 `Message`를 가리키는데, `Message`는 `gadgetron-core/src/message.rs`에 정의되어 있다. 비스트리밍 응답의 추론 내용 보존을 위해 `Message`에 필드를 추가한다.

변경 전 (`crates/gadgetron-core/src/message.rs`):
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Content,
}
```

변경 후:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Content,
    /// SGLang reasoning 모델(GLM-5.1 등)의 비스트리밍 응답에서 반환되는 추론 과정.
    /// 비reasoning 모델 + 모든 OpenAI/Anthropic 응답에는 존재하지 않는다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}
```

**기존 생성자(`Message::user`, `Message::system`, `Message::assistant`) 변경 불필요**: 세 생성자 모두 `reasoning_content: None` 이 `Default`로 채워진다. 단, `#[derive(Default)]`가 없으므로 세 함수 본문을 직접 수정한다:

```rust
pub fn user(text: impl Into<String>) -> Self {
    Self {
        role: Role::User,
        content: Content::Text(text.into()),
        reasoning_content: None,
    }
}

pub fn system(text: impl Into<String>) -> Self {
    Self {
        role: Role::System,
        content: Content::Text(text.into()),
        reasoning_content: None,
    }
}

pub fn assistant(text: impl Into<String>) -> Self {
    Self {
        role: Role::Assistant,
        content: Content::Text(text.into()),
        reasoning_content: None,
    }
}
```

### 2.2 `build_providers()` 수정

파일: `crates/gadgetron-cli/src/main.rs`

변경 전 (`other =>` 분기 내부):
```rust
other => {
    let kind = match other {
        ProviderConfig::Gemini { .. } => "gemini",
        ProviderConfig::Vllm { .. } => "vllm",
        ProviderConfig::Sglang { .. } => "sglang",
        _ => "unknown",
    };
    anyhow::bail!("unsupported provider type in Phase 1: {kind}");
}
```

변경 후 (`match provider_cfg` 블록 전체):
```rust
let provider: Arc<dyn LlmProvider + Send + Sync> = match provider_cfg {
    ProviderConfig::Openai {
        api_key, base_url, ..
    } => Arc::new(gadgetron_provider::OpenAiProvider::new(
        api_key.clone(),
        base_url.clone(),
    )),
    ProviderConfig::Anthropic {
        api_key, base_url, ..
    } => Arc::new(gadgetron_provider::AnthropicProvider::new(
        api_key.clone(),
        base_url.clone(),
    )),
    ProviderConfig::Ollama { endpoint } => {
        Arc::new(gadgetron_provider::OllamaProvider::new(endpoint.clone()))
    }
    ProviderConfig::Vllm { endpoint, api_key } => Arc::new(
        gadgetron_provider::VllmProvider::new(endpoint.clone(), api_key.clone()),
    ),
    ProviderConfig::Sglang { endpoint, api_key } => Arc::new(
        gadgetron_provider::SglangProvider::new(endpoint.clone(), api_key.clone()),
    ),
    // SEC-M2: Gemini는 D-20260411-02에 따라 Week 5-7 구현 예정.
    // api_key 노출 방지: format!("{:?}", provider_cfg) 금지.
    ProviderConfig::Gemini { .. } => {
        anyhow::bail!("gemini provider is scheduled for Week 5-7 (D-20260411-02); remove from config until then");
    }
};
```

변경 이유:
- `ProviderConfig::Vllm` / `ProviderConfig::Sglang` 각각 대응 생성자 호출. `VllmProvider::new(endpoint: String, api_key: Option<String>)` / `SglangProvider::new(endpoint: String, api_key: Option<String>)` 시그니처와 `ProviderConfig` 필드가 1:1 대응된다.
- Gemini는 여전히 bail이지만 메시지를 "Phase 1 unsupported"에서 "Week 5-7 예정"으로 개선해 운영자 UX 향상. 이 변경은 기존 behavior를 유지하면서 더 명확한 안내를 제공한다.

**forward compatibility catch-all**: `ProviderConfig`는 `#[non_exhaustive]`가 없다 (`crates/gadgetron-core/src/config.rs:35` 참조). 즉 현재는 Rust 컴파일러가 exhaustive match를 강제한다 — 새 variant가 추가되면 이 `match`가 컴파일 에러를 낸다. 이 점을 활용해 코더가 새 variant 추가 시 반드시 이 match도 업데이트하도록 유도한다. 단, 만약 미래에 `#[non_exhaustive]`가 추가될 경우를 대비해 match 블록 끝에 다음 catch-all을 포함해야 한다:

```rust
// 이 줄은 현재 dead_code 경고를 낼 수 있으나 #[non_exhaustive] 추가 시 필수가 된다.
// _ => anyhow::bail!("unknown provider type; update build_providers() to handle it"),
```

현재(Sprint 4) 시점에서 `ProviderConfig`에 `#[non_exhaustive]`가 없으므로 위 catch-all은 주석 처리 상태로 유지한다. `#[non_exhaustive]`가 추가되는 시점에 주석을 해제한다.

**Downstream struct literal fixes**: After adding `reasoning_content` to `Message`, update these 2 files to include `reasoning_content: None` in their `Message { ... }` literals:

- `crates/gadgetron-gateway/src/handlers.rs:268` — `FakeLlmProvider` test helper builds `Message { role: Role::Assistant, content: Content::Text(...) }`. Add `reasoning_content: None` as a third field.
- `crates/gadgetron-provider/src/anthropic.rs:211` — `AnthropicProvider` builds `Message { role: Role::Assistant, content: assistant_content }` when converting Anthropic non-streaming responses. Add `reasoning_content: None` as a third field.

Both files will fail to compile with a struct-update or exhaustive-literal error once `Message` gains the new field, because `Message` has no `#[non_exhaustive]` and no `Default` derive. Adding `reasoning_content: None` to each literal is the complete fix.

### 2.3 설정 스키마

`gadgetron.toml` vLLM 최소 설정 (E2E 테스트용):

```toml
[server]
bind = "0.0.0.0:8080"

[router]
strategy = "round_robin"
fallback_enabled = false

[providers.vllm-gemma]
type = "vllm"
endpoint = "http://10.100.1.5:8100"
# api_key 생략 = None → Authorization 헤더 없이 요청

[providers.sglang-glm]
type = "sglang"
endpoint = "http://10.100.1.110:30000"
# api_key 생략 = None
```

검증 규칙:
- `endpoint`는 schema 포함 URL이어야 한다 (`http://` 또는 `https://`). 현재 `ProviderConfig` 수준 검증 없음 — `VllmProvider::new` 에 전달되어 `format!("{}/v1/chat/completions", self.endpoint)` 로 URL이 조합되므로 잘못된 endpoint는 런타임 reqwest 에러로 나타난다. [P2] 에서 `url::Url::parse` 검증 추가 예정.
- `api_key`가 `${ENV_VAR}` 형식이면 `AppConfig::resolve_env_vars()` 가 환경 변수로 치환한다. 없는 환경 변수는 리터럴 그대로 유지된다 (기존 동작).

### 2.4 에러 & 로깅

신규 `GadgetronError` variant: **없음**. 기존 `GadgetronError::Provider(String)` 가 모든 HTTP 에러, 파싱 에러를 포괄한다. `chief-architect` 승인 없이 variant 추가 불가 (역할 규칙).

로깅 — 기존 코드가 이미 처리:
```
tracing::info!(name = %name, "provider registered")  // build_providers 내
tracing::warn!(path = %config_path, "config file not found — using built-in defaults")
```

추가 span 불필요. `VllmProvider::chat` / `SglangProvider::chat_stream` 내부 에러는 `GadgetronError::Provider` 메시지에 포함되어 gateway 레이어에서 `tracing::error!` 로 emit된다.

**SEC-M2 준수**: `ProviderConfig::Gemini { .. }` 분기에서 `{:?}` 포맷 사용 금지. api_key 필드가 출력되지 않도록 variant를 패턴 매칭하지 않고 unit-like로 처리.

### 2.5 의존성

Sprint 4에서 추가 crate 없음. `VllmProvider` / `SglangProvider` 는 이미 `gadgetron-provider/Cargo.toml`의 기존 의존성(`reqwest`, `async-stream`, `async-trait`, `futures`, `serde_json`)만 사용한다.

---

## 3. 전체 모듈 연결 구도 (Where)

### 데이터 흐름 (비스트리밍)

```
curl POST /v1/chat/completions
  │
  ▼
gadgetron-gateway (axum, Tower 미들웨어 16-layer)
  │  Authorization: Bearer gad_live_<token>
  │  → PgKeyValidator (moka cache → PostgreSQL api_keys)
  │  → QuotaEnforcer (InMemoryQuotaEnforcer)
  │  → AuditWriter (mpsc ch 4096)
  ▼
gadgetron-router (LlmRouter::chat)
  │  strategy = round_robin
  │  providers: {"vllm-gemma": Arc<VllmProvider>}
  ▼
gadgetron-provider::VllmProvider::chat(ChatRequest)
  │  reqwest POST http://10.100.1.5:8100/v1/chat/completions
  │  JSON body: ChatRequest (serde)
  ▼
vLLM (10.100.1.5:8100)
  │  model: cyankiwi/gemma-4-31B-it-AWQ-4bit
  ▼
ChatResponse → gadgetron-gateway → curl
```

### 데이터 흐름 (스트리밍)

```
curl POST /v1/chat/completions  {"stream": true}
  │
  ▼
gadgetron-gateway
  ▼
LlmRouter::chat_stream(ChatRequest)
  ▼
VllmProvider::chat_stream(ChatRequest)  ← stream=true 강제 설정됨
  │  reqwest bytes_stream()
  │  SSE 파싱: "\n\n" 구분, "data: " prefix 제거
  │  serde_json::from_str::<ChatChunk>(data)
  │  "data: [DONE]" → 무시 후 스트림 종료
  ▼
Stream<Item = Result<ChatChunk, GadgetronError>>
  ▼
gateway → SSE "data: {json}\n\n" → curl
  마지막 줄: "data: [DONE]\n\n"
```

### 크레이트 경계 (D-12 준수)

| 크레이트 | 이 Sprint에서 변경 | 변경 이유 |
|---------|-------------------|----------|
| `gadgetron-core` | `provider.rs`: `ChunkDelta.reasoning_content` 추가 | 공유 타입이므로 core에 위치 (D-12) |
| `gadgetron-core` | `message.rs`: `Message.reasoning_content` 추가 | 동상 |
| `gadgetron-cli` | `main.rs`: `build_providers()` 2개 분기 교체 | entrypoint — provider 인스턴스화 책임 |
| `gadgetron-provider` | **변경 없음** | `VllmProvider`, `SglangProvider` 이미 완성 |
| `gadgetron-router` | **변경 없음** | 라우터는 `Arc<dyn LlmProvider>` 를 통해 호출 |
| `gadgetron-gateway` | **변경 없음** | `ChatChunk` 직렬화는 `core`에 위임 |

### 타 서브에이전트 인터페이스 계약

- `gateway-router-lead`: `chat_stream` 이 `Stream<Item = Result<ChatChunk, GadgetronError>>` 를 반환하는 계약은 변경 없음. `reasoning_content` 는 `Option`이므로 기존 소비자에 영향 없음.
- `chief-architect`: `Message` 구조체에 새 `Option` 필드 추가. `#[non_exhaustive]`가 없으므로 `Message { role, content }` 패턴으로 소비하는 코드가 있다면 컴파일 에러가 발생한다. 현재 codebase에 해당 패턴은 2개 존재하며 (handlers.rs:268, anthropic.rs:211), §2.2 Downstream struct literal fixes에 fix가 명시되어 있다.

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

| 테스트 함수 | 대상 | Invariant |
|------------|------|-----------|
| `build_providers_vllm_config_returns_provider` | `build_providers()` | `ProviderConfig::Vllm` 분기가 `VllmProvider`를 반환하고 에러가 없음 |
| `build_providers_sglang_config_returns_provider` | `build_providers()` | `ProviderConfig::Sglang` 분기가 `SglangProvider`를 반환하고 에러가 없음 |
| `build_providers_gemini_still_bails` | `build_providers()` | `ProviderConfig::Gemini` 분기가 여전히 에러를 반환함 |
| `chunk_delta_reasoning_content_deserializes` | `ChunkDelta` serde | `reasoning_content` 필드가 있을 때 `Some(String)` 으로 역직렬화됨 |
| `chunk_delta_missing_reasoning_content_is_none` | `ChunkDelta` serde | `reasoning_content` 필드가 없을 때 `None` 으로 역직렬화됨 |
| `chunk_delta_serializes_without_reasoning_when_none` | `ChunkDelta` serde | `reasoning_content: None` 일 때 직렬화 출력에 `reasoning_content` 키가 없음 |
| `message_reasoning_content_deserializes` | `Message` serde | 비스트리밍 응답 `Message` 에 `reasoning_content` 포함 시 `Some` 역직렬화 |
| `message_constructors_set_reasoning_none` | `Message::user` / `system` / `assistant` | 세 생성자 모두 `reasoning_content: None` 반환 |

### 4.2 구체 입력 / 기대 출력

#### `build_providers_vllm_config_returns_provider`

```rust
// 파일: crates/gadgetron-cli/src/main.rs (기존 #[cfg(test)] mod tests 에 추가)
#[test]
fn build_providers_vllm_config_returns_provider() {
    use gadgetron_core::config::{AppConfig, ProviderConfig, ServerConfig};
    use gadgetron_core::routing::RoutingConfig;
    use std::collections::HashMap;

    let mut providers = HashMap::new();
    providers.insert(
        "my-vllm".to_string(),
        ProviderConfig::Vllm {
            endpoint: "http://10.100.1.5:8100".to_string(),
            api_key: None,
        },
    );

    let cfg = AppConfig {
        server: ServerConfig {
            bind: "0.0.0.0:8080".to_string(),
            api_key: None,
            request_timeout_ms: 30_000,
        },
        router: RoutingConfig::default(),
        providers,
        nodes: vec![],
        models: vec![],
    };

    let result = build_providers(&cfg);
    assert!(result.is_ok(), "expected Ok, got {:?}", result.err());
    let map = result.unwrap();
    assert_eq!(map.len(), 1);
    assert!(map.contains_key("my-vllm"));
    assert_eq!(map["my-vllm"].name(), "vllm");
}
```

#### `build_providers_sglang_config_returns_provider`

```rust
#[test]
fn build_providers_sglang_config_returns_provider() {
    use gadgetron_core::config::{AppConfig, ProviderConfig, ServerConfig};
    use gadgetron_core::routing::RoutingConfig;
    use std::collections::HashMap;

    let mut providers = HashMap::new();
    providers.insert(
        "my-sglang".to_string(),
        ProviderConfig::Sglang {
            endpoint: "http://10.100.1.110:30000".to_string(),
            api_key: None,
        },
    );

    let cfg = AppConfig {
        server: ServerConfig {
            bind: "0.0.0.0:8080".to_string(),
            api_key: None,
            request_timeout_ms: 30_000,
        },
        router: RoutingConfig::default(),
        providers,
        nodes: vec![],
        models: vec![],
    };

    let result = build_providers(&cfg);
    assert!(result.is_ok());
    let map = result.unwrap();
    assert_eq!(map.len(), 1);
    assert!(map.contains_key("my-sglang"));
    assert_eq!(map["my-sglang"].name(), "sglang");
}
```

#### `build_providers_gemini_still_bails`

```rust
#[test]
fn build_providers_gemini_still_bails() {
    use gadgetron_core::config::{AppConfig, ProviderConfig, ServerConfig};
    use gadgetron_core::routing::RoutingConfig;
    use std::collections::HashMap;

    let mut providers = HashMap::new();
    providers.insert(
        "gemini".to_string(),
        ProviderConfig::Gemini {
            api_key: "dummy".to_string(),
            models: vec!["gemini-pro".to_string()],
        },
    );

    let cfg = AppConfig {
        server: ServerConfig {
            bind: "0.0.0.0:8080".to_string(),
            api_key: None,
            request_timeout_ms: 30_000,
        },
        router: RoutingConfig::default(),
        providers,
        nodes: vec![],
        models: vec![],
    };

    let result = build_providers(&cfg);
    assert!(result.is_err());
    let msg = format!("{}", result.unwrap_err());
    assert!(msg.contains("gemini"), "error should mention gemini, got: {msg}");
}
```

#### `chunk_delta_reasoning_content_deserializes`

```rust
// 파일: crates/gadgetron-core/src/provider.rs (기존 #[cfg(test)] 블록에 추가)
#[test]
fn chunk_delta_reasoning_content_deserializes() {
    let json = r#"{"role":"assistant","content":"Hello","reasoning_content":"Step 1: think"}"#;
    let delta: ChunkDelta = serde_json::from_str(json).unwrap();
    assert_eq!(delta.role.as_deref(), Some("assistant"));
    assert_eq!(delta.content.as_deref(), Some("Hello"));
    assert_eq!(delta.reasoning_content.as_deref(), Some("Step 1: think"));
}
```

#### `chunk_delta_missing_reasoning_content_is_none`

```rust
#[test]
fn chunk_delta_missing_reasoning_content_is_none() {
    let json = r#"{"content":"Hi"}"#;
    let delta: ChunkDelta = serde_json::from_str(json).unwrap();
    assert_eq!(delta.content.as_deref(), Some("Hi"));
    assert!(delta.reasoning_content.is_none());
}
```

#### `chunk_delta_serializes_without_reasoning_when_none`

```rust
#[test]
fn chunk_delta_serializes_without_reasoning_when_none() {
    let delta = ChunkDelta {
        role: None,
        content: Some("Hi".to_string()),
        tool_calls: None,
        reasoning_content: None,
    };
    let json = serde_json::to_string(&delta).unwrap();
    assert!(!json.contains("reasoning_content"),
        "key should be absent when None, got: {json}");
    assert!(json.contains("\"content\":\"Hi\""));
}
```

#### `message_reasoning_content_deserializes`

```rust
// 파일: crates/gadgetron-core/src/message.rs (기존 또는 신규 #[cfg(test)] 블록)
#[test]
fn message_reasoning_content_deserializes() {
    let json = r#"{"role":"assistant","content":"result","reasoning_content":"internal thoughts"}"#;
    let msg: crate::message::Message = serde_json::from_str(json).unwrap();
    assert_eq!(msg.reasoning_content.as_deref(), Some("internal thoughts"));
}
```

#### `message_constructors_set_reasoning_none`

```rust
#[test]
fn message_constructors_set_reasoning_none() {
    let u = crate::message::Message::user("hi");
    let s = crate::message::Message::system("sys");
    let a = crate::message::Message::assistant("ans");
    assert!(u.reasoning_content.is_none());
    assert!(s.reasoning_content.is_none());
    assert!(a.reasoning_content.is_none());
}
```

#### `message_missing_reasoning_content_is_none`

Input: `{"role":"assistant","content":"hello"}`  (no reasoning_content key)
Expected: `msg.reasoning_content.is_none() == true`

```rust
#[test]
fn message_missing_reasoning_content_is_none() {
    let json = r#"{"role":"assistant","content":"hello"}"#;
    let msg: crate::message::Message = serde_json::from_str(json).unwrap();
    assert!(msg.reasoning_content.is_none());
}
```

### 4.3 테스트 하네스

- mock 불필요: 단위 테스트는 네트워크 접근 없이 순수 Rust 데이터 구조와 `build_providers()` 의 인스턴스화만 검증한다.
- `VllmProvider` / `SglangProvider` 는 `Client::new()` 를 `new()` 에서 호출하나, 단위 테스트에서는 실제 HTTP 요청을 보내지 않으므로 문제없다.
- property-based test: 불필요. serde 직렬화/역직렬화는 특정 JSON 구조에 대한 결정론적 검증으로 충분하다.

### 4.4 커버리지 목표

- `build_providers()` 의 Vllm / Sglang / Gemini 분기 각 1회 이상 실행 (branch coverage 3/6 분기 신규).
- `ChunkDelta` serde 라인: 새 필드 포함 serialize/deserialize 각 1회.
- `Message` serde 라인: 새 필드 1회.
- Line coverage 목표: 신규 추가 라인 기준 100% (분기가 없는 단순 필드 추가이므로 달성 용이).

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 E2E 시나리오 — vLLM 비스트리밍

**전제**: PostgreSQL이 실행 중이고 마이그레이션이 적용됨. Gadgetron이 `gadgetron.toml` (§2.3) 으로 기동됨.

#### Step 1: PostgreSQL 기동

```bash
docker run -d \
  --name gadgetron-pg \
  -e POSTGRES_USER=gadgetron \
  -e POSTGRES_PASSWORD=gadgetron \
  -e POSTGRES_DB=gadgetron \
  -p 5432:5432 \
  postgres:16-alpine
```

`docker logs gadgetron-pg` 에서 `ready to accept connections` 확인 후 진행.

#### Step 2: 설정 파일 작성

```bash
cat > /tmp/gadgetron.toml << 'EOF'
[server]
bind = "0.0.0.0:8080"

[router]
strategy = "round_robin"
fallback_enabled = false

[providers.vllm-gemma]
type = "vllm"
endpoint = "http://10.100.1.5:8100"
EOF
```

#### Step 3: Gadgetron 기동

```bash
GADGETRON_CONFIG=/tmp/gadgetron.toml \
GADGETRON_DATABASE_URL="postgresql://gadgetron:gadgetron@localhost:5432/gadgetron" \
cargo run -p gadgetron-cli 2>&1 | tee /tmp/gadgetron.log
```

기대 로그 (순서 보장):
```
INFO gadgetron: database migrations applied
INFO gadgetron: provider registered name="vllm-gemma"
INFO gadgetron: listening addr=0.0.0.0:8080
```

#### Step 4: tenant + API 키 INSERT

```sql
-- 별도 터미널:
psql "postgresql://gadgetron:gadgetron@localhost:5432/gadgetron" << 'SQL'

-- (1) tenant 생성
INSERT INTO tenants (id, name, status)
VALUES ('00000000-0000-0000-0000-000000000001', 'test-tenant', 'Active');

-- (2) API 키 생성
-- raw key = "gad_live_sprint4testkey00000000000000"  (총 38자, 언더스코어 2개)
-- prefix = "gad_live"
-- SHA-256("gad_live_sprint4testkey00000000000000") 계산:
--   $ echo -n "gad_live_sprint4testkey00000000000000" | sha256sum
--   => 미리 계산: 실행 환경에서 직접 계산 후 아래 hash 값으로 교체
INSERT INTO api_keys (tenant_id, prefix, key_hash, kind, scopes, name)
VALUES (
    '00000000-0000-0000-0000-000000000001',
    'gad_live',
    -- sha256('gad_live_sprint4testkey00000000000000') — pre-computed:
    --   $ echo -n "gad_live_sprint4testkey00000000000000" | shasum -a 256
    --   39ca2707e580ba703865cd80992904a6be32d6194b9c9c9c1a5c893523b6a98e
    '39ca2707e580ba703865cd80992904a6be32d6194b9c9c9c1a5c893523b6a98e',
    'live',
    ARRAY['OpenAiCompat'],
    'sprint4-test-key'
);
SQL
```

실제 raw key: `gad_live_sprint4testkey00000000000000`

#### Step 5: 비스트리밍 curl

```bash
curl -s -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer gad_live_sprint4testkey00000000000000" \
  -d '{
    "model": "cyankiwi/gemma-4-31B-it-AWQ-4bit",
    "messages": [{"role": "user", "content": "Say hello in one word."}],
    "max_tokens": 10,
    "stream": false
  }' | jq .
```

기대 응답 shape (값은 모델 출력에 따라 다름):
```json
{
  "id": "chatcmpl-<uuid>",
  "object": "chat.completion",
  "created": <unix-timestamp>,
  "model": "cyankiwi/gemma-4-31B-it-AWQ-4bit",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "Hello."
      },
      "finish_reason": "stop"
    }
  ],
  "usage": {
    "prompt_tokens": <int>,
    "completion_tokens": <int>,
    "total_tokens": <int>
  }
}
```

검증 조건:
- HTTP 200 반환.
- `choices[0].message.content`가 non-empty string.
- `choices[0].finish_reason`이 `"stop"` 또는 `"length"`.
- `usage.total_tokens > 0`.

#### Step 6: 스트리밍 curl

```bash
curl -s -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer gad_live_sprint4testkey00000000000000" \
  --no-buffer \
  -d '{
    "model": "cyankiwi/gemma-4-31B-it-AWQ-4bit",
    "messages": [{"role": "user", "content": "Count from 1 to 5."}],
    "max_tokens": 50,
    "stream": true
  }'
```

기대 출력 형식 (각 줄이 SSE event):
```
data: {"id":"chatcmpl-<uuid>","object":"chat.completion.chunk","created":<ts>,"model":"cyankiwi/gemma-4-31B-it-AWQ-4bit","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}

data: {"id":"chatcmpl-<uuid>","object":"chat.completion.chunk","created":<ts>,"model":"cyankiwi/gemma-4-31B-it-AWQ-4bit","choices":[{"index":0,"delta":{"content":"1"},"finish_reason":null}]}

...

data: [DONE]
```

검증 조건:
- 첫 번째 `data:` 라인이 도착하기까지 첫 토큰 latency (TTFT) < 10초 (네트워크 및 모델 조건에 따라 다름).
- 마지막 줄이 `data: [DONE]` 으로 끝남.
- 모든 `data:` 라인이 `ChatChunk` JSON으로 파싱 가능:
  ```bash
  curl ... | grep "^data:" | grep -v "\[DONE\]" | sed 's/^data: //' | while read line; do
    echo "$line" | python3 -c "import json,sys; json.load(sys.stdin); print('OK')"
  done
  ```

### 5.2 E2E 시나리오 — SGLang 비스트리밍 + reasoning_content

```bash
curl -s -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer gad_live_sprint4testkey00000000000000" \
  -d '{
    "model": "GLM-5.1",
    "messages": [{"role": "user", "content": "What is 2+2? Show your reasoning."}],
    "max_tokens": 100,
    "stream": false
  }' | jq .
```

**주의**: 이 curl은 `http://localhost:8080` → `gadgetron-router` → `sglang-glm` provider (SGLang 10.100.1.110:30000) 로 라우팅된다. `gadgetron.toml`에 `[providers.sglang-glm]` 섹션이 있어야 한다 (§2.3 참조).

기대 응답: `choices[0].message` 에 `reasoning_content` 필드가 있을 수 있음:
```json
{
  "choices": [
    {
      "message": {
        "role": "assistant",
        "content": "4",
        "reasoning_content": "2 plus 2 equals 4."
      }
    }
  ]
}
```

`reasoning_content` 가 `null` 또는 absent인 경우도 정상 — 모델 출력에 따라 다름. 어느 경우에도 HTTP 200과 파싱 가능한 JSON을 기대한다.

### 5.3 테스트 환경

| 의존성 | 설정 | 확인 방법 |
|--------|------|----------|
| PostgreSQL 16 | `docker run` (§5.1 Step 1) | `pg_isready -h localhost -p 5432` |
| vLLM 서버 | `http://10.100.1.5:8100` (외부, 별도 기동됨) | `curl http://10.100.1.5:8100/health` → 200 |
| SGLang 서버 | `http://10.100.1.110:30000` (외부, 별도 기동됨) | `curl http://10.100.1.110:30000/health` → 200 |
| Gadgetron | `cargo run -p gadgetron-cli` | 포트 8080 listen 확인 |

CI 재현성: vLLM/SGLang 서버는 로컬 GPU 클러스터 의존이므로 CI에서 라이브 E2E 테스트를 실행할 수 없다. CI에서는 §4 단위 테스트만 실행한다. E2E는 개발자 로컬 또는 스테이징 환경에서 수동으로 실행한다. [P2] 에서 `gadgetron-testing` 의 `FakeVllmServer` (WireMock 또는 axum 기반 mock) 를 도입하여 CI 통합 예정.

### 5.4 회귀 방지

다음 변경이 발생하면 이 테스트들이 실패해야 한다:

| 변경 | 실패하는 테스트 |
|------|----------------|
| `ProviderConfig::Vllm` 분기를 다시 `bail!`로 되돌림 | `build_providers_vllm_config_returns_provider` |
| `ProviderConfig::Sglang` 분기를 다시 `bail!`로 되돌림 | `build_providers_sglang_config_returns_provider` |
| `ProviderConfig::Gemini` 분기를 `bail!`에서 실제 구현으로 교체 (Week 5-7 이전) | `build_providers_gemini_still_bails` |
| `ChunkDelta`에서 `reasoning_content` 필드 제거 | `chunk_delta_reasoning_content_deserializes` |
| `ChunkDelta.reasoning_content`에 `#[serde(default)]` 제거 | `chunk_delta_missing_reasoning_content_is_none` |
| `ChunkDelta.reasoning_content`에 `skip_serializing_if` 제거 | `chunk_delta_serializes_without_reasoning_when_none` |

---

## 6. Phase 구분

| 항목 | Phase |
|------|-------|
| `build_providers()` Vllm/Sglang 분기 활성화 | [P1] — 이 Sprint |
| `ChunkDelta.reasoning_content` 추가 | [P1] — 이 Sprint |
| `Message.reasoning_content` 추가 | [P1] — 이 Sprint |
| `ProviderConfig::Gemini` 활성화 | [P1] — Week 5-7 (D-20260411-02) |
| `endpoint` URL 형식 검증 (`url::Url::parse`) | [P2] |
| CI용 `FakeVllmServer` / `FakeSglangServer` mock | [P2] — `gadgetron-testing` 크레이트 (D-20260411-05) |
| SGLang `reasoning_content` TUI 표시 | [P2] |

---

## 7. 오픈 이슈 / 의사결정 필요

| ID | 내용 | 옵션 | 추천 | 상태 |
|----|------|------|------|------|
| Q-1 | SGLang non-streaming 응답에서 `reasoning_content`가 `ChatResponse.choices[].message`가 아닌 최상위 필드로 올 수 있음. 실제 GLM-5.1 응답 구조 확인 필요. | A. `Message`에 필드 추가 (현재 설계) / B. 최상위에 별도 필드 추가 | A — 구조 확인 전 보수적 접근 | 확인 필요 (E2E Step 5.2 실행 시 관찰) |

---

## 리뷰 로그 (append-only)

_(리뷰 미시작 — Draft 상태)_

---
