# 17 — Penny Claude Code LLM Gateway Settings

> **담당**: @gateway-router-lead
> **상태**: Implemented
> **작성일**: 2026-05-02
> **최종 업데이트**: 2026-05-02 — 구현 완료 (`BrainConfig` 확장, Admin API/UI, DB persistence, Penny hot-swap, Claude Code env/args)
> **관련 크레이트**: `gadgetron-core`, `gadgetron-penny`, `gadgetron-gateway`, `gadgetron-xaas`, `gadgetron-web`, `gadgetron-cli`
> **Phase**: [P2B] primary / [P2C] managed router follow-up
> **관련 문서**: `docs/design/phase2/02-penny-agent.md`, `docs/design/phase2/04-mcp-tool-registry.md`, `docs/design/phase2/13-penny-shared-surface-loop.md`, `docs/adr/ADR-P2A-05-agent-centric-control-plane.md`, `docs/adr/ADR-P2A-06-claude-code-subprocess.md`, `docs/manual/configuration.md`
> **외부 기준**: Claude Code LLM gateway / env vars / model config docs, vLLM OpenAI-compatible server docs, SGLang OpenAI-compatible APIs docs, Claude Code Router quick start

---

## 1. 철학 & 컨셉 (Why)

### 1.1 이 문서가 닫는 공백

Penny 는 현재 `gadgetron-penny`가 Claude Code CLI 를 subprocess 로 띄워 동작한다. 이 구조는 Claude Code 의 MCP/tool orchestration 을 그대로 재사용할 수 있어 빠르고 안정적이지만, 운영자가 Penny 의 underlying LLM 을 바꾸는 경로가 없다.

운영자가 지금 원하는 일은 크다.

1. Claude Code 의 모델을 문자열로 지정한다.
2. Claude Code 를 vLLM / SGLang 같은 OpenAI-compatible `/v1/chat/completions` 서버 뒤의 로컬 모델로 보낸다.
3. 이 설정을 UI Settings/Admin 에서 바꾸고, 가능하면 서버 재시작 없이 적용한다.
4. Codex/OpenCode worker 추가는 나중 문제로 둔다.

핵심 제약은 Claude Code 가 직접 기대하는 gateway 표면이다. Claude Code gateway 는 Anthropic Messages `/v1/messages` 계열, Bedrock, Vertex rawPredict 중 하나를 기대한다. vLLM/SGLang 은 OpenAI-compatible `/v1/chat/completions` 를 제공하므로, vLLM/SGLang endpoint 를 Claude Code 의 `ANTHROPIC_BASE_URL`에 직접 넣는 방식은 계약이 맞지 않는다. 따라서 1차 구현은 **CCR(Claude Code Router) 또는 동등한 Anthropic Messages gateway** 를 외부에 세우고, Gadgetron 은 Claude Code subprocess 에 올바른 env/args 를 주입하는 데 집중한다.

### 1.2 제품 비전과의 연결

`docs/00-overview.md §1`의 제품 방향은 Gadgetron 이 사용자의 작업 환경을 이해하고 조작하는 personal AI runtime 이 되는 것이다. Penny 의 brain 을 운영자가 바꿀 수 있어야 다음 단계가 열린다.

- 로컬 모델 실험: vLLM/SGLang 으로 서빙되는 사내/개인 모델을 Penny 에 연결한다.
- 비용/보안 통제: 외부 Claude Max, Anthropic API, 사내 gateway 를 배포 환경별로 선택한다.
- 후속 worker 확장: Claude Code, Codex, OpenCode 를 별도 coding worker 로 추가할 때도 같은 "runtime agent settings" 표면을 재사용한다.

### 1.3 고려한 대안과 채택하지 않은 이유

| 대안 | 장점 | 채택하지 않은 이유 |
|---|---|---|
| A. Claude Code 에 vLLM/SGLang `/v1`을 직접 연결 | 가장 단순해 보임 | Claude Code gateway 계약은 Anthropic Messages/Bedrock/Vertex 이다. OpenAI Chat Completions endpoint 만으로는 tool/use-streaming semantics 가 맞지 않는다. |
| B. Gadgetron 내부에 Anthropic Messages -> OpenAI Chat shim 구현 | 외부 서비스 없이 완결 | 메시지/stream/tool/result/count_tokens 변환을 우리가 소유하게 된다. Claude Code 변화에 취약하고, 첫 구현 범위가 커진다. |
| C. CCR/LiteLLM 같은 외부 gateway 를 operator-managed 로 사용 | 빠르고 검증 가능. Claude Code 가 권장하는 gateway 방식과 일치 | gateway 프로세스의 start/restart/status/logs 는 Gadgetron 밖에서 관리해야 한다. 1차 범위에서는 이 trade-off 를 수용한다. |
| D. Gadgetron 이 CCR sidecar 를 설치/기동/관리 | 사용자 경험은 가장 좋음 | 패키징, 업그레이드, credential 저장, 로그/프로세스 수명주기를 새로 소유해야 한다. P2C 로 미룬다. |

**채택: C — 외부 CCR-compatible gateway + Gadgetron Admin Settings + Claude Code subprocess env/args 주입.**

### 1.4 핵심 설계 원칙과 trade-off

