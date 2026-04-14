# ADR-P2A-05 — Agent-Centric Control Plane + MCP Tool Registry

| Field | Value |
|---|---|
| **Status** | ACCEPTED (stub — detailed design in `docs/design/phase2/04-mcp-tool-registry.md` v2) |
| **Date** | 2026-04-14 |
| **Author** | PM (Claude), user-directed |
| **Parent** | `docs/process/04-decision-log.md` **D-20260414-04** |
| **Blocks** | `docs/design/phase2/04-mcp-tool-registry.md` (authoring), Phase 2C infra tool rollout, Phase 3 scheduler/cluster tool rollout |
| **Supersedes (partial)** | `docs/design/phase2/00-overview.md §1 + §2` (하방/상방 프레이밍 → Agent-Centric), §3 "Explicit non-goals" Anthropic `/v1/messages` 조항 (local shim 으로 조건부 reopen) |
| **Amended by** | **ADR-P2A-06** (2026-04-14) — §(d) "승인 카드 UX — 채팅 입력 금지, UI 카드 필수" and §(e) "서버 측 approval 흐름 — SSE + `/v1/approvals/{id}`" are **deferred to Phase 2B**. Four pre-impl reviews on `04 v1` returned 24 combined blockers; ~15 concentrated on the cross-process approval bridge (SEC-MCP-B1), race-free state machine (SEC-MCP-B7/CA-MCP-B5), and scope middleware collision (SEC-MCP-B4). Per user direction 2026-04-14, the approval flow is deferred to P2B with a fresh design round. The control-plane scaffold (trait, registry, `AgentConfig`, brain modes, `agent.*` reservation) ships as planned in `04 v2`. |

---

## Context

Phase 2 의 기존 프레이밍은 "하방(Phase 1 LLM 오케스트레이션 인프라) + 상방(Kairos 지식 레이어 기반 개인 비서)" 로 두 레이어가 동등하게 존재했다. Kairos 는 wiki + 웹 검색 도구를 가진 한 개의 LlmProvider 로서 라우터 provider map 에 다른 provider 와 병렬 등록되었다. 인프라 제어(노드 상태, GPU 이용률, 라우팅 전략, 클러스터 관리) 는 운영자가 TUI·HTTP API 로 별도 제어하는 별개 경로였다.

2026-04-14 사용자 지시로 이 구조가 재정렬된다:

1. **에이전트 = 플랫폼의 브레인이자 중추**. Claude Code CLI 는 "하나의 모델" 이 아니라 Gadgetron 이 제공하는 대화형 경험의 단일 진입점
2. **모든 입력 → 에이전트**. 기본 사용 동선에서 사용자 입력은 에이전트에게 전달되고, 에이전트가 도구를 선택·조합하여 응답 생성
3. **인프라는 에이전트의 도구**. Phase 1 의 router/provider/node/scheduler/cluster 관리 기능은 에이전트가 호출하는 MCP 도구로 재노출
4. **확장 가능한 tool registry**. P2A 에서부터 stable plugin 인터페이스로 설계해, P2B(inference tools) / P2C(infra tools) / P3(scheduler/cluster tools) 를 단순 추가로 처리

우회 경로(사용자 편의성): 사용자가 `POST /v1/chat/completions` 에 `model=vllm/llama3` 같이 직접 지정하면 에이전트를 통과하지 않고 바로 provider 로. `gadgetron-web` 드롭다운에서 `kairos` 가 기본이지만 다른 모델도 선택 가능.

브레인 모델 선택: 에이전트의 추론에 사용되는 LLM 을 **운영자** 가 4가지 모드 중 선택. `gadgetron_local` 모드에서는 Gadgetron 자체 인프라의 로컬 모델을 Claude Code 의 브레인으로 사용 (내부 Anthropic shim 을 통해).

## Decision

### (a) 플랫폼 서사 재정렬

- `docs/design/phase2/00-overview.md §1 (Purpose)`, `§2 (Core Insight)`, `§13 (Roadmap)` 를 Agent-Centric 으로 재작성
- 비전 한 줄: **"Claude Code is the brain. Everything else is a tool."**
- 하방/상방 프레이밍은 구조적 설명 (2 계층) 이 아니라 기능적 설명 (지식 도구 카테고리 + 인프라 도구 카테고리) 으로 전환

### (b) `McpToolProvider` trait — stable plugin 인터페이스

```rust
#[async_trait]
pub trait McpToolProvider: Send + Sync + 'static {
    /// 카테고리 (namespace 구분용): "knowledge" | "infrastructure" | "scheduler" | "cluster" | "custom"
    fn category(&self) -> &'static str;
    /// 이 provider 가 노출하는 도구 스키마 목록
    fn tool_schemas(&self) -> Vec<ToolSchema>;
    /// 도구 호출 dispatch. `args` 는 JSON object.
    async fn call(&self, name: &str, args: serde_json::Value) -> Result<ToolResult, McpError>;
    /// Cargo feature flag 등으로 provider 가 비활성일 수 있음
    fn is_available(&self) -> bool { true }
}

pub struct ToolSchema {
    pub name: String,          // namespaced: "wiki.read", "infra.list_nodes"
    pub tier: Tier,            // T1 / T2 / T3
    pub description: String,
    pub input_schema: serde_json::Value, // JSON Schema
}

pub enum Tier { Read, Write, Destructive }
```

