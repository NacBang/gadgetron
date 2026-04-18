# 15 — Penny Chat Bootstrap Injection & Resume Boundary

> **담당**: @gateway-router-lead
> **상태**: Approved
> **작성일**: 2026-04-18
> **최종 업데이트**: 2026-04-18
> **관련 크레이트**: `gadgetron-gateway`, `gadgetron-penny`, `gadgetron-core`, `gadgetron-router`, `gadgetron-xaas`
> **Phase**: [P2B] primary / [P2C] request-scoped workbench gadget bridge hardening
> **관련 문서**: `docs/design/phase2/02-penny-agent.md`, `docs/design/phase2/10-penny-permission-inheritance.md`, `docs/design/phase2/13-penny-shared-surface-loop.md`, `docs/design/phase2/14-penny-retrieval-citation-contract.md`, `docs/design/gateway/route-groups-and-scope-gates.md`, `docs/reviews/pm-decisions.md` D-6/D-7/D-11/D-12/D-13, `docs/process/04-decision-log.md` D-20260418-15
> **보정 범위**: 2026-04-18 기준 `origin/main` 은 `PennyTurnContextAssembler`, `render_penny_shared_context()`, `SharedContextConfig`, `WorkbenchAwarenessGadgetProvider` 를 이미 landed 했지만, 이 산출물을 실제 `/v1/chat/completions` ingress 에 어떻게 주입하고 native session resume 과 어떻게 접합하는지는 trunk 에 authoritative doc 가 없다. 본 문서는 그 runtime seam 만 고정한다. `13-penny-shared-surface-loop.md` 가 넓은 product contract 이고, 본 문서는 그것의 PSL-1b execution slice 다.

---

## 1. 철학 & 컨셉 (Why)

### 1.1 이 문서가 닫는 공백 [P2B]

`origin/main` 의 최신 상태는 PSL-1 contract slice 를 이미 상당 부분 landed 했다.

1. `gadgetron-core::agent::shared_context` 가 `PennyTurnBootstrap` 및 digest 타입을 제공한다.
2. `gadgetron-gateway::penny::shared_context` 가 `DefaultPennyTurnContextAssembler` 와 `render_penny_shared_context()` 를 제공한다.
3. `gadgetron-penny::workbench_awareness` 가 `workbench.*` gadget schema 를 정의한다.
4. `docs/design/phase2/13-penny-shared-surface-loop.md` 가 "resume 는 conversational convenience, shared surface 는 operator truth" 라는 product rule 을 Approved 로 고정했다.

하지만 아직 trunk 에는 다음 질문에 대한 독립 설계 계약이 없다.

- `/v1/chat/completions` 요청에서 **언제** bootstrap 을 조립하는가
- bootstrap text 를 **어느 메시지에 어떤 형식으로** 주입하는가
- `--resume` turn 에서 왜 같은 규칙이 계속 적용되어야 하는가
- shared context 가 부분 실패한 경우 chat 요청을 어떻게 degraded continuation 시키는가
- non-Penny 모델과 Penny 모델이 같은 handler 를 공유할 때 어디서 분기하고 무엇을 절대 바꾸면 안 되는가

이 seam 이 열려 있으면 네 가지 회귀가 즉시 생긴다.

1. `render_penny_shared_context()` 는 존재하지만 실제 Penny subprocess 입력에는 도달하지 않을 수 있다.
2. native resume turn 이 "이전 session 이 있으니 이미 알고 있겠지" 라는 잘못된 최적화로 bootstrap 재조립을 생략할 수 있다.
3. shared context 를 별도 `System` message 로 붙이면 `NativeResumeTurn` 경로에서 `build_stdin_payload()` 가 마지막 user message 만 보내는 현재 규칙과 충돌해, resume turn 에서 context 가 조용히 사라질 수 있다.
4. shared context 를 요청 body 바깥 비공식 메모리에 넣어두면 `13` 문서가 금지한 hidden memory path 가 다시 생긴다.

즉, recent mainline 이 노출한 가장 큰 문서 공백은 다음 한 문장으로 요약된다.

> **Penny shared-context bootstrap 을 gateway ingress 에서 어떤 request rewrite 규칙으로 붙이고, 그 규칙이 stateless / first-turn / resume-turn 모두에서 어떻게 동일하게 유지되는가**

본 문서는 그 runtime contract 를 닫는다.

### 1.2 제품 비전과의 연결 [P2B]

`docs/00-overview.md §1`, `docs/design/phase2/13-penny-shared-surface-loop.md`, `docs/design/web/expert-knowledge-workbench.md` 가 합쳐서 정의한 Gadgetron 의 P2B 원칙은 다음과 같다.

> **Penny 는 중심 surface 이지만, truth 는 shared activity / evidence / candidate / approval projection 이 소유한다.**

이 원칙은 prompt engineering 문제가 아니라 ingress/runtime 설계 문제다. Bootstrap 이 실제 subprocess 입력에 안정적으로 도달하지 못하면, Penny 는 다음 turn awareness 를 "운이 좋으면 기억한다" 수준으로만 제공하게 되고, direct action parity 는 문서 문구에만 남는다.

따라서 이 문서의 목표는 새 기능을 하나 더 만드는 것이 아니라, 이미 승인된 product contract 를 실제 HTTP ingress 와 native resume semantics 위에 정확히 앉히는 것이다.

### 1.3 고려한 대안과 채택하지 않은 이유 [P2B]

| 대안 | 설명 | 채택하지 않은 이유 |
|---|---|---|
| A. `System` message 를 하나 더 prepend 한다 | 구현이 직관적으로 보인다 | `NativeResumeTurn` 은 현재 마지막 user message 만 stdin 으로 보내므로, resume turn 에서 shared context 가 사라지는 silent regression 을 만들기 쉽다 |
| B. gateway 가 bootstrap 을 만들고 Penny private session store 에만 저장한다 | chat path 는 간단해 보인다 | shared surface truth 와 subprocess-local memory 가 갈라지고, `13` 문서의 "no hidden memory path" 원칙과 정면 충돌한다 |
| C. latest user message text 를 request-local clone 에서만 prefix rewrite 한다 | stateless / first-turn / resume-turn 모두 동일한 data path 를 탄다 | message rewrite 규칙을 정확히 문서화하지 않으면 audit/UX 회귀 위험이 있다 |
| D. `LlmProvider` trait 을 확장해 actor/bootstrap 을 직접 넘긴다 | provider 추상화는 깔끔해 보인다 | 모든 provider 와 router crate 를 건드리는 데 비해, 현재 문제는 Penny ingress 한정 seam 이다. 범위가 과하다 |