1. **Penny runtime 은 유지한다.** 이번 변경은 Claude Code 를 다른 agent 로 교체하지 않는다. Claude Code 의 brain model/gateway 만 바꾼다.
2. **OpenAI-compatible local LLM 은 gateway 뒤에 둔다.** vLLM/SGLang 과 Claude Code 사이의 프로토콜 변환 책임은 CCR 같은 router 가 갖는다.
3. **핫 적용 범위는 "다음 Claude Code subprocess" 이다.** 현재 in-flight 요청은 기존 env/args 로 끝난다. 다음 Penny turn 부터 새 설정을 읽는다.
4. **시크릿 값은 Gadgetron DB/UI 에 저장하지 않는다.** Admin UI 는 env var 이름만 저장한다. 실제 token/key 값은 `gadgetron serve` 프로세스 환경에 둔다.
5. **관리자 전용이다.** LLM gateway/model 변경은 비용, 보안, 데이터 경계가 바뀌는 설정이므로 `/admin` subtree + `Management` scope 로만 연다.
6. **설정 파일 rewrite 는 1차 범위가 아니다.** 시작 시 TOML 기본값을 seed 로 쓰고, Admin 변경분은 DB runtime setting 으로 persistence 한다.

---

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API

#### 2.1.1 `gadgetron-core::agent::BrainConfig` 확장

기존 `BrainConfig` 는 `mode`, `external_anthropic_api_key_env`, `external_base_url`, `local_model`, `shim` 을 갖는다. 여기에 Claude Code 의 model/gateway 설정 필드를 추가한다.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainConfig {
    #[serde(default)]
    pub mode: BrainMode,

    #[serde(default = "default_external_anthropic_env")]
    pub external_anthropic_api_key_env: String,

    #[serde(default)]
    pub external_base_url: String,

    /// Optional Claude Code model string. Passed as `claude --model <model>`
    /// and `ANTHROPIC_MODEL=<model>` when non-empty.
    #[serde(default)]
    pub model: String,

    /// Optional process env var name whose value becomes
    /// `ANTHROPIC_AUTH_TOKEN` for gateway/proxy auth. The token value is
    /// never serialized, persisted, returned by API, or logged.
    #[serde(default)]
    pub external_auth_token_env: String,

    /// When true and `model` is non-empty, expose `model` to Claude Code as
    /// `ANTHROPIC_CUSTOM_MODEL_OPTION=<model>`. This is useful for gateway or
    /// local model ids that do not start with `claude` / `anthropic`.
    #[serde(default)]
    pub custom_model_option: bool,

    #[serde(default)]
    pub local_model: String,

    #[serde(default)]
    pub shim: BrainShimConfig,
}
```

`model` 은 사용자가 요청한 "문자열 모델"을 그대로 보존한다. Gadgetron 은 provider-specific model id 를 해석하지 않는다. 해석 책임은 Claude Code 와 gateway/CCR 에 있다.

#### 2.1.2 Runtime settings DTO

Admin API 는 `BrainConfig` 전체를 그대로 노출하지 않는다. shim/local-model forward compatibility 필드는 숨기고, 이번 기능의 설정만 wire type 으로 분리한다.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentBrainSettings {
    pub mode: BrainMode,
    pub external_base_url: String,
    pub model: String,
    pub external_auth_token_env: String,
    pub custom_model_option: bool,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub updated_by: Option<uuid::Uuid>,
    pub source: AgentBrainSettingsSource,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentBrainSettingsSource {
    ConfigFile,
    Database,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateAgentBrainSettingsRequest {
    pub mode: BrainMode,
    #[serde(default)]
    pub external_base_url: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub external_auth_token_env: String,
    #[serde(default)]
    pub custom_model_option: bool,
}
```

#### 2.1.3 Store trait

`gadgetron-gateway` 가 `gadgetron-xaas` 구현체에 의존하지 않도록 `gadgetron-core` 에 trait 를 둔다.

```rust
#[async_trait::async_trait]
pub trait AgentBrainSettingsStore: Send + Sync {
    async fn get_agent_brain_settings(
        &self,
        tenant_id: uuid::Uuid,
    ) -> Result<Option<AgentBrainSettings>>;

    async fn upsert_agent_brain_settings(
        &self,
        tenant_id: uuid::Uuid,
        actor_user_id: Option<uuid::Uuid>,
        request: UpdateAgentBrainSettingsRequest,
    ) -> Result<AgentBrainSettings>;
}
```

기존 admin users/billing/audit 와 같은 방향이다. Gateway 는 trait object 만 들고, Postgres 구현은 `gadgetron-xaas` 에 둔다.

#### 2.1.4 Gateway routes

두 endpoint 를 `/api/v1/web/workbench/admin` 아래에 추가한다. 기존 middleware 에 의해 `Management` scope 가 필요하다.

| Method | Path | Scope | 설명 |
|---|---|---|---|
| `GET` | `/api/v1/web/workbench/admin/agent/brain` | `Management` | 현재 Penny brain runtime settings 조회 |
| `PATCH` | `/api/v1/web/workbench/admin/agent/brain` | `Management` | 검증, DB upsert, `ArcSwap<BrainConfig>` 교체 |

에러 envelope 는 기존 workbench admin 패턴을 따른다.

```json
{
  "error": {
    "message": "agent brain settings store is not configured",
    "type": "workbench_error",
    "code": "agent_brain_settings_unavailable"
  }
}
```

### 2.2 내부 구조

#### 2.2.1 Hot apply substrate

기존 코드에는 live gadget mode 변경을 위해 `GatewayWorkbenchService.gadget_modes: Option<Arc<ArcSwap<GadgetsConfig>>>` 와 `PennyProvider` 쪽 registry reconfigure 경로가 있다. Brain 설정도 같은 운영 모델을 따른다.

```rust
pub struct GatewayWorkbenchService {
    // existing fields...
    pub agent_brain: Option<Arc<arc_swap::ArcSwap<gadgetron_core::agent::BrainConfig>>>,
    pub agent_brain_store: Option<Arc<dyn AgentBrainSettingsStore>>,
    pub agent_config_base: Option<Arc<gadgetron_core::agent::AgentConfig>>,
}

pub struct PennyProvider {
    // existing fields...
    brain_config: Option<Arc<arc_swap::ArcSwap<BrainConfig>>>,
}
```