P2A 에서 첫 구현체는 `gadgetron-knowledge::mcp::KnowledgeToolProvider` (category = "knowledge"). P2B 에서 `InferenceToolProvider` (list_models, call_provider), P2C 에서 `InfraToolProvider` (list_nodes, deploy_model), P3 에서 `SchedulerToolProvider` + `ClusterToolProvider` 가 같은 trait 의 추가 구현체로 landing.

### (c) 3-tier × 3-mode 권한 모델

Tier 는 tool 개발자가 `ToolSchema.tier` 로 선언; Mode 는 운영자가 `gadgetron.toml` 에서 선택.

- **T1 Read** — 항상 `auto`. 설정 불가.
- **T2 Write** — 기본 `ask`. 운영자가 서브카테고리별로 `auto` / `ask` / `never` override.
- **T3 Destructive** — 기본 `enabled = false` (= `never`). 활성화해도 **mode 는 항상 `ask`** (cardinal rule, config validation 에서 `auto` 거부).

### (d) 승인 카드 UX — 채팅 입력 금지, UI 카드 필수

- `gadgetron-web` 에 `<ApprovalCard>` 컴포넌트 도입
- SSE 이벤트 `gadgetron.approval_required` 로 카드 트리거
- T2 카드: "Allow once / Allow always / Deny"
- T3 카드: 빨간 보더 + "CANNOT be undone" 문구 + rate limit 잔여 표시, **"Allow always" 버튼 영구 금지**
- Timeout 60초 → auto-deny, rate limit counter 증가 없음
- API SDK 소비자는 `AgentAutoApproveT2` scope 로 T2 자동 승인 가능; T3 은 여전히 사람이 필요 (UI 없으면 해당 호출 실패)

### (e) 서버 측 approval 흐름 — SSE + `/v1/approvals/{id}`

- `gadgetron-kairos::approval::ApprovalRegistry` 에 `DashMap<ApprovalId, oneshot::Sender<Decision>>`
- MCP server 가 `ask` mode tool 요청 시 registry 에 enqueue → SSE event emit → `oneshot::Receiver` 대기
- 프론트엔드 → `POST /v1/approvals/{id} { decision }`
- Gateway → `ApprovalRegistry::decide(id, decision)` → tool 실행 또는 거부
- 각 단계별 감사 로그 (`ToolApprovalRequested` / `Granted` / `Denied` / `Timeout`)

### (f) 브레인 모델 선택

`[agent.brain]` config:

- `mode = "claude_max"` — 사용자 `~/.claude/` OAuth (기본)
- `mode = "external_anthropic"` — 외부 Anthropic API key + base URL
- `mode = "external_proxy"` — 사용자 운영 LiteLLM 등
- `mode = "gadgetron_local"` — Gadgetron 내부 `/internal/agent-brain/v1/messages` Anthropic shim 을 통해 로컬 provider 로 라우팅

`gadgetron_local` 모드 구현은 **옵션 D — 최소 내부 shim**:
- Loopback-only 바인딩
- 시작 시 32바이트 랜덤 토큰 생성, Claude Code subprocess 의 `ANTHROPIC_API_KEY` env 로 전달, 메모리에만 존재, 재시작 시 rotation
- Rust 네이티브 Anthropic ↔ OpenAI 프로토콜 번역기 (`messages` / `system` / `tools` / 스트림 이벤트만 커버; 이미지/PDF/cache_control 은 Phase 3)
- 재귀 방지: config validation 거부 + 요청 태그 (`internal_call = true` → `KairosProvider` dispatch 제외) + recursion depth 헤더 (≥ 2 거부)
- 쿼터: 사용자 쿼터는 최상위 `/v1/chat/completions` 만 차감; 브레인 호출은 별도 `agent_brain` audit 카테고리로 기록하되 쿼터 미차감

### (g) 에이전트는 자기 브레인을 선택할 수 없다 (scope boundary)

- MCP 도구 registry 에 `agent.set_brain` / `agent.list_brains` / `agent.switch_model` / `agent.read_config` / `agent.write_config` **영구 제외**
- `[agent.brain]` 은 운영자 전용 config. 변경은 Gadgetron 재시작 (또는 hot-reload 구현 시에도 에이전트가 트리거할 수 없음) 필수
- 인프라 읽기 도구 (`list_providers`, `list_models`) 는 제공하되, 브레인으로 선택된 모델은 결과에 플래그로 표시되거나 옵션으로 숨김 (detail in `04-mcp-tool-registry.md`)

**근거**: 프롬프트 인젝션 공격 벡터 차단. 에이전트가 자신의 권한 구성(어떤 모델, 어떤 도구)을 스스로 바꿀 수 있다면, 한 번의 프롬프트 조작으로 "더 제약 없는 모델 + 더 많은 도구" 로 승격될 수 있음. Cardinal rule: 메타-조작은 에이전트 외부에서만.