채택: **C. gateway ingress 에서 latest user message 를 request-local clone 으로 rewrite 하고, Penny provider / session 은 그 rewritten request 를 기존 경로로 소비한다**

### 1.4 핵심 설계 원칙과 trade-off [P2B]

1. **Bootstrap assembly happens in gateway, not in Penny subprocess**
   actor/auth/quota/request_id 를 가진 truth boundary 는 gateway ingress 다. subprocess 는 이미 준비된 request 만 소비한다.
2. **Injection targets the latest user turn, not a side-channel**
   `NativeResumeTurn` 이 마지막 user message 만 stdin 으로 보낸다는 현재 contract 를 깨지 않고도 shared context 를 전달해야 한다.
3. **Every Penny request gets a fresh bootstrap**
   `conversation_id`, session store hit, `--resume`, cached Claude state 어느 것도 bootstrap 생략 근거가 아니다.
4. **Request-local rewrite only**
   bootstrap text 는 client body, canonical transcript, wiki write payload, knowledge candidate raw source 로 저장되지 않는다. ingress clone 에만 존재한다.
5. **No silent degradation**
   shared surface 일부가 실패하면 request 는 계속 갈 수 있지만, injected block 에 `health: degraded` 와 plain-language reason 이 반드시 나타나야 한다.
6. **Non-Penny models stay bit-for-bit unchanged**
   `model != "penny"` 경로는 bootstrap assemble, message rewrite, extra tracing, config validation 변화가 없어야 한다.
7. **Resume continuity and operator truth are separate concerns**
   session resume 는 Claude 대화 continuity, bootstrap 은 operator truth digest 다. 둘은 결합되지만 서로 대체하지 않는다.

Trade-off:

- request body 를 handler 에서 rewrite 하는 것은 순수 pass-through보다 복잡하다.
- 대신 이 복잡도를 ingress 한 지점에 모아야 `session.rs`, `provider.rs`, `router.rs`, client payload shape 전체를 동시에 흔들지 않고도 resume-safe shared context 를 보장할 수 있다.

### 1.5 Operator Touchpoint Walkthrough [P2B]

1. 사용자가 `/web` 또는 API client 로 `model = "penny"` 요청을 보낸다.
2. gateway 는 기존 auth/quota/scope 체인을 통과해 `TenantContext` 와 `request_id` 를 만든다.
3. 같은 ingress 지점에서 `DefaultPennyTurnContextAssembler::build()` 가 최근 activity / pending candidate / pending approval digest 를 조립한다.
4. gateway 는 digest 를 `render_penny_shared_context()` 로 deterministic block 으로 렌더링한 뒤, request-local clone 의 **마지막 user message text** 앞에 붙인다.
5. Penny provider 는 기존과 같은 `ChatRequest` shape 를 받는다. stateless/first-turn/resume-turn 모두 마지막 user message 경로로 bootstrap 이 전달된다.
6. shared surface 일부가 실패해도 요청은 계속 갈 수 있지만, injected block 의 `health: degraded` 와 `degraded_reasons` 때문에 Penny 는 제한된 awareness 상태를 숨기지 못한다.
7. 같은 `conversation_id` 로 resume turn 이 와도 gateway 는 다시 조립하고 다시 붙인다. Claude session resume 는 continuity 를 제공하지만 사실 source 는 아니다.

#### 1.5.1 5-Minute Operator Smoke Path [P2B]

```bash
curl -sS -N -D - \
  -H "Authorization: Bearer gad_live_localtest" \
  -H "Content-Type: application/json" \
  -H "X-Gadgetron-Conversation-Id: smoke-psl-1b" \
  http://127.0.0.1:8080/v1/chat/completions \
  -d '{
    "model": "penny",
    "stream": true,
    "messages": [
      {"role": "user", "content": "방금 직접 실행된 작업이 있었는지 알려줘"}
    ]
  }'
```

기대 성공 신호:

- HTTP 200
- `content-type: text/event-stream`
- response body 에 `data:` SSE frame 이 1개 이상 존재
- Penny 응답이 최근 shared activity 를 설명하거나, 없으면 없다고 명시
- server trace 에 `penny_turn_prepare_started`, `penny_shared_context_injected` 가 1회씩 기록

기대 실패 신호:

- shared context wiring 누락 시 HTTP 503 with `error.code = "penny_shared_context_unconfigured"`
- malformed Penny payload 시 HTTP 400 with `error.code = "penny_tool_invalid_args"`
- `stream = false` 요청 시 Penny dispatch 전에 HTTP 400 with `error.code = "penny_tool_invalid_args"`

---

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API [P2B]

본 문서가 새로 고정하는 공개 surface 는 세 층이다.

1. gateway-local Penny turn preparation helper
2. latest-user-message rewrite contract
3. ingress tracing / error contract

#### 2.1.1 Gateway-local turn preparation API (`gadgetron-gateway`) [P2B]

새 helper module:

```rust
// crates/gadgetron-gateway/src/penny/turn_prep.rs

use gadgetron_core::{
    agent::shared_context::PennyTurnBootstrap,
    error::GadgetronError,
    provider::ChatRequest,
};
use crate::{
    middleware::tenant_context::TenantContext,
    server::AppState,
};

pub struct PreparedPennyTurn {
    pub bootstrap: PennyTurnBootstrap,
    pub rendered_context: String,
    pub bootstrap_digest_sha256_prefix: String,
    pub request: ChatRequest,
}

pub async fn prepare_penny_turn(
    state: &AppState,
    ctx: &TenantContext,
    req: ChatRequest,
) -> Result<PreparedPennyTurn, GadgetronError>;

pub fn inject_shared_context_into_latest_user_turn(
    req: ChatRequest,
    rendered_context: &str,
) -> Result<ChatRequest, GadgetronError>;
```