`gadgetron-cli` startup 순서:

1. `AppConfig` 를 TOML 에서 로드하고 `AgentConfig::validate` 를 통과시킨다.
2. DB store 가 있으면 tenant 의 `agent_brain_settings` row 를 조회한다.
3. row 가 있으면 `AgentConfig.brain` 에 overlay 하고 다시 validation 한다.
4. `Arc<ArcSwap<BrainConfig>>` 를 생성한다.
5. 같은 handle 을 `PennyProvider` 와 `GatewayWorkbenchService` 에 전달한다.

요청 처리 순서:

1. `PennyProvider::chat_stream` 진입 시 base `AgentConfig` 를 clone 한다.
2. `brain_config.load_full()` 로 현재 snapshot 을 읽고 `config.brain` 에 overlay 한다.
3. 기존 gadget mode overlay 를 적용한다.
4. `ClaudeCodeSession::run` -> `build_claude_command_with_session` -> `build_claude_command_with_env` 가 새 brain snapshot 으로 env/args 를 구성한다.

이 방식은 `RwLock` 보다 기존 hot-reload 패턴과 일치한다. reader 는 lock 을 잡지 않고 snapshot 을 읽으며, PATCH writer 는 새 `Arc<BrainConfig>` 를 만들어 한 번에 교체한다.

#### 2.2.2 Persistence

새 migration 을 추가한다.

```sql
-- crates/gadgetron-xaas/migrations/20260502000001_agent_brain_settings.sql
CREATE TABLE agent_brain_settings (
    tenant_id UUID PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    mode TEXT NOT NULL,
    external_base_url TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL DEFAULT '',
    external_auth_token_env TEXT NOT NULL DEFAULT '',
    custom_model_option BOOLEAN NOT NULL DEFAULT FALSE,
    updated_by UUID REFERENCES users(id) ON DELETE SET NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

actual token 값은 저장하지 않는다. `external_auth_token_env` 는 env var 이름만 저장한다.

#### 2.2.3 Claude Code subprocess env/args

`crates/gadgetron-penny/src/spawn.rs` 를 확장한다.

```rust
fn apply_brain_mode_env(
    cmd: &mut Command,
    config: &AgentConfig,
    env: &dyn EnvResolver,
) -> Result<(), SpawnError> {
    match config.brain.mode {
        BrainMode::ClaudeMax => {}
        BrainMode::ExternalAnthropic => {
            // existing ANTHROPIC_API_KEY injection
            // optional ANTHROPIC_BASE_URL
        }
        BrainMode::ExternalProxy => {
            // required ANTHROPIC_BASE_URL
        }
        BrainMode::GadgetronLocal => return Err(SpawnError::GadgetronLocalNotFunctional),
    }

    if !config.brain.external_auth_token_env.is_empty() {
        let token = env.get(&config.brain.external_auth_token_env).unwrap_or_default();
        if token.is_empty() {
            return Err(SpawnError::MissingAuthToken {
                env_name: config.brain.external_auth_token_env.clone(),
            });
        }
        cmd.env("ANTHROPIC_AUTH_TOKEN", token);
    }

    if !config.brain.model.is_empty() {
        cmd.env("ANTHROPIC_MODEL", &config.brain.model);
        if config.brain.custom_model_option {
            cmd.env("ANTHROPIC_CUSTOM_MODEL_OPTION", &config.brain.model);
        }
    }

    Ok(())
}

fn apply_claude_args(
    cmd: &mut Command,
    config: &AgentConfig,
    mcp_config_path: &Path,
    allowed_tools: &[String],
) {
    cmd.arg("-p");
    if !config.brain.model.is_empty() {
        cmd.arg("--model").arg(&config.brain.model);
    }
    // existing flags...
}
```

`--model` 이 가장 명시적인 startup 선택이다. `ANTHROPIC_MODEL` 은 Claude Code 의 env-based fallback 과 status/debug 일관성을 위해 같이 주입한다. `ANTHROPIC_CUSTOM_MODEL_OPTION` 은 model picker/validation 우회가 필요한 gateway model id 를 위해 optional 로 둔다.

#### 2.2.4 API validation

Validation 은 `BrainConfig::validate_with_env` 에 모은다. PATCH handler 는 request 를 `BrainConfig` 로 overlay 한 뒤 같은 validator 를 호출한다.

Rules:

| Field | Rule |
|---|---|
| `mode` | `claude_max`, `external_anthropic`, `external_proxy` 허용. `gadgetron_local` 은 기존처럼 P2C 전까지 reject. |
| `external_base_url` | `external_proxy` 에서 필수. 비어있지 않으면 `http://` 또는 `https://` 로 시작. CR/LF/NUL 금지. |
| `model` | 비어있거나 1..=256 chars. CR/LF/NUL 금지. Shell escaping 은 필요 없지만 CLI arg injection 방지 차원에서 control char 금지. |
| `external_auth_token_env` | 비어있거나 `[A-Z_][A-Z0-9_]*`. 값이 설정되면 runtime env 에 존재해야 함. |
| `custom_model_option` | `true` 이면 `model` 이 비어있을 수 없음. |
| `external_anthropic_api_key_env` | 기존 rule 유지. PATCH API 에서는 노출하지 않으며 TOML 값만 사용. |

### 2.3 설정 스키마

TOML 은 기본 seed 이다. Admin UI 변경분이 있으면 DB row 가 우선한다.