### (h) 환경설정은 유저 명시적 — auto-detect 금지

- `gadgetron.toml` 이 유일한 진실 공급원
- `kairos init` 이 대화형으로 운영자에게 브레인 모드를 묻되, 자동 탐지(`~/.claude/` 존재 여부 등)로 기본값을 "똑똑하게" 바꾸지 않음. 기본은 항상 `claude_max` 이고, 다른 선택지는 운영자가 직접 타이핑
- 모호성은 에러로 드러나야 함 — 조용히 동작하지 않음

## Alternatives considered

### Brain model selection — 4가지 구현 옵션

| Option | 요지 | 판결 |
|---|---|---|
| A. 외부 프록시 위임 | `kairos.claude_base_url` 로 사용자가 LiteLLM 직접 운영 | ❌ 단일 바이너리 철학 + "자체 제공하는 로컬 모델" 요구사항과 충돌 |
| B. LiteLLM 을 compose sibling 으로 번들 | docker-compose 에 Python 프록시 추가 | ❌ OpenWebUI 제거(D-20260414-02)와 일관성 없음, sibling Python 프로세스 |
| C. 완전한 `/v1/messages` Anthropic 호환 구현 | Gateway 가 Anthropic 프로토콜 모든 표면 구현 | ⏸ Phase 3+ 유보. P2A 에는 과도한 스코프 |
| D. **최소 내부 shim (선택)** | `/internal/agent-brain/v1/messages` + 최소 번역기 + 기존 router 재사용 | ✅ 단일 바이너리 유지, 점진적 확장, 재사용 극대화 |

### Approval UX — 채팅 입력 vs UI 카드

- 채팅 입력 ("yes" 타이핑): 거부. 에이전트가 사용자의 "yes" 를 오인/조작하는 여지, 시각적 구분 부족, 클라이언트 자동화 공격 벡터
- UI 카드 (선택): 명확한 버튼, Deny 가 기본(auto-deny on timeout), 시각적 T2/T3 차별화, 감사 로그에 사용자 intent 가 명확히 기록

## Consequences

### Immediate (이 ADR 의 pre-merge gate)

- `docs/design/phase2/04-mcp-tool-registry.md` 작성 완료
- `docs/design/phase2/00-overview.md §1 + §2 + §13` Agent-Centric 재작성
- `docs/design/phase2/03-gadgetron-web.md §12 + §17` 에 승인 카드 UX 반영
- `gadgetron-core::config::{AgentConfig, ToolsConfig, BrainConfig}` 타입 추가 + `AppConfig::agent` 필드

### Phase 2A 구현 중

- `#10` (MCP server) 가 `McpToolProvider` trait 기반으로 구현 — `KnowledgeToolProvider` 가 첫 구현체
- `#13`-`#15` (Kairos session/stream/provider) 가 `ApprovalRegistry` 통합
- Gateway `POST /v1/approvals/{id}` 엔드포인트 추가 (`#5` 후속 확장)
- `gadgetron-web` `<ApprovalCard>` 컴포넌트 + SSE 이벤트 파서 (`#4` 후속 확장)

### Phase 2C / Phase 3 유보

- `InfraToolProvider` (T2 infra_write) — `list_nodes`, `deploy_model`, `set_routing_strategy` 등
- `SchedulerToolProvider` (slurm, k8s jobs)
- `ClusterToolProvider` (kubectl, helm)
- `gadgetron_local` 브레인 모드 활성화 (Anthropic shim 구현)
- Anthropic 프로토콜 완전 호환 (Option C 경로)

## Verification

1. `docs/design/phase2/04-mcp-tool-registry.md` 가 존재하고 `McpToolProvider` trait + `ToolSchema` + `Tier` 를 명시
2. `gadgetron_core::config::AgentConfig` 가 `destructive.default_mode = "auto"` 설정 시 `validate()` 에러
3. `gadgetron_core::config::AgentConfig` 가 `brain.mode = "gadgetron_local"` + `brain.local_model = "kairos"` 시 `validate()` 에러 (재귀 방지)
4. MCP 도구 registry 에 `agent.set_brain` / `agent.read_config` / `agent.write_config` 가 존재하지 않음 (grep `McpToolProvider` 구현체)
5. `<ApprovalCard>` 컴포넌트의 T3 렌더 경로가 `Allow always` 버튼을 조건부 숨김 (`if tier === "T3"`)
6. `gadgetron-web` 의 브라우저 `localStorage.gadgetron_web_auto_approve` 가 T3 tool 을 저장하지 않음 (프론트엔드 코드 가드)

## Sources

- D-20260414-04 decision log entry (this ADR's parent)
- 사용자 지시 2026-04-14 (3 차 interaction)
- ADR-P2A-01/02 — `--allowed-tools` enforcement + `--dangerously-skip-permissions` risk acceptance (foundation for Layer 2)
- M6 tools_called audit — 확장됨 by §(d)
- D-20260411-10 Scope enum — `AgentApproval` variant 추가 예정