계약:

- `prepare_penny_turn()` 는 `req.model == "penny"` 인 경우에만 호출한다.
- `state.penny_shared_surface = None` 이면 fail-closed 한다. Penny P2B contract 에서 shared context 는 optional feature 가 아니다.
- `AppState` 는 `penny_shared_context_cfg: SharedContextConfig` 를 함께 보유한다. helper 는 이 config 와 `state.penny_shared_surface` 로 `DefaultPennyTurnContextAssembler` 를 구성한다.
- `ctx.request_id`, `ctx.tenant_id`, `ctx.api_key_id`, `req.conversation_id` 를 Penny prepare path 전 구간에서 함께 유지한다.
- `inject_shared_context_into_latest_user_turn()` 는 handler 가 소유한 `ChatRequest` 를 consume 하며, deep clone 을 만들지 않는다.
- Penny prepare path 는 `/v1/chat/completions` 기존 auth chain 뒤에서만 동작한다. 즉, Bearer auth + `Scope::OpenAiCompat` 를 통과한 actor 만 `model = "penny"` branch 에 도달할 수 있고, 실패 시 기존 default-deny 401/403 응답을 그대로 사용한다.

신뢰 경계 입력표:

| 입력 | 경계 | 검증 규칙 |
|---|---|---|
| `HeaderMap` | HTTP client -> gateway | `X-Gadgetron-Conversation-Id` 는 기존 256-byte / no-NUL / no-CRLF 규칙 재사용 |
| `ChatRequest.messages` | HTTP client -> gateway | Penny path 는 `messages.last().role == User` 를 요구하고, 위반 시 즉시 400 `penny_tool_invalid_args` |
| `req.model` | HTTP client -> gateway | exact `"penny"` match 일 때만 prepare path 진입 |
| `TenantContext` | auth middleware -> handler | existing validated `tenant_id`, `api_key_id`, `scopes` 를 신뢰하며 재파싱하지 않음 |
| shared-surface records | projection service -> renderer | actor-filtered summary-only payload 이어야 하며 raw secret/path/stack trace 금지 |

#### 2.1.2 Latest user turn rewrite contract [P2B]

핵심 규칙은 "synthetic system message 추가"가 아니라 "마지막 user message prefix rewrite" 다.

```rust
pub fn inject_shared_context_into_latest_user_turn(
    req: ChatRequest,
    rendered_context: &str,
) -> Result<ChatRequest, GadgetronError> {
    let mut req = req;
    let last = req
        .messages
        .last_mut()
        .ok_or_else(|| GadgetronError::Penny {
            kind: PennyErrorKind::ToolInvalidArgs {
                reason: "penny request must contain at least one message and the final message must be a user turn".into(),
            },
            message: "cannot inject shared context without a user turn".into(),
        })?;
    if !matches!(last.role, Role::User) {
        return Err(GadgetronError::Penny {
            kind: PennyErrorKind::ToolInvalidArgs {
                reason: "penny request requires messages.last().role == User".into(),
            },
            message: "invalid Penny request shape for shared-context injection".into(),
        });
    }

    let original = last.content.text().unwrap_or("");
    let merged = format!(
        "{rendered_context}\n\n<gadgetron_user_request>\n{original}\n</gadgetron_user_request>"
    );
    last.content = Content::Text(merged);
    Ok(req)
}
```

Load-bearing 이유:

- `Stateless` path 는 전체 history 를 flatten 하지만 마지막 user message 도 같이 들어간다.
- `NativeFirstTurn` path 는 system framing + 마지막 user message 를 보낸다. user message rewrite 는 이 규칙과 충돌하지 않는다.
- `NativeResumeTurn` path 는 마지막 user message 만 보낸다. 별도 system message prepend 전략은 이 경로에서 사라질 수 있지만, user message rewrite 는 사라지지 않는다.
- gateway 는 `messages.last().role == User` invariant 를 session driver 와 동일하게 선검증해, malformed request 가 handler 와 `session.rs` 에서 서로 다른 400 contract 를 만들지 않게 한다.
- rendered block 내부의 untrusted summary text 는 `sanitize_digest_line()` 규칙을 거친다.
  - `\r`, `\n`, `\0` -> space 치환
  - repeated whitespace collapse
  - literal `<`, `>` -> `‹`, `›` 치환
  - raw markdown/code fence/opening tag 삽입 금지

#### 2.1.3 Handler branching contract [P2B]

`chat_completions_handler()` 의 routing contract:

```rust
pub async fn chat_completions_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    headers: HeaderMap,
    Json(mut req): Json<ChatRequest>,
) -> Response {
    hydrate_conversation_id(&headers, &mut req)?;

    // Existing auth + tenant context already established `ctx`.
    // Quota for Penny is computed on the rewritten request shape.
    if req.model == "penny" {
        let prepared = prepare_penny_turn(&state, &ctx, req).await?;
        req = prepared.request;
    }

    // quota pre-check uses the final request shape actually sent to the model
    // existing router + audit path continues
}
```

규칙:

- `conversation_id` hydration/validation 이 bootstrap assemble 보다 먼저다.
- Penny path 에서는 bootstrap assemble/rewrite 뒤의 최종 request shape 를 기준으로 quota 와 usage accounting 을 계산한다. injected bootstrap tokens 도 실제 모델 context 를 소비하므로 tenant quota/billing 대상이다.
- bootstrap assemble/rewrite 는 **quota 계산 직전, router dispatch 직전** request-local branch 에서 수행한다.
- non-Penny model 은 기존 bit-identical request 를 그대로 router 로 보낸다.
- tracing/audit 은 `tenant_id`, `api_key_id`, `request_id`, `conversation_id`, `bootstrap_digest_sha256_prefix` 를 함께 남기되 rendered block 전문은 남기지 않는다.

#### 2.1.4 Error surface [P2B]

새 operator-facing error code 및 message contract:

| 상황 | HTTP | OpenAI error.type | code | message contract |
|---|---|---|---|---|
| Penny request final message 가 user turn 이 아님 / message 가 비어 있음 | 400 | `invalid_request_error` | `penny_tool_invalid_args` | `Penny requests require the final messages[] entry to be a user turn. Append a user message and retry.` |
| Penny shared surface service 미구성 | 503 | `server_error` | `penny_shared_context_unconfigured` | `Penny shared context is not configured on this gateway. Verify Penny startup wiring and retry after the service is enabled.` |
| bootstrap assemble identity failure | 401/403 | 기존 auth mapping 재사용 | 기존 code 유지 | 기존 auth/permission remediation 을 그대로 유지 |
| partial shared-surface failure | 200 | N/A | N/A | chat 계속 진행, injected block 에 `health: degraded` 반영 |

원칙:

- partial failure 는 HTTP 에러가 아니다.
- "왜/어떻게 고칠지" 없는 generic 500 문자열은 금지한다.
- `degraded_reasons` 는 prompt block 과 tracing 에는 보이지만, raw DB DSN / file path / stack trace 는 금지한다.
- `penny_tool_invalid_args` 는 current trunk 의 `GadgetronError::Penny { kind: PennyErrorKind::ToolInvalidArgs { .. } }` mapping 을 재사용한다. authoritative wire mapping 은 `gadgetron-core::error` 와 `docs/design/phase2/04-mcp-tool-registry.md` §10.1 이며, 본 문서는 새 Penny error code 를 발명하지 않는다.

### 2.2 내부 구조 [P2B]

#### 2.2.1 Data flow [P2B]

```text
HTTP POST /v1/chat/completions
        |
        v
request_id_middleware
        |
        v
auth_middleware + tenant_context_middleware
        |
        v
chat_completions_handler
        |
        +--> if model != "penny" ------------------------------+
        |                                                     |
        +--> if model == "penny"                              |
                |                                             |
                v                                             |
        DefaultPennyTurnContextAssembler::build               |
                |                                             |
                v                                             |
        render_penny_shared_context                           |
                |                                             |
                v                                             |
        inject_shared_context_into_latest_user_turn           |
                |                                             |
                +------------------------> router.chat/chat_stream
```

중요한 점:

- assembler 와 renderer 는 gateway process 안에서 끝난다.
- Penny provider / session / spawn path 는 이미 rewrite 된 request 만 본다.
- resume-turn 여부는 session.rs 가 판단하지만, shared context 주입 여부는 그 전에 이미 결정된다.

#### 2.2.2 Why not synthetic `System` message [P2B]

현재 `gadgetron-penny::session::build_stdin_payload()` 의 세 mode 는 다음 contract 를 가진다.

| Mode | 현재 입력 규칙 | synthetic system message 전략 위험 |
|---|---|---|
| `FlattenHistory` | 모든 message flatten | 전달은 되지만 mode 간 일관성이 깨진다 |
| `NativeFirstTurn` | 첫 system + 마지막 user | 전달되더라도 "어느 system 이 authoritative 인가" ambiguity 발생 |
| `NativeResumeTurn` | 마지막 user only | prepend 한 system message 가 완전히 버려질 수 있다 |

따라서 PSL-1b 는 `session.rs` 의 mode semantics 를 바꾸지 않는다. 대신 mode-independent 하게 **마지막 user message 자체** 를 rewrite 한다.

#### 2.2.3 Idempotence and storage boundary [P2B]

rewrite 규칙은 handler 가 소유한 request-local value 에만 적용된다.

- client 가 보낸 raw JSON body 를 디스크에 rewrite 해서 저장하지 않는다.
- session store 는 Claude native session continuity 만 관리한다. bootstrap block 자체를 사실 저장소로 취급하지 않는다.
- audit/event/wire log 는 request_id, health, counts, degraded 여부만 남긴다. rendered block 전문을 남기지 않는다.

이 규칙이 필요한 이유:

1. bootstrap text 를 canonical transcript 로 저장하면 next-turn truth source 가 subprocess-local jsonl 로 이동한다.
2. same request retry 시 shared context block 이 누적 저장되면 prompt 폭주가 생긴다.
3. operator 가 user-authored content 와 system-injected context 를 분리해 이해할 수 있어야 한다.

#### 2.2.4 Resume boundary contract [P2B]

`conversation_id` 와 `SessionStore` 는 Claude 의 대화 continuity 를 위해 유지한다. 하지만 PSL-1b 에서 새로 고정하는 규칙은 다음이다.

1. `conversation_id = Some(_)` 이고 session store entry 가 존재해도 assembler 는 매 요청 실행한다.
2. `NativeResumeTurn` 의 stdin payload 는 "새 마지막 user message" 여야 한다. 이 마지막 user message 는 이미 rewrite 된 form 이어야 한다.
3. `SessionStore::touch()` / mutex / `--resume` branch 는 shared context freshness 판단에 사용하지 않는다.
4. out-of-band direct action 이 resume 사이에 발생하면, 다음 resume turn 의 rewritten user message 앞에는 새 digest 가 붙어야 한다.

즉:

> **resume 는 Claude continuity 를 제공하지만, shared context freshness 는 gateway ingress 가 책임진다.**

#### 2.2.5 Partial failure behavior [P2B]

세 failure class:

1. **Unconfigured**
   `state.penny_shared_surface.is_none()`.
   결과: 503 fail-closed.
2. **Identity failure**
   actor/context resolution 실패.
   결과: 기존 auth failure mapping 사용.
3. **Partial shared-surface failure**
   activity/candidate/approval 일부 read 실패.
   결과: assembler 는 `health = Degraded`, `degraded_reasons += ...`, request 는 계속 진행.

Load-bearing 규칙:

- degraded 는 "없는 척 계속 진행"이 아니다.
- `require_explicit_degraded_notice = true` 는 startup validation 과 runtime rendering 양쪽에서 지켜야 한다.
- non-Penny model 은 shared surface 장애의 영향을 받지 않는다.
- `degraded_reasons` 는 fixed vocabulary + redaction allowlist 로 normalize 한다. downstream error `Display` 문자열을 그대로 넣지 않는다.
  - `recent activity may be incomplete`
  - `pending candidate summary may be incomplete`
  - `pending approval summary may be incomplete`