```toml
[agent.brain]
# default: claude_max
mode = "external_proxy"

# CCR / LiteLLM / equivalent Anthropic Messages gateway root.
# Do NOT put vLLM/SGLang http://host:port/v1 here directly unless the
# service exposes Anthropic Messages /v1/messages.
external_base_url = "http://127.0.0.1:8080"

# Free-form Claude Code model string.
model = "openai/Qwen3-Coder-30B-A3B-Instruct"

# Optional env var name. Gadgetron reads its value at subprocess spawn and
# passes it as ANTHROPIC_AUTH_TOKEN. The actual secret is not persisted.
external_auth_token_env = "PENNY_CCR_AUTH_TOKEN"

# Add model to Claude Code custom model option for non-standard ids.
custom_model_option = true
```

Default values:

| Field | Default |
|---|---|
| `mode` | `claude_max` |
| `external_base_url` | `""` |
| `model` | `""` |
| `external_auth_token_env` | `""` |
| `custom_model_option` | `false` |

### 2.4 에러 & 로깅

#### 2.4.1 Errors

신규 `GadgetronError` variant 는 만들지 않는다. Config/API 검증 실패는 기존 `GadgetronError::Config` 와 workbench HTTP error envelope 로 매핑한다.

`SpawnError` 에는 subprocess 전용 에러를 추가한다.

```rust
pub enum SpawnError {
    // existing...
    MissingAuthToken { env_name: String },
}
```

User-facing examples:

| 상황 | HTTP / 로그 메시지 |
|---|---|
| non-admin PATCH | existing 403 Management scope error |
| no DB/store wired | `agent brain settings store is not configured` |
| proxy mode without URL | `agent.brain.external_base_url is required when brain.mode = 'external_proxy'` |
| token env missing | `agent.brain.external_auth_token_env "PENNY_CCR_AUTH_TOKEN" is not set in the environment` |

Secret value 는 절대 메시지에 포함하지 않는다.

#### 2.4.2 Tracing

| Target | Level | Fields |
|---|---|---|
| `agent_config` | `info` | `mode`, `has_model`, `has_base_url`, `has_auth_token_env`, `custom_model_option`, `source` |
| `workbench.admin.agent_brain` | `info` | `tenant_id`, `actor_user_id`, `mode`, `model_hash`, `source` |
| `penny_subprocess` | `debug` | `mode`, `has_model`, `has_base_url`, `has_auth_token`, `custom_model_option` |

`model_hash` 는 debugging 상관관계용으로 `sha256(model)` 앞 12 hex 만 남긴다. model id 자체가 민감하지 않은 경우도 많지만, 사내 deployment name 이 노출될 수 있어 admin write log 에서는 hash 만 둔다. subprocess debug 에도 token 값은 절대 기록하지 않는다.

#### 2.4.3 STRIDE threat model

| Category | Threat | Mitigation |
|---|---|---|
| Spoofing | 일반 사용자가 gateway/model 을 바꿔 비용/데이터 경계를 우회 | `/admin` subtree + `Management` scope. UI 도 admin role 에서만 노출. |
| Tampering | PATCH body 로 invalid URL/control char/model injection | shared validator, CR/LF/NUL 금지, URL scheme allowlist, env var name regex. |
| Repudiation | 누가 LLM gateway 를 바꿨는지 추적 불가 | `updated_by`, `updated_at` 저장. audit event follow-up 은 P2C 로 둘 수 있으나 admin log 에 actor 를 남긴다. |
| Information disclosure | token/key 값이 DB/API/log 에 노출 | env var 이름만 저장. 값은 spawn 직전 process env 에서 읽고 child env 로만 전달. |
| Denial of service | 잘못된 gateway URL 로 Penny 가 계속 실패 | PATCH validation + status GET + spawn error message. in-flight request 는 기존 snapshot 으로 완료. |
| Elevation of privilege | OpenAiCompat key 로 admin endpoint 호출 | existing scope middleware 가 `/api/v1/web/workbench/admin/*` 를 Management 로 제한. |

### 2.5 의존성

신규 third-party Rust crate 는 추가하지 않는다.

이미 workspace 에 있는 것을 재사용한다.

- `arc-swap`: hot apply snapshot.
- `chrono`, `uuid`: existing xaas/admin DTOs 와 같은 shape.
- `async-trait`: 기존 store trait 패턴과 동일하게 필요 시 사용.

CCR, LiteLLM, vLLM, SGLang 은 Gadgetron dependency 가 아니다. 운영자가 외부 프로세스로 설치/관리한다.

### 2.6 서비스 기동 / 제공 경로

이번 P2B 구현은 Gadgetron 의 runtime path 를 바꾸므로 운영 경로를 명시한다.

#### 2.6.1 Gadgetron

기존 경로를 유지한다.

```bash
cargo build --workspace
RUST_LOG=info,agent_config=debug,penny_subprocess=debug gadgetron serve --config gadgetron.toml
```

Status:

```bash
curl -H "Authorization: Bearer $ADMIN_KEY" \
  http://127.0.0.1:8080/api/v1/web/workbench/admin/agent/brain
```

Logs:

- `agent_config`: startup/runtime validation.
- `workbench.admin.agent_brain`: admin PATCH.
- `penny_subprocess`: child env/args shape, without secrets.

Stop/restart 는 기존 `gadgetron serve` 프로세스 관리 방식(systemd, launchd, foreground shell)을 따른다. 이 기능 자체는 Gadgetron restart 없이 적용된다.

#### 2.6.2 External CCR-compatible gateway

Gadgetron 은 CCR process 를 시작/중지하지 않는다. 운영자가 별도 경로로 관리한다.

Example:

```bash
ccr start
ccr status
ccr logs
ccr restart
```

CCR config 를 바꾼 경우 CCR 문서 기준으로 `ccr restart` 가 필요할 수 있다. Gadgetron Admin Settings 에서 `external_base_url`, `model`, `external_auth_token_env` 를 바꾼 것은 Gadgetron restart 없이 다음 Penny subprocess 부터 적용된다.

#### 2.6.3 vLLM/SGLang

vLLM/SGLang 은 OpenAI-compatible server 로 뜬다. 이 endpoint 는 CCR provider config 의 `api_base_url` 쪽에 들어가야 하며, Gadgetron 의 `external_base_url` 에 직접 들어가지 않는다.

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 데이터 흐름

```text
Admin UI
  |
  | GET/PATCH /api/v1/web/workbench/admin/agent/brain
  v
gadgetron-gateway
  | validate + Management scope
  | upsert
  v
gadgetron-xaas Postgres agent_brain_settings
  |
  | ArcSwap<BrainConfig>::store(new_snapshot)
  v
PennyProvider
  | per request: clone base AgentConfig + overlay live BrainConfig
  v
ClaudeCodeSession
  |
  v
spawn.rs build_claude_command_with_env
  | env: ANTHROPIC_BASE_URL, ANTHROPIC_AUTH_TOKEN,
  |      ANTHROPIC_MODEL, ANTHROPIC_CUSTOM_MODEL_OPTION
  | args: claude -p --model <model> ...
  v
Claude Code CLI
  |
  | Anthropic Messages gateway request
  v
CCR / Anthropic-compatible gateway
  |
  | OpenAI Chat Completions request
  v
vLLM / SGLang / other local LLM server
```

### 3.2 Crate responsibilities

| Crate | Responsibility |
|---|---|
| `gadgetron-core` | `BrainConfig` fields, validation, DTOs, store trait. No HTTP or DB. |
| `gadgetron-xaas` | Postgres migration + `AgentBrainSettingsStore` implementation. |
| `gadgetron-gateway` | Admin endpoints, scope guard reuse, request validation, `ArcSwap<BrainConfig>` swap. |
| `gadgetron-penny` | Per-turn live brain snapshot overlay, Claude Code env/args injection. |
| `gadgetron-web` | Admin page section for Penny LLM Gateway settings. |
| `gadgetron-cli` | Startup wiring: load TOML, overlay DB row, construct shared `ArcSwap`. |

This follows D-12 crate boundaries: shared config/contracts stay in `core`, persistence in `xaas`, HTTP in `gateway`, subprocess logic in `penny`, UI in `web`.

### 3.3 Interface contracts with other domains

| Domain | Contract |
|---|---|
| Security | Admin-only endpoint, env-name-only secret reference, no secret logging. |
| DX/Product | Existing `/web/admin` page gets a compact settings section. Saving applies to the next Penny turn; UI copy must state this. |
| QA | Fake env resolver and fake Claude binary can verify env/args without running real Claude Code or CCR. |
| Inference | vLLM/SGLang remain behind CCR-compatible gateway. Gadgetron does not implement OpenAI-to-Anthropic translation in P2B. |

### 3.4 Graph verification

Graphify was run before writing this design.

- `graphify-out/GRAPH_REPORT.md` reports 3642 nodes, 8792 edges, 153 communities.
- The report identifies `build_claude_command_with_env()` as a god node with 25 edges. This matches this design's decision to keep all Claude Code env/args mutation in `crates/gadgetron-penny/src/spawn.rs`.
- `graphify explain "build_claude_command_with_env()"` resolves to `spawn_build_claude_command_with_env`, mapped to `crates/gadgetron-penny/src/spawn.rs` around `build_claude_command_with_env`, with edges to `build_claude_command_with_session()` and spawn tests such as `build_claude_command_external_anthropic_injects_api_key` and `build_claude_command_external_proxy_injects_base_url_only`.
- `graphify query "How does PennyProvider reach Claude Code subprocess env construction and AgentConfig brain settings?" --budget 1800` returned the relevant `build_claude_command_with_env()` / `init_serve_runtime()` area but the BFS output was noisy.
- `graphify path "PennyProvider" "build_claude_command_with_env()"` returned a noisy inferred path through `.chat()` / `.ok()`. `graphify path "AgentConfig" "build_claude_command_with_env()"` found no path. Manual code inspection therefore remains authoritative for the concrete call chain:

```text
PennyProvider::chat_stream
  -> ClaudeCodeSession::run
  -> spawn_claude_process
  -> build_claude_command_with_session
  -> build_claude_command_with_env
  -> apply_base_env_allowlist / apply_brain_mode_env / apply_claude_args
```

Reviewer note: graphify confirms the hot node to modify, but current graph labels are not precise enough to prove the full dependency chain automatically. This document treats graphify as supporting evidence and records the manual inspection boundary.

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

`gadgetron-core`:

- `BrainConfig::validate_with_env` accepts empty `model` and rejects CR/LF/NUL.
- `external_proxy` requires `external_base_url`.
- `external_base_url` rejects unsupported schemes and control chars.
- `external_auth_token_env` accepts `PENNY_CCR_AUTH_TOKEN` and rejects lowercase/invalid names.
- `custom_model_option = true` rejects empty `model`.
- `gadgetron_local` remains rejected until P2C.

`gadgetron-penny`:

- `build_claude_command_with_env` passes `--model <model>` when `brain.model` is set.
- `ANTHROPIC_MODEL` is injected when `brain.model` is set.
- `ANTHROPIC_CUSTOM_MODEL_OPTION` is injected only when `custom_model_option = true`.
- `ANTHROPIC_AUTH_TOKEN` is resolved from `external_auth_token_env`.
- missing token env returns `SpawnError::MissingAuthToken { env_name }`.
- inherited `ANTHROPIC_*` process env remains stripped by `env_clear`; only allowlisted/resolved values are present.
- `ExternalProxy` still injects only base URL + optional auth/model, not `ANTHROPIC_API_KEY` unless explicitly configured through existing external Anthropic mode.

`gadgetron-gateway`:

- `GET /admin/agent/brain` requires `Management`.
- `PATCH /admin/agent/brain` requires `Management`.
- no store / no DB returns deterministic `agent_brain_settings_unavailable`.
- valid PATCH calls store, swaps `ArcSwap<BrainConfig>`, and response reflects source `database`.
- invalid PATCH does not swap current snapshot.

`gadgetron-xaas`:

- migration creates `agent_brain_settings`.
- upsert creates row for tenant.
- second upsert updates same row and `updated_at`.
- `updated_by` accepts admin user and becomes NULL if user is deleted.

`gadgetron-web`:

- Admin page loads current settings.
- Save sends PATCH body with `mode`, `external_base_url`, `model`, `external_auth_token_env`, `custom_model_option`.
- non-admin / 403 path keeps existing admin key override behavior.

### 4.2 테스트 하네스

- `FakeEnv` already exists in `gadgetron-core::agent::config`; reuse it for env var lookup tests.
- `tokio::process::Command::as_std().get_args()/get_envs()` pattern already exists in `gadgetron-penny/src/spawn.rs` tests; extend it.
- Gateway route tests can mirror existing workbench admin tests and construct `GatewayWorkbenchService` with `ArcSwap::from_pointee(BrainConfig::default())`.
- XaaS tests use the existing Postgres migration/test harness if available. If no shared Pg harness exists for this module, keep store tests behind the same feature/profile used by identity/users tests.
- Web tests use existing React test setup if present; otherwise keep first pass to TypeScript typecheck plus focused fetch helper tests.

Property-based tests are not required. The validation space is small and table-driven unit tests are clearer.

### 4.3 커버리지 목표

- Changed Rust lines: 85% line coverage in touched modules.
- Validation branches: 100% branch coverage for new `BrainConfig` rules.
- Spawn env/args: every new env var/arg has at least one positive and one negative test.
- Web UI: smoke/type coverage sufficient for P2B; full Playwright visual coverage is not required because this is an existing dense admin form, not a new frontend experience.

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

Primary integration path:

```text
gadgetron-cli startup
  -> gateway admin PATCH
  -> Postgres store
  -> ArcSwap brain snapshot
  -> PennyProvider request
  -> fake Claude Code binary receives env/args
  -> gateway returns chat stream
```

The integration test does not need a real CCR/vLLM instance to prove Gadgetron propagation. The fake Claude binary records environment and args, then emits minimal `stream-json` compatible output. A separate manual smoke covers real CCR.

### 5.2 E2E scenarios

1. Start test server with Postgres and fake `claude` binary first in `PATH`.
2. Admin `PATCH /api/v1/web/workbench/admin/agent/brain`:

```json
{
  "mode": "external_proxy",
  "external_base_url": "http://127.0.0.1:8080",
  "model": "openai/Qwen3-Coder-30B-A3B-Instruct",
  "external_auth_token_env": "PENNY_CCR_AUTH_TOKEN",
  "custom_model_option": true
}
```

3. Send `/v1/chat/completions` with Penny model.
4. Assert fake Claude observed:

```text
--model openai/Qwen3-Coder-30B-A3B-Instruct
ANTHROPIC_BASE_URL=http://127.0.0.1:8080
ANTHROPIC_AUTH_TOKEN=<test-token>
ANTHROPIC_MODEL=openai/Qwen3-Coder-30B-A3B-Instruct
ANTHROPIC_CUSTOM_MODEL_OPTION=openai/Qwen3-Coder-30B-A3B-Instruct
```

5. Send another PATCH with a different `model`.
6. Send a second Penny request and assert only the second fake Claude invocation sees the new model.
7. Assert first in-flight request remains successful and no shared state panic occurs.

### 5.3 테스트 환경

- Postgres with existing xaas migrations.
- Fake Claude binary/script in a temp dir.
- `PENNY_CCR_AUTH_TOKEN=test-token` in test process env or `FakeEnv` depending on test layer.
- No real vLLM/SGLang needed in CI.

### 5.4 회귀 방지

The tests must fail if:

- `ANTHROPIC_AUTH_TOKEN` leaks from parent env without explicit `external_auth_token_env`.
- `--model` is omitted when a model is configured.
- invalid PATCH mutates the live snapshot.
- non-Management clients can change settings.
- Gadgetron restart is required for a DB-backed PATCH to affect the next Penny subprocess.

### 5.5 운영 검증

Manual smoke, after implementation:

1. Start vLLM or SGLang OpenAI-compatible server.
2. Configure CCR provider to point at that server.
3. `ccr start`.
4. Start Gadgetron with `PENNY_CCR_AUTH_TOKEN` in the environment.
5. Open `/web/admin` as admin and save Penny LLM Gateway settings.
6. Call Penny through `/v1/chat/completions`.
7. Confirm:
   - Gadgetron logs show `mode=external_proxy`, `has_model=true`, `has_auth_token=true`.
   - CCR logs show a request routed to the intended provider/model.
   - vLLM/SGLang logs show `/v1/chat/completions`.
8. Change model in Admin UI and send another Penny request without restarting Gadgetron.
9. Confirm the new model appears in fake/CCR/vLLM logs for the new request.

---

## 6. Phase 구분

| Phase | Scope |
|---|---|
| [P2B] | `BrainConfig` model/auth fields, Admin GET/PATCH, DB persistence, `ArcSwap<BrainConfig>`, Claude Code env/args injection, Admin UI section. |
| [P2B] | Hot apply for next Claude Code subprocess. No Gadgetron restart for DB-backed Admin changes. |
| [P2B] | External CCR-compatible gateway documented as operator-managed dependency. |
| [P2C] | Gadgetron-managed CCR sidecar lifecycle (`build/start/stop/status/logs`) if we decide to own it. |
| [P2C] | Internal Anthropic Messages -> OpenAI Chat shim (`gadgetron_local`) if CCR is not enough. |
| [P2C] | Codex/OpenCode as separate coding workers, not replacements for Penny. |
| [P3] | Per-user/per-project model policy, allowlist, model capability metadata, audit event hardening. |