### 2.3 설정 스키마 [P2B]

기존 `[agent.shared_context]` 를 그대로 사용한다.

```toml
[agent.shared_context]
bootstrap_activity_limit = 6
bootstrap_candidate_limit = 4
bootstrap_approval_limit = 3
digest_summary_chars = 240
require_explicit_degraded_notice = true
```

Env override convention:

- `GADGETRON_AGENT_SHARED_CONTEXT_BOOTSTRAP_ACTIVITY_LIMIT`
- `GADGETRON_AGENT_SHARED_CONTEXT_BOOTSTRAP_CANDIDATE_LIMIT`
- `GADGETRON_AGENT_SHARED_CONTEXT_BOOTSTRAP_APPROVAL_LIMIT`
- `GADGETRON_AGENT_SHARED_CONTEXT_DIGEST_SUMMARY_CHARS`
- `GADGETRON_AGENT_SHARED_CONTEXT_REQUIRE_EXPLICIT_DEGRADED_NOTICE`

적용 순서:

1. `gadgetron.toml` deserialize
2. `GADGETRON_AGENT_*` env override 적용
3. `AgentConfig::validate()` 실행

authority: `docs/design/phase2/04-mcp-tool-registry.md` §4 env override convention

PSL-1b 에서 새로 고정하는 규칙:

- ingress rewrite 는 이 subsection 이 존재하지 않아도 `Default::default()` 로 활성이다.
- `enabled = false` 같은 toggle 은 추가하지 않는다.
- `digest_summary_chars` cap 은 render 단계에서만 적용한다. assembler 가 source digest 자체를 mutate 하지 않는다.
- `require_explicit_degraded_notice = false` 는 문서상 금지일 뿐 아니라 startup validation failure 여야 한다.

필드별 operator semantics:

| 필드 | default | env override | operator 의미 |
|---|---|---|---|
| `bootstrap_activity_limit` | `6` | `GADGETRON_AGENT_SHARED_CONTEXT_BOOTSTRAP_ACTIVITY_LIMIT` | recent activity digest 최대 개수. 높을수록 awareness 는 늘고 prompt 비용도 증가 |
| `bootstrap_candidate_limit` | `4` | `GADGETRON_AGENT_SHARED_CONTEXT_BOOTSTRAP_CANDIDATE_LIMIT` | pending candidate 최대 개수. curation-heavy 환경에서만 상향 권장 |
| `bootstrap_approval_limit` | `3` | `GADGETRON_AGENT_SHARED_CONTEXT_BOOTSTRAP_APPROVAL_LIMIT` | pending approval 최대 개수. `0` 은 approval summary 비표시지만 degraded 규칙은 유지 |
| `digest_summary_chars` | `240` | `GADGETRON_AGENT_SHARED_CONTEXT_DIGEST_SUMMARY_CHARS` | 각 summary/title clipping 상한. 다국어 환경은 320 전후까지 상향 가능 |
| `require_explicit_degraded_notice` | `true` | `GADGETRON_AGENT_SHARED_CONTEXT_REQUIRE_EXPLICIT_DEGRADED_NOTICE` | degraded 숨김 금지. `false` 는 startup validation fail-closed |

### 2.4 에러 & 로깅 [P2B]

#### 2.4.1 tracing span/event [P2B]

필수 tracing:

```rust
tracing::info!(
    request_id = %ctx.request_id,
    tenant_id = %ctx.tenant_id,
    api_key_id = %ctx.api_key_id,
    conversation_id = req.conversation_id.as_deref().unwrap_or("<none>"),
    model = %req.model,
    "penny_turn_prepare_started"
);

tracing::info!(
    request_id = %ctx.request_id,
    tenant_id = %ctx.tenant_id,
    api_key_id = %ctx.api_key_id,
    rendered_bytes = prepared.rendered_context.len(),
    bootstrap_digest_sha256_prefix = %prepared.bootstrap_digest_sha256_prefix,
    degraded = matches!(prepared.bootstrap.health, PennySharedContextHealth::Degraded),
    "penny_shared_context_injected"
);
```

추가 규칙:

- rendered block 전문은 trace field 로 남기지 않는다.
- `degraded_reasons` 는 warn event 로 남길 수 있지만 secret-bearing string 은 금지다.
- non-Penny path 에서는 이 span/event 가 없어야 한다.
- `bootstrap_digest_sha256_prefix` 는 rendered block 전문 대신 provenance correlation 용으로 남기는 12-hex prefix fingerprint 다.

#### 2.4.2 Audit / quota contract [P2B]

- quota pre-check 는 Penny rewrite 이후의 최종 request shape 로 수행한다
- injected bootstrap tokens 는 tenant quota / usage accounting 대상이다. 별도 공짜 gateway overhead 로 취급하지 않는다
- bootstrap 조립 실패가 fail-closed 503 인 경우에도 audit status 는 `Error` 로 남긴다
- partial degraded continuation 은 정상 request 로 분류하되, structured tracing 으로 degraded 여부를 남긴다
- audit path 는 `tenant_id`, `api_key_id`, `request_id`, `conversation_id`, `bootstrap_digest_sha256_prefix`, `degraded` 를 남긴다
- shared context block 은 audit payload 의 user prompt 전문에 포함하지 않는다
- raw client prompt 와 rewritten Penny prompt 가 모두 full-text 로 audit 저장되는 것은 금지한다. 필요 시 prompt metrics 는 token count 와 digest fingerprint 로만 남긴다

#### 2.4.3 STRIDE 요약 [P2B]