---

## 7. 오픈 이슈 / 의사결정 필요

| ID | 내용 | 옵션 | 추천 | 상태 |
|---|---|---|---|---|
| Q-1 | vLLM/SGLang 을 Claude Code 에 직접 연결할 것인가 | A. 직접 연결 / B. CCR-compatible gateway 사용 / C. 내부 shim | B | 승인됨 — 2026-05-02 사용자 대화 |
| Q-2 | 설정 적용에 Gadgetron restart 가 필요한가 | A. restart / B. 다음 subprocess 부터 hot apply | B | 승인됨 — restart 없이 가능하면 선호 |
| Q-3 | Settings 노출 범위 | A. 모든 사용자 / B. Admin only | B | 승인됨 — restart/운영 설정은 Admin only |
| Q-4 | CCR process 를 Gadgetron 이 관리할 것인가 | A. P2B 에서 관리 / B. 외부 operator-managed / C. 내부 shim | B | 승인됨 — P2B 범위 축소 |

현재 남은 의사결정 대기 항목은 없다. 후속 Phase 범위는 P2C/P3 항목으로 분리했다.

---

## 리뷰 로그 (append-only)

### Round 0 — 2026-05-02 — PM draft

**결론**: Draft v0.

**요약**:
- 사용자 요청을 "Claude Code 모델 문자열 + CCR-compatible gateway 연결 + Admin Settings hot apply" 로 축소.
- Codex/OpenCode worker, managed sidecar, internal shim 은 후속 Phase 로 분리.
- 기존 live gadget mode `ArcSwap` 패턴을 brain settings 로 확장하는 설계로 정리.

### Round 1 — 2026-05-02 — @gateway-router-lead @inference-engine-lead

**결론**: Pass

**체크리스트** (`03-review-rubric.md §1` 기준):
- [x] 인터페이스 계약 — Admin GET/PATCH, `AgentBrainSettingsStore`, `BrainConfig` extension 이 호출 경계를 명확히 나눈다.
- [x] 크레이트 경계 — config/trait 는 `core`, persistence 는 `xaas`, HTTP 는 `gateway`, subprocess 는 `penny`, UI 는 `web`.
- [x] 타입 중복 — 기존 `BrainConfig` 를 확장하고 별도 provider config 를 만들지 않는다.
- [x] 에러 반환 — 신규 `GadgetronError` variant 없이 `Config` + workbench error envelope 로 충분하다. Spawn-only failure 는 `SpawnError` 에 둔다.
- [x] 동시성 — 기존 hot reload substrate 인 `ArcSwap` 재사용. request 는 snapshot 을 읽고 PATCH 는 새 snapshot 을 store 한다.
- [x] 의존성 방향 — gateway 가 penny 에 의존하지 않고 shared `ArcSwap<BrainConfig>` 와 trait object 만 본다.
- [x] 그래프 검증 — `build_claude_command_with_env()` god node 확인. path query 한계는 문서에 기록됨.
- [x] Phase 태그 — P2B/P2C/P3 분리.
- [x] 레거시 결정 준수 — ADR-P2A-05/06 의 Claude Code subprocess path 를 유지한다.

**Reviewer notes**:
- @gateway-router-lead: `/admin` subtree 사용은 맞다. 일반 `/agent/modes` 처럼 사용자-facing endpoint 로 두면 비용/데이터 경계 설정이 너무 넓게 열린다.
- @inference-engine-lead: vLLM/SGLang direct 연결을 배제한 점이 중요하다. OpenAI Chat endpoint 는 Claude Code gateway contract 와 다르므로 P2B 에서 internal shim 을 만들면 범위가 폭발한다.

**Action Items**:
- 없음.

**다음 라운드 조건**: Round 1.5 진행.

### Round 1.5 — 2026-05-02 — @security-compliance-lead @dx-product-lead

**결론**: Pass

**보안 체크리스트** (`03-review-rubric.md §1.5-A` 기준):
- [x] 위협 모델 — STRIDE 표 포함.
- [x] 신뢰 경계 입력 검증 — URL, model, env var name validation 명시.
- [x] 인증·인가 — Management scope + Admin UI only.
- [x] 시크릿 관리 — env var name only, token value never persisted/logged.
- [x] 공급망 — 신규 Rust dependency 없음. CCR/vLLM/SGLang 은 external operator dependency 로 문서화.
- [x] 암호화 — 자체 crypto 없음. HTTPS gateway URL 허용, HTTP loopback/local 실험도 허용.
- [x] 감사 로그 — `updated_by`, `updated_at`; full audit event 는 P3 hardening 으로 열어둠.
- [x] 에러 정보 누출 — env name 은 노출 가능, token value/path/stack 은 노출 금지.
- [x] LLM 특이 위협 — protocol boundary 를 CCR 로 분리. Penny prompt/tool policy 는 기존 docs 유지.
- [x] 컴플라이언스 매핑 — SOC2 CC6.x access control / CC7.x change trace 에 해당.

**사용성 체크리스트** (`03-review-rubric.md §1.5-B` 기준):
- [x] 사용자 touchpoint — `/web/admin` section + API + TOML seed + logs.
- [x] 에러 메시지 3요소 — 무엇/왜/수정 방법이 가능한 메시지로 설계.
- [x] CLI flag — 신규 CLI flag 없음.
- [x] API 응답 shape — workbench admin error envelope 유지.
- [x] config 필드 — defaults/validation/TOML 예시 명시.
- [x] defaults 안전성 — default `claude_max`, no external gateway, no token.
- [x] 문서 5분 경로 — CCR start + Admin save + Penny call smoke 절차 포함.
- [x] runbook playbook — status/logs/smoke 포함.
- [x] 하위 호환 — 기존 TOML 은 추가 field default 로 계속 동작.
- [x] i18n 준비 — Admin copy 는 기존 page style 을 따른다. Full i18n framework 는 현 web 앱 범위 밖.