| 자산 / 경계 | 신뢰 경계 | Spoofing | Tampering | Repudiation | Info Disclosure | DoS | EoP | 완화 |
|---|---|---|---|---|---|---|---|---|
| `HeaderMap` / `conversation_id` | HTTP client -> gateway | 다른 대화 id 위조 | malformed header 삽입 | 누가 어떤 resume id 를 썼는지 부인 | trace/log 로 raw id 누출 | giant header flood | 다른 actor session 오용 | 256-byte/no-NUL/no-CRLF validation, header winner rule, request_id correlation |
| `ChatRequest.messages` | HTTP client -> gateway | assistant/tool tail 을 user turn 인 척 위조 | wrapper/tag spoofing | malformed request 부인 | raw prompt/log 노출 | giant prompt | non-Penny path 오염 | `messages.last().role == User` preflight, body-size cap, no full-text audit, wrapper boundary |
| `TenantContext` | auth middleware -> handler | 다른 tenant identity 주입 | middleware extension 변조 | tenant provenance 부인 | tenant/api key leakage | auth fanout overhead | unauthorized Penny access | existing Bearer auth + `Scope::OpenAiCompat`, handler never reparses auth, default-deny 401/403 |
| shared-surface digest | projection service -> renderer | 다른 actor digest 주입 | digest text 임의 조작 | "그런 context 못 받았다" 부인 | 타 사용자 activity/candidate 노출 | giant digest 로 prompt 폭주 | admin-only 정보 노출 | actor-filtered projection, char cap, `sanitize_digest_line`, fixed degraded vocabulary |
| latest user message rewrite | gateway prepare -> provider | wrong request 에 주입 | user text 덮어쓰기 | injected/system text 경계 불명확 | raw internal reason 노출 | duplicate rewrite 누적 | non-Penny path 오염 | owned-request rewrite only, Penny-only branch, fingerprinted tracing, no persistent storage |
| resume boundary | session store -> provider spawn | stale session 을 fresh truth 로 오인 | old digest 재사용 | direct action 이후 awareness 누락 | resume jsonl 를 truth 로 간주 | repeated stale retries | hidden private memory 승격 | always rebuild on every request, session store is continuity only |

컴플라이언스 매핑:

- SOC2 CC6.1: actor-filtered ingress + least privilege prompt context
- SOC2 CC6.7: degraded / injection / failure tracing correlation
- GDPR Art 32: raw candidate payload, secrets, internal paths 를 prompt block 에 넣지 않음
- HIPAA §164.312: N/A in current scope. PHI-specific storage/transport contract 는 본 PSL-1b slice 범위 밖이다

#### 2.4.4 Runbook / Troubleshooting [P2B]

| 증상 | 확인 포인트 | 원인 | 운영자 조치 |
|---|---|---|---|
| `penny_shared_context_unconfigured` 503 | startup logs, Penny registration path, `AppState` wiring | `penny_shared_surface` 또는 `penny_shared_context_cfg` 미주입 | Penny router registration/wiring 확인 후 gateway 재시작. 5-minute smoke path 재실행 |
| 응답은 오지만 awareness 가 비어 있음 | `penny_shared_context_injected` trace, degraded flag, activity source health | projection source empty 또는 degraded normalization | `/api/v1/web/workbench/activity` source 및 projection health 확인. degraded reason 과 request_id 상관관계 점검 |
| resume turn 에 예전 상태를 말함 | same `conversation_id` 두 turn 의 fingerprint diff | bootstrap 재조립 생략 또는 stale digest reuse 회귀 | stale negative control integration test 실행, `bootstrap_digest_sha256_prefix` 비교 |
| `penny_tool_invalid_args` 400 | `messages[]` 마지막 role, conversation id validation | malformed client payload | final user turn appended 되도록 client 수정. same error code 유지 여부 확인 |

#### 2.4.5 i18n / Copy Boundary [P2B]

- `error.code`, wrapper tag (`<gadgetron_shared_context>`, `<gadgetron_user_request>`), fingerprint field name 은 stable ASCII contract 다.
- human-readable `error.message` 와 degraded reason 문구는 future localization 가능한 catalog entry 로 취급한다.
- 구현 초기에는 English operator copy 를 기본으로 두되, localization layer 는 `error.code` 를 key 로 bind 한다.
- 이 문서의 한국어 문장은 설명용이며 wire contract 의 권위는 structural field 와 `error.code` 다.

### 2.5 의존성 [P2B]

- 신규 외부 crate 추가 없음
- 사용 기존 경계:
  - `gadgetron-gateway::penny::shared_context`
  - `gadgetron-core::agent::shared_context`
  - `gadgetron-penny::session`
  - `gadgetron-xaas` audit/quota existing hooks

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 크레이트 경계 [P2B]

| 위치 | 소유 책임 | 이유 |
|---|---|---|
| `gadgetron-core::agent::shared_context` | bootstrap DTO / trait | shared wire shape, D-12 준수 |
| `gadgetron-gateway::penny::shared_context` | assembler + renderer | auth/request_id/tenant truth boundary 가 gateway 이기 때문 |
| `gadgetron-gateway::penny::turn_prep` | ingress rewrite helper | HTTP handler-local orchestration 이므로 leaf crate 소유 |
| `gadgetron-penny::session` | spawn mode + stdin shaping | subprocess lifecycle 소유 |
| `gadgetron-router` | provider dispatch | Penny 여부와 무관한 generic routing 유지 |

본 문서는 **core DTO 를 gateway 로 끌어올리지 않고**, **provider trait 을 Penny-specific 으로 오염시키지 않으며**, **HTTP ingress rewrite 를 gateway leaf concern 으로 남기는 것** 을 D-12 compliant 기본선으로 둔다.

### 3.2 데이터 흐름 다이어그램 [P2B]

```text
Browser / SDK
   |
   v
POST /v1/chat/completions (model = penny)
   |
   v
gateway auth + tenant context
   |
   +--> DefaultPennyTurnContextAssembler
   |        |
   |        +--> recent activity
   |        +--> pending candidates
   |        +--> pending approvals
   |
   +--> render_penny_shared_context
   |
   +--> prepare_penny_turn + rewrite
   |
   +--> quota pre-check on rewritten request
   |
   v
router.chat_stream()
   |
   v
PennyProvider
   |
   v
ClaudeCodeSession
   |
   v
build_stdin_payload(Stateless | NativeFirstTurn | NativeResumeTurn)
```

### 3.3 타 도메인 인터페이스 계약 [P2B]

- `gateway-router-lead`
  - Penny branch 는 `/v1/chat/completions` namespace와 OpenAI error shape 를 유지해야 한다.
- `chief-architect`
  - `ChatRequest` public wire shape breaking change 없이 request-local rewrite helper 로 해결해야 한다.