**Reviewer notes**:
- @security-compliance-lead: 실제 token 값을 DB 에 저장하지 않는 결정은 유지해야 한다. UI 에 "env var name" 임을 분명히 표시한다.
- @dx-product-lead: "저장 즉시 현재 응답이 바뀐다"가 아니라 "다음 Penny turn 부터 적용"임을 UI helper copy 에 명시해야 한다.

**Action Items**:
- 없음.

**다음 라운드 조건**: Round 2 진행.

### Round 2 — 2026-05-02 — @qa-test-architect

**결론**: Pass

**체크리스트** (`03-review-rubric.md §2` 기준):
- [x] 단위 테스트 범위 — core validation, penny spawn, gateway handlers, xaas store, web helper 를 구분.
- [x] mock 가능성 — `FakeEnv`, fake Claude binary, ArcSwap fixture 로 외부 Claude/CCR/vLLM 없이 검증 가능.
- [x] 결정론 — time-sensitive assertion 은 `updated_at` presence/ordering 정도만 확인. network timing 없음.
- [x] 통합 시나리오 — Admin PATCH -> Penny request -> fake Claude env/args 기록.
- [x] CI 재현성 — Postgres + fake binary 로 충분. real CCR/vLLM 은 manual smoke.
- [x] 성능 검증 — per-request overhead 는 `ArcSwap::load_full()` + clone 이며 benchmark 없이 unit boundary 에서 충분. 필요하면 P2C 에 micro-bench.
- [x] 회귀 테스트 — env leak, auth bypass, invalid PATCH mutation, missing model arg 모두 실패 조건 명시.
- [x] 테스트 데이터 — no snapshot fixture required.

**Reviewer notes**:
- Fake Claude binary 로 "Gadgetron 이 Claude Code 에 무엇을 넘겼는가"를 검증하는 것이 맞다. Real CCR/vLLM 을 CI 에 넣으면 flake 와 setup cost 가 크다.
- PATCH 후 "두 번째 요청만 새 model" 시나리오가 hot apply 회귀를 잘 잡는다.

**Action Items**:
- 없음.

**다음 라운드 조건**: Round 3 진행.

### Round 3 — 2026-05-02 — @chief-architect

**결론**: Pass

**체크리스트** (`03-review-rubric.md §3` 기준):
- [x] Rust 관용구 — existing config validation + trait object store + `Result<T, GadgetronError>` 패턴 유지.
- [x] 제로 비용 추상화 — hot path 에 async trait 없음. request path 는 `ArcSwap` snapshot read 와 config clone 만 수행.
- [x] 제네릭 vs 트레이트 객체 — gateway/xaas 경계는 기존 admin store pattern 처럼 trait object 가 적절하다.
- [x] 에러 전파 — config error context 가 string 으로 충분하다. secret value 는 포함하지 않는다.
- [x] 수명주기 — shared handles 는 `Arc` 로 AppState/PennyProvider 에 소유권 공유.
- [x] 의존성 추가 — 신규 crate 없음.
- [x] 트레이트 설계 — store trait 는 small surface. P2C audit hooks 를 넣어도 breaking change 없이 확장 가능.
- [x] 관측성 — target/fields 가 운영 debugging 에 충분하고 secret-safe 하다.
- [x] hot path — no locks, no network, no DB read in Penny request path.
- [x] 문서화 — 공개 config fields 에 rustdoc 계획 포함.

**Reviewer notes**:
- DB row 를 request 마다 읽지 않고 startup/PATCH 시 `ArcSwap` 에 반영하는 결정이 중요하다.
- `BrainConfig` 에 field 를 추가하는 방식은 단기적으로 가장 작다. 별도 `AgentBrainRuntimeConfig` type 을 core public config 로 올리면 중복이 커진다.
- `external_auth_token_env` 는 `ExternalProxy` 뿐 아니라 `ExternalAnthropic` 과도 조합 가능하게 두는 편이 Claude Code auth precedence 와 맞다.

**Action Items**:
- 없음.

**다음 라운드 조건**: PM 최종 승인.

### 최종 승인 — 2026-05-02 — PM

**결론**: Approved.

**승인 범위**:
- P2B 에서는 외부 CCR-compatible gateway 를 사용한다.
- Admin Settings 는 DB-backed runtime setting 으로 저장하고, 다음 Claude Code subprocess 부터 hot apply 한다.
- Gadgetron 은 token 값을 저장하지 않고 env var 이름만 저장한다.
- Codex/OpenCode worker 와 managed CCR sidecar 는 후속 Phase 로 분리한다.

### 구현 완료 — 2026-05-02 — PM

**결론**: Implemented.

**구현 범위**:
- `BrainConfig` 에 `model`, `external_auth_token_env`, `custom_model_option` 추가.
- Claude Code subprocess 에 `--model`, `ANTHROPIC_MODEL`, `ANTHROPIC_CUSTOM_MODEL_OPTION`, `ANTHROPIC_AUTH_TOKEN` 주입.
- `agent_brain_settings` Postgres migration + xaas persistence helper 추가.
- `/api/v1/web/workbench/admin/agent/brain` GET/PATCH 추가.
- `/web/admin` 에 Penny LLM Gateway 설정 섹션 추가.
- `ArcSwap<BrainConfig>` 로 다음 Penny 요청부터 hot apply.