- `qa-test-architect`
  - resume refresh regression, duplicate injection, degraded continuation, non-Penny no-op 를 test pyramid 에 고정해야 한다.
- `xaas-platform-lead`
  - audit/quota semantics 는 Penny branch 에서도 기존 contract 를 유지해야 한다.
- `security-compliance-lead`
  - injected block 에 secret/path/stack trace 가 들어가지 않도록 redaction rule 을 리뷰해야 한다.

### 3.4 D-12 크레이트 경계표 준수 [P2B]

준수 판단:

- shared DTO stays in `gadgetron-core`
- HTTP orchestration stays in `gadgetron-gateway`
- subprocess mode logic stays in `gadgetron-penny`
- no new reverse dependency from `gadgetron-penny` to `gadgetron-gateway`

따라서 본 설계는 D-12 를 준수한다.

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위 [P2B]

| 대상 | 검증할 invariant |
|---|---|
| `inject_shared_context_into_latest_user_turn()` | 마지막 user message 만 rewrite 하고 다른 message/order/role 은 보존 |
| `inject_shared_context_into_latest_user_turn()` error path | final message 가 user turn 이 아니면 기존 `penny_tool_invalid_args` 400 contract 로 fail-closed |
| `prepare_penny_turn()` happy path | assembler → render → rewrite 를 수행하고 rendered block 이 latest user turn prefix 에 존재 |
| `prepare_penny_turn()` degraded path | partial failure 시 request 는 성공하고 block 에 `health: degraded` + reason 존재 |
| handler model branch | `model != "penny"` 는 assemble/render/rewrite 를 전혀 호출하지 않음 |
| resume path compatibility | rewritten latest user message 가 `NativeResumeTurn` stdin payload 에 그대로 실림 |
| startup validation path | `require_explicit_degraded_notice = false` 는 config validation fail-closed |
| trace/audit redaction path | `tenant_id`/`api_key_id`/fingerprint 는 남고 rendered block 전문과 secret-bearing string 은 남지 않음 |

### 4.2 테스트 하네스 [P2B]

- gateway unit test:
  - fake `PennySharedSurfaceService`
  - fixed `TenantContext { request_id, tenant_id, api_key_id, ... }`
  - fixed `ChatRequest`
- penny session unit test:
  - existing `build_stdin_payload()` tests 확장
  - rewritten request 를 입력으로 넣어 native first/resume 결과 비교
- tracing assertion:
  - `tracing-test` 또는 existing subscriber harness 로 event name/field 존재 확인
- deterministic controls:
  - fixed `Uuid` fixture for `request_id`
  - fixed clock injection 또는 deterministic helper 로 rendered RFC3339 string 고정
  - activity/candidate/approval fixture 는 newest-first canonical ordering 으로 제공
  - golden snapshot 파일은 `crates/gadgetron-gateway/tests/snapshots/penny_turn_prep__*.snap`
  - snapshot regeneration 은 contract 변경 PR 에서만 `cargo insta review` 로 수행. unrelated PR 에서 auto-update 금지

property-based test 필요 여부:

- 필수는 아니지만 `digest_summary_chars` clipping 과 wrapper insertion 이 UTF-8 / multi-code-point text 에서 panic 없이 동작하는지 property-based test 도입 가능

### 4.3 커버리지 목표 [P2B]

- `turn_prep.rs`: line coverage 90%+, branch coverage 85%+
- `chat_completions_handler` Penny branch: branch coverage 90%+
- `session.rs` 관련 resume regression tests: affected branches 100%
- perf verification:
  - `crates/gadgetron-gateway/benches/middleware_chain.rs` 확장 또는 `benches/penny_turn_prep.rs` 신규
  - fake assembler/service 기준 bootstrap prepare + rewrite 추가 오버헤드의 P99 < 1 ms 측정
  - 초기에는 CI non-blocking report 로 시작하되, regression review gate 에 숫자를 첨부

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위 [P2B]

- 함께 테스트할 크레이트:
  - `gadgetron-gateway`
  - `gadgetron-penny`
  - `gadgetron-core`
  - `gadgetron-xaas` (audit/quota fake)

통합 시나리오:

1. **5-minute smoke**
   local gateway + fake Penny provider -> one `curl` request -> 200 success and trace pair `penny_turn_prepare_started` / `penny_shared_context_injected`.
2. **Penny first turn injects fresh shared context**
   `POST /v1/chat/completions` with `model = "penny"` -> fake assembler returns activity digest -> fake Penny provider receives rewritten latest user message with `<gadgetron_shared_context>` block.
3. **Resume turn rebuilds after out-of-band change**
   same `conversation_id`, first request digest says `[]`, second request fake assembler returns new direct-action digest -> second rewritten latest user message reflects new digest even though resume path is used.
4. **Resume stale negative control**
   first turn digest A -> out-of-band direct action -> second turn digest B -> assert assembler invocation count == 2 and `bootstrap_digest_sha256_prefix` changed. cached digest reuse 시 test fail.
5. **Non-Penny model untouched**
   `model = "gpt-4o"` (or fixture provider) -> request body forwarded unchanged.
6. **Partial failure degraded continuation**
   assembler candidate/approval read partial fail -> response still streams, rewritten prompt contains degraded notice.
7. **Unconfigured fail-closed**
   `state.penny_shared_surface = None` + `model = "penny"` -> 503 `penny_shared_context_unconfigured`.

### 5.2 테스트 환경 [P2B]

- external dependency 없음
- in-memory fake router/provider sufficient
- existing gateway handler integration harness 재사용
- session store 실제 `SessionStore` 인메모리 구현 사용 가능
- fixture/snapshot 위치:
  - `crates/gadgetron-gateway/tests/fixtures/penny_turn/`
  - `crates/gadgetron-gateway/tests/snapshots/penny_turn_prep__*.snap`
  - `crates/gadgetron-penny/tests/fixtures/session_resume/`
- golden strategy:
  - structural assertions 우선
  - rendered prompt block shape 는 golden snapshot 으로 추가 고정
  - snapshot 변경은 doc/prompt-shape contract 변경 PR 에서만 허용

### 5.3 회귀 방지 [P2B]

다음 변경은 반드시 테스트를 깨야 한다.

- synthetic system message 로 전략을 바꿔 resume turn 에서 context 가 사라지는 변경
- resume request 에서 bootstrap 재조립을 생략하는 최적화
- stale digest A 를 second turn 에 재사용해 digest B 로 갱신되지 않는 변경
- non-Penny model 까지 rewritten message 를 받는 branch leakage
- degraded 이유를 숨기거나 `require_explicit_degraded_notice = false` 를 허용하는 변경
- injected block 전문이 trace/audit payload 로 남는 변경

---

## 6. Phase 구분

- [P2B]
  - gateway ingress bootstrap assembly
  - latest user turn rewrite
  - resume-safe prompt injection
  - degraded continuation / tracing / tests
- [P2C]
  - request-scoped `workbench.*` gadget runtime bridge hardening
  - richer audit correlation between injected digest and downstream tool calls
  - stronger prompt/provenance inspection tooling

---

## 7. 오픈 이슈 / 의사결정 필요

| ID | 내용 | 옵션 | 추천 | 상태 |
|---|---|---|---|---|
| Q-1 | injected shared-context block 을 future request-scoped `workbench.*` gadget bridge 와 어떻게 상관시킬 것인가 | A. 현행 prefix rewrite 유지 / B. 별도 internal envelope 타입 도입 / C. provider trait 확장 | A — PSL-1b 는 injection seam 만 닫고, gadget bridge 는 P2C 로 분리 | ⚪ follow-up, 비차단 |

---

## 리뷰 로그 (append-only)

### Round 1 — 2026-04-18 — @inference-engine-lead @xaas-platform-lead
**결론**: Pass

**체크리스트**: (`03-review-rubric.md §1` 기준)
- [x] 인터페이스 계약
- [x] 크레이트 경계
- [x] 타입 중복
- [x] 에러 반환
- [x] 동시성
- [x] 의존성 방향
- [x] Phase 태그
- [x] 레거시 결정 준수

**Action Items**:
- A1: 5-minute smoke path 를 Penny streaming-only contract 에 맞춰 `stream = true` SSE 검증으로 수정했다.
- A2: malformed request surface 가 current trunk `PennyErrorKind::ToolInvalidArgs` / `penny_tool_invalid_args` mapping 을 재사용함을 명시했다.
- A3: quota/accounting 이 rewritten request shape 기준임을 유지해 xaas 과금 경계를 고정했다.

**Open Questions**:
- 없음

**다음 라운드 조건**: 없음. Round 1 이슈 반영 후 Round 1.5 진행.

### Round 1.5 — 2026-04-18 — @security-compliance-lead @dx-product-lead
**결론**: Pass

**체크리스트**: (`03-review-rubric.md §1.5` 기준)
- [x] 위협 모델 (필수)
- [x] 신뢰 경계 입력 검증
- [x] 인증·인가
- [x] 시크릿 관리
- [x] 공급망
- [x] 암호화
- [x] 감사 로그
- [x] 에러 정보 누출
- [x] LLM 특이 위협
- [x] 컴플라이언스 매핑
- [x] 사용자 touchpoint 워크스루
- [x] 에러 메시지 3요소
- [x] CLI flag
- [x] API 응답 shape
- [x] config 필드
- [x] defaults 안전성
- [x] 문서 5분 경로
- [x] runbook playbook
- [x] 하위 호환
- [x] i18n 준비

**Action Items**:
- A1: trust-boundary 입력표, default-deny auth/scope gate, redaction rules, fixed degraded vocabulary 를 추가했다.
- A2: operator smoke path, troubleshooting table, i18n/copy boundary 를 추가했다.
- A3: env override naming 과 startup validation fail-closed 규칙을 문서화했다.

**Open Questions**:
- 없음

**다음 라운드 조건**: 없음. Round 2 진행 가능.

### Round 2 — 2026-04-18 — @qa-test-architect
**결론**: Pass

**체크리스트**: (`03-review-rubric.md §2` 기준)
- [x] 단위 테스트 범위
- [x] mock 가능성
- [x] 결정론
- [x] 통합 시나리오
- [x] CI 재현성
- [x] 성능 검증
- [x] 회귀 테스트
- [x] 테스트 데이터

**Action Items**:
- A1: deterministic fixture seed, snapshot 경로, stale negative control 을 문서에 고정했다.
- A2: P99 ingress overhead 검증 경로와 2-turn resume regression 시나리오를 명시했다.
- A3: in-memory gateway/provider/session harness 재사용 경계를 분리해 mock 전략을 명확히 했다.

**Open Questions**:
- 없음

**다음 라운드 조건**: 없음. Round 3 진행 가능.

### Round 3 — 2026-04-18 — @chief-architect
**결론**: Pass

**체크리스트**: (`03-review-rubric.md §3` 기준)
- [x] Rust 관용구
- [x] 제로 비용 추상화
- [x] 제네릭 vs 트레이트 객체
- [x] 에러 전파
- [x] 수명주기
- [x] 의존성 추가
- [x] 트레이트 설계
- [x] 관측성
- [x] hot path
- [x] 문서화

**Action Items**:
- A1: prepare helper 가 owned-request rewrite 만 수행하고 non-Penny path 는 bit-identical pass-through 임을 명시했다.
- A2: `penny_tool_invalid_args` 가 existing trunk taxonomy 재사용임을 `gadgetron-core::error` / `04-mcp-tool-registry.md` 기준으로 고정했다.
- A3: tracing fingerprint 와 quota boundary 를 분리해 hot-path observability contract 를 명확히 했다.

**Open Questions**:
- 없음

**다음 라운드 조건**: 없음. 최종 승인 가능.

### 최종 승인 — 2026-04-18 — PM
**결론**: Approved

Round 1 / 1.5 / 2 / 3 지적 사항이 동일 문서 revision 에서 모두 반영되었음을 확인했다. 본 문서는 recent `origin/main` PSL-1 contract landed state 와 충돌하지 않으며, P2B runtime seam 을 중복 없이 닫는 authoritative design doc 로 승인한다.
