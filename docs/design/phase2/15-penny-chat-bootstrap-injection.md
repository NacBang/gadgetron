# 15 — Penny Chat Bootstrap Injection & Resume Boundary

> **담당**: @gateway-router-lead
> **상태**: Approved
> **작성일**: 2026-04-18
> **최종 업데이트**: 2026-04-18
> **관련 크레이트**: `gadgetron-gateway`, `gadgetron-penny`, `gadgetron-core`, `gadgetron-router`, `gadgetron-xaas`
> **Phase**: [P2B] current landed slice / [P2C] request-scoped gadget bridge hardening / [P2C+] native resume delivery correction
> **관련 문서**: `docs/design/phase2/02-penny-agent.md`, `docs/design/phase2/10-penny-permission-inheritance.md`, `docs/design/phase2/13-penny-shared-surface-loop.md`, `docs/design/phase2/14-penny-retrieval-citation-contract.md`, `docs/design/gateway/route-groups-and-scope-gates.md`, `docs/reviews/pm-decisions.md` D-6/D-7/D-11/D-12/D-13, `docs/process/04-decision-log.md` D-20260418-15, D-20260418-16
> **보정 범위**: 2026-04-18 기준 `origin/main` 의 authoritative runtime contract 는 `crates/gadgetron-gateway/src/handlers.rs` 와 `crates/gadgetron-penny/src/session.rs` 가 합쳐서 정의한다. 현재 landed PSL-1b 구현은 shared-context block 을 **기존 text `System` message 앞에 prepend 하거나, 없으면 index 0 에 새 `System` message 로 insert** 한다. gateway 는 매 요청 bootstrap 을 다시 조립하지만, `NativeResumeTurn` 은 여전히 마지막 user message 만 stdin 으로 보내므로, native resume delivery 보장은 아직 닫히지 않았다. 이전 revision 의 "latest user turn rewrite" 서술은 이 문서 revision 에서 superseded 된다.

---

## 1. 철학 & 컨셉 (Why)

### 1.1 이 문서가 닫는 공백 [P2B]

`origin/main` 은 이미 다음을 landed 했다.

1. `gadgetron-core::agent::shared_context` 가 `PennyTurnBootstrap` 및 digest 타입을 제공한다.
2. `gadgetron-gateway::penny::shared_context` 가 `DefaultPennyTurnContextAssembler` 와 `render_penny_shared_context()` 를 제공한다.
3. `gadgetron-gateway::handlers::chat_completions_handler()` 가 bootstrap 을 조립해 request-local message list 에 주입한다.
4. `gadgetron-penny::session::build_stdin_payload()` 가 `FlattenHistory`, `NativeFirstTurn`, `NativeResumeTurn` 별 stdin 전달 규칙을 고정한다.

하지만 recent mainline 은 문서 SSOT 측면에서 두 개의 seam 을 남겼다.

- gateway 가 실제로는 어떤 위치에 bootstrap block 을 붙이는가
- 매 turn reassembly 와 native resume delivery 중 어디까지가 현재 trunk 의 보장 범위인가

이 seam 이 열려 있으면 세 가지 운영 회귀가 즉시 생긴다.

1. 설계 문서가 최신 landed code 와 다른 injection 방식을 설명해 reviewer 와 구현자가 서로 다른 contract 를 믿게 된다.
2. operator 가 `conversation_id` 가 있으면 fresh shared context 가 Claude subprocess 에도 항상 들어간다고 오해하게 된다.
3. hidden memory 를 금지한 `13-penny-shared-surface-loop.md` 와 달리, gateway reassembly 와 session continuity 의 책임 경계가 흐려진다.

즉, 이 문서가 닫아야 하는 가장 큰 공백은 다음 한 문장이다.

> **gateway 가 shared-context bootstrap 을 현재 trunk 에서 정확히 어디에 주입하고, 그 결과 stateless / native first turn / native resume turn 각각에서 무엇이 실제로 보장되는가**

### 1.2 제품 비전과의 연결 [P2B]

`docs/00-overview.md §1` 과 `docs/design/phase2/13-penny-shared-surface-loop.md` 가 고정한 원칙은 같다.

> **Penny 는 중심 surface 이지만, truth 는 shared activity / evidence / candidate / approval projection 이 소유한다.**

따라서 gateway 는 Claude session continuity 와 operator truth 를 같은 것으로 취급하면 안 된다.

- session resume 는 Claude 대화 연속성이다.
- bootstrap reassembly 는 operator truth freshness 다.
- 둘이 같은 요청 안에서 만날 수는 있지만, 서로를 대체해서는 안 된다.

이 문서의 목표는 "더 이상적인 주입 방식"을 선언하는 것이 아니라, **현재 landed PSL-1b slice 가 실제로 보장하는 범위**를 production-level 문서로 고정하는 데 있다.

### 1.3 고려한 대안과 채택하지 않은 이유 [P2B]

| 대안 | 설명 | 채택하지 않은 이유 |
|---|---|---|
| A. latest user turn rewrite | native resume 와 가장 잘 맞는다 | 현재 trunk 구현이 아니며, handler/helper/session 경계 전체를 다시 조정해야 한다 |
| B. synthetic system framing (`System` prepend/insert) | landed 구현과 일치하고 user-authored text 를 덮어쓰지 않는다 | `NativeResumeTurn` 에서는 이 framing 이 Claude stdin 으로 전달되지 않는다 |
| C. gateway 가 bootstrap 을 private session store 에 저장 | chat path 는 단순해 보인다 | shared surface truth 와 subprocess-local memory 가 갈라지고 hidden memory path 가 생긴다 |
| D. `LlmProvider` trait 확장 | provider 가 actor/bootstrap 을 직접 안다 | 현재 seam 은 gateway ingress 한정인데 범위를 여러 크레이트로 확장한다 |

채택: **B. 현재 trunk authority 는 synthetic system framing (`prepend_to_system` / `insert_new_system`) 이다.**

단, 이 채택은 "문제가 모두 닫혔다"는 뜻이 아니다. native resume delivery 와 Penny-only branch narrowing 은 follow-up 문서/PR 에서 다시 닫아야 한다.

### 1.4 핵심 설계 원칙과 trade-off [P2B]

1. **Bootstrap assembly happens in gateway, not in Penny subprocess**
   actor, auth, request_id, degraded policy 는 gateway ingress 가 truth boundary 다.
2. **Current injection shape is synthetic system framing**
   landed helper 는 user message 를 rewrite 하지 않고, text `System` message 를 prepend 하거나 새 `System` message 를 삽입한다.
3. **Every request gets a fresh bootstrap**
   `conversation_id`, session store hit, `--resume` 여부는 assembler 호출 생략 근거가 아니다.
4. **Request-local only**
   injected block 은 canonical transcript, wiki writeback payload, audit full text 저장소가 아니다.
5. **No silent degradation**
   shared surface 일부가 실패하면 handler 는 요청을 계속 보내더라도 warn trace 와 degraded block 규칙을 유지해야 한다.
6. **Current delivery guarantee is mode-dependent**
   `FlattenHistory` 와 `NativeFirstTurn` 에서는 system framing 이 Claude stdin 까지 간다. `NativeResumeTurn` 에서는 아니다.
7. **Session continuity and operator truth remain separate concerns**
   gateway 는 매 turn truth 를 다시 본다. Claude resume jsonl 은 그 truth 의 캐시 키가 아니다.

Trade-off:

- current landed helper 는 구현이 작고 user-authored content 를 직접 변형하지 않는다.
- 반면 native resume 에서는 injected system framing 이 subprocess 에 도달하지 않으므로, "재조립됨"과 "모델이 받음"을 같은 뜻으로 말할 수 없다.

### 1.5 Operator Touchpoint Walkthrough [P2B]

1. 사용자가 `/web` 또는 API client 로 `/v1/chat/completions` 요청을 보낸다.
2. gateway 는 auth/quota/scope 체인을 통과해 `TenantContext` 와 `request_id` 를 만든다.
3. `agent.shared_context.enabled = true` 이고 `state.penny_assembler.is_some()` 이면 gateway 는 `DefaultPennyTurnContextAssembler::build()` 를 실행한다.
4. assembler 가 성공하면 gateway 는 `render_penny_shared_context()` 를 호출하고, 그 결과를 `req.messages` 의 맨 앞 text `System` message 에 prepend 하거나, 적절한 system framing 이 없으면 index 0 에 새 `Message::system(block)` 을 insert 한다.
5. assembler 가 실패하면 handler 는 warn trace 만 남기고 원래 request 로 계속 진행한다. 현재 trunk 는 이 상황을 5xx 로 승격하지 않는다.
6. `FlattenHistory` 또는 `NativeFirstTurn` path 에서는 injected block 이 Penny subprocess stdin 으로 도달한다.
7. `NativeResumeTurn` path 에서는 gateway 가 block 을 다시 만들더라도 session driver 가 마지막 user message 만 보내므로, injected system block 은 Claude stdin 까지 가지 않는다.

즉:

> **현재 trunk 는 매 turn freshness reassembly 를 보장하지만, native resume turn 에서의 shared-context delivery 까지는 아직 보장하지 않는다.**

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
- server trace 에 `target = "penny_shared_context.inject"` info event 가 1회 기록되고 `injection_mode = "prepend_to_system"` 또는 `"insert_new_system"` 중 하나를 가진다

기대 한계 신호:

- `state.penny_assembler = None` 또는 `enabled = false` 인 build 에서는 request 는 그대로 200 으로 계속 진행될 수 있다. 이 경우 inject trace 가 없다.
- assembler build failure 시 `target = "penny_shared_context"` warn event 가 남고 request 는 원본 shape 로 계속 간다.
- native resume turn 에서는 gateway trace 상 inject 가 보여도 Claude stdin 이 마지막 user message only 규칙을 따르므로, same request 에서 fresh shared context 를 실제로 읽었다고 단정할 수 없다.

---

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API [P2B]

현재 load-bearing surface 는 네 층이다.

1. `SharedContextConfig` (`gadgetron-core`)
2. `PennyTurnContextAssembler` + `render_penny_shared_context()` (`gadgetron-gateway::penny::shared_context`)
3. `inject_shared_context_block()` (`gadgetron-gateway::handlers`)
4. `StdinMode` / `build_stdin_payload()` (`gadgetron-penny::session`)

#### 2.1.1 Handler-local injection helper (`gadgetron-gateway`) [P2B]

현재 trunk helper 는 다음 shape 다.

```rust
fn inject_shared_context_block(messages: &mut Vec<Message>, block: &str) -> &'static str {
    if let Some(first) = messages.first_mut() {
        if first.role == Role::System {
            if let Content::Text(text) = &mut first.content {
                let mut prefixed = String::with_capacity(block.len() + 2 + text.len());
                prefixed.push_str(block);
                prefixed.push_str("\n\n");
                prefixed.push_str(text);
                *text = prefixed;
                return "prepend_to_system";
            }
        }
    }
    messages.insert(0, Message::system(block));
    "insert_new_system"
}
```

계약:

- 첫 message 가 `Role::System` 이고 `Content::Text` 면 그 text 앞에 block 을 prepend 한다.
- 첫 message 가 없거나 system 이 아니거나 `Content::Parts` 인 경우에는 새 `Message::system(block)` 을 index 0 에 insert 한다.
- helper 는 user-authored message text 를 mutate 하지 않는다.
- helper 는 `messages.last().role == User` 를 검증하지 않는다. resume-time validation 은 `gadgetron-penny::session` 이 소유한다.
- 반환값은 tracing label 이다. 현재 유효한 값은 `"prepend_to_system"` 또는 `"insert_new_system"` 뿐이다.

#### 2.1.2 Handler orchestration contract (`chat_completions_handler`) [P2B]

현재 handler contract 는 다음 순서다.

```rust
pub async fn chat_completions_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<TenantContext>,
    headers: HeaderMap,
    Json(mut req): Json<ChatRequest>,
) -> Response {
    hydrate_conversation_id_from_header_if_missing(&headers, &mut req);

    let shared_cfg = &state.agent_config.shared_context;
    if shared_cfg.enabled {
        if let Some(assembler) = state.penny_assembler.as_ref() {
            match assembler
                .build(&AuthenticatedContext, req.conversation_id.as_deref(), ctx.request_id)
                .await
            {
                Ok(bootstrap) => {
                    let block = render_penny_shared_context(
                        &bootstrap,
                        shared_cfg.digest_summary_chars as usize,
                    );
                    let injection_mode = inject_shared_context_block(&mut req.messages, &block);
                    tracing::info!(target: "penny_shared_context.inject", ...);
                }
                Err(e) => {
                    tracing::warn!(target: "penny_shared_context", error = %e, ...);
                }
            }
        }
    }

    quota_precheck();
    router_dispatch();
}
```

규칙:

- conversation_id hydration 은 assemble 전에 수행된다.
- `enabled = false` 면 assembler 자체를 호출하지 않는다.
- `state.penny_assembler = None` 이면 request 는 원래 shape 로 계속 간다. current trunk 는 이를 503 으로 승격하지 않는다.
- assembler 실패는 graceful degrade 다. warn trace 후 원래 request 로 계속 진행한다.
- provider/model narrowing 은 현재 helper branch 의 gating 조건이 아니다. handler 는 `req.model == "penny"` 를 보지 않는다. 이것은 current trunk reality 이며 follow-up review item 이다.

신뢰 경계 입력표:

| 입력 | 경계 | 검증 규칙 |
|---|---|---|
| `HeaderMap` | HTTP client -> gateway | `x-gadgetron-conversation-id` 는 body `conversation_id` 가 없을 때만 hydrate 하며, blank/oversized 값은 무시 |
| `ChatRequest.messages` | HTTP client -> gateway | helper 는 shape 를 보존하고, resume-time user-last invariant 는 `session.rs` 에서 최종 검증 |
| shared-surface digest | projection service -> renderer | actor-filtered summary-only payload 여야 하며 raw secret/path/stack trace 금지 |
| `TenantContext` | auth middleware -> handler | 기존 validated tenant/api_key/request_id 를 그대로 사용하며 재파싱하지 않음 |

#### 2.1.3 Stdin mode delivery contract (`gadgetron-penny::session`) [P2B]

현재 session driver 는 다음 mode semantics 를 고정한다.

| Mode | stdin payload 규칙 | system framing 전달 여부 |
|---|---|---|
| `FlattenHistory` | 모든 message 를 `Role: content` 쌍으로 flatten | 전달됨 |
| `NativeFirstTurn` | 첫 `System` message + 마지막 `User` message | 전달됨 |
| `NativeResumeTurn` | 마지막 `User` message only | 전달되지 않음 |

따라서 current trunk 에서 같은 "inject success" trace 라도 mode 별 의미는 다르다.

- `FlattenHistory`: gateway 가 넣은 block 이 모델에 간다.
- `NativeFirstTurn`: gateway 가 넣은 첫 system framing 이 모델에 간다.
- `NativeResumeTurn`: gateway 가 넣은 system framing 은 request-local transcript 에만 남고 Claude stdin 까지는 가지 않는다.

#### 2.1.4 Error surface [P2B]

현재 trunk 의 operator-facing behavior:

| 상황 | 현재 HTTP 동작 | operator-visible signal |
|---|---|---|
| `enabled = false` | request 원형 그대로 계속 진행 | inject trace 부재 |
| `state.penny_assembler = None` | request 원형 그대로 계속 진행 | inject trace 부재 |
| assembler build error | request 원형 그대로 계속 진행 | `target = "penny_shared_context"` warn event |
| `NativeResumeTurn` shape 오류 | downstream Penny path 에서 400 계열 `penny_tool_invalid_args` 가능 | session.rs validation error |
| partial shared-surface failure | assembler 가 degraded bootstrap 을 render 후 계속 진행 | inject trace 의 `health = degraded`, `degraded_reasons > 0` |

원칙:

- current trunk 는 `penny_shared_context_unconfigured` 같은 dedicated gateway error code 를 아직 제공하지 않는다.
- "shared context 미적용" 과 "chat request 전체 실패"를 같은 것으로 취급하면 안 된다.
- 문서와 runbook 은 inject trace 유무를 성공/미적용의 1차 관찰면으로 사용해야 한다.

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
        +--> if shared_context.enabled && penny_assembler.is_some()
        |         |
        |         +--> assembler.build(...)
        |         +--> render_penny_shared_context(...)
        |         +--> inject_shared_context_block(&mut req.messages, ...)
        |         +--> info/warn tracing
        |
        +--> quota pre-check
        |
        v
router.chat() / router.chat_stream()
        |
        v
PennyProvider (or other provider chosen later)
        |
        v
ClaudeCodeSession
        |
        v
build_stdin_payload(FlattenHistory | NativeFirstTurn | NativeResumeTurn)
```

중요한 점:

- gateway injection 은 provider dispatch 전에 끝난다.
- 현재 helper 는 request-local transcript shape 를 바꾸지만, provider/session mode semantics 를 바꾸지는 않는다.
- 같은 request 에서 "gateway transcript 상 inject 성공"과 "Claude stdin 상 inject 전달 성공"을 구분해서 말해야 한다.

#### 2.2.2 Storage boundary [P2B]

injected block 에 대한 저장 경계:

- raw client body 를 디스크에 rewrite 해서 저장하지 않는다.
- session store 는 Claude continuity 메타데이터를 관리할 뿐, bootstrap truth 저장소가 아니다.
- audit/trace 는 rendered block 전문 대신 request_id, health, count, injection_mode 같은 요약만 남긴다.

이 규칙이 필요한 이유:

1. truth source 가 subprocess-local jsonl 로 미끄러지는 것을 막는다.
2. retry 마다 shared-context block 이 누적 저장되는 prompt 폭주를 막는다.
3. operator 가 user-authored content 와 gateway-injected system framing 을 분리해서 이해할 수 있게 한다.

#### 2.2.3 Freshness / invalidation boundary [P2B]

current trunk 의 invalidation semantics:

1. gateway 는 `conversation_id` 존재 여부와 무관하게 매 요청 assembler 를 다시 호출한다.
2. out-of-band direct action, candidate 변화, approval 변화는 다음 요청에서 즉시 새 bootstrap source 가 된다.
3. session store hit, `SessionStore::touch()`, `--resume` flag 는 freshness cache 가 아니다.
4. current P2B slice 는 bootstrap fingerprint 를 session jsonl 안에 reconcile 하지 않는다.

즉:

> **freshness invalidation 은 gateway assembler 가 책임지고, native resume delivery 는 아직 그 보장 범위에 포함되지 않는다.**

#### 2.2.4 Partial failure behavior [P2B]

세 가지 경우를 구분한다.

1. **Disabled / unwired**
   `enabled = false` 또는 `penny_assembler = None`.
   결과: inject 시도 자체를 건너뛴다.
2. **Assembler build failure**
   read path 또는 identity/path 내부 에러.
   결과: warn trace 후 원래 request 로 계속 진행한다.
3. **Assembler success with degraded health**
   partial read failure 가 digest 에 반영된 경우.
   결과: degraded bootstrap block 을 inject 하고 계속 진행한다.

Load-bearing 규칙:

- "degraded" 와 "skipped" 는 서로 다른 상태다.
- `require_explicit_degraded_notice = true` 는 degraded block 렌더링에만 적용된다.
- current trunk 는 skipped state 를 별도 error code 로 노출하지 않는다.

### 2.3 설정 스키마 [P2B]

현재 `[agent.shared_context]` shape:

```toml
[agent.shared_context]
enabled = true
bootstrap_activity_limit = 6
bootstrap_candidate_limit = 4
bootstrap_approval_limit = 3
digest_summary_chars = 240
require_explicit_degraded_notice = true
```

Env override convention:

- `GADGETRON_AGENT_SHARED_CONTEXT_ENABLED`
- `GADGETRON_AGENT_SHARED_CONTEXT_BOOTSTRAP_ACTIVITY_LIMIT`
- `GADGETRON_AGENT_SHARED_CONTEXT_BOOTSTRAP_CANDIDATE_LIMIT`
- `GADGETRON_AGENT_SHARED_CONTEXT_BOOTSTRAP_APPROVAL_LIMIT`
- `GADGETRON_AGENT_SHARED_CONTEXT_DIGEST_SUMMARY_CHARS`
- `GADGETRON_AGENT_SHARED_CONTEXT_REQUIRE_EXPLICIT_DEGRADED_NOTICE`

필드별 semantics:

| 필드 | default | 의미 |
|---|---|---|
| `enabled` | `true` | emergency rollback switch. `false` 면 inject 자체를 skip 한다 |
| `bootstrap_activity_limit` | `6` | activity digest 최대 개수 |
| `bootstrap_candidate_limit` | `4` | candidate digest 최대 개수 |
| `bootstrap_approval_limit` | `3` | approval digest 최대 개수 |
| `digest_summary_chars` | `240` | 각 summary/title clipping 상한 |
| `require_explicit_degraded_notice` | `true` | degraded 숨김 금지. `false` 는 validation error |

규칙:

- `enabled = false` 와 `require_explicit_degraded_notice = false` 는 의미가 다르다.
- `enabled = false` 는 rollback 이고, validation error 가 아니다.
- `require_explicit_degraded_notice = false` 는 design rule 위반이며 validation error 다.

### 2.4 에러 & 로깅 [P2B]

#### 2.4.1 tracing contract [P2B]

현재 landed success event:

```rust
tracing::info!(
    target: "penny_shared_context.inject",
    request_id = %ctx.request_id,
    health = ?bootstrap.health,
    degraded_reasons = bootstrap.degraded_reasons.len(),
    rendered_bytes = block.len(),
    injection_mode = %injection_mode,
    "shared context block injected"
);
```

현재 landed failure event:

```rust
tracing::warn!(
    target: "penny_shared_context",
    request_id = %ctx.request_id,
    error = %e,
    "penny_shared_context.build_failed — degrading gracefully"
);
```

추가 규칙:

- rendered block 전문은 trace field 로 남기지 않는다.
- skipped (`enabled = false` / `penny_assembler = None`) 는 current trunk 에 별도 trace event 가 없다.
- runbook 은 inject info event 존재 여부와 warn event 존재 여부를 함께 봐야 한다.

#### 2.4.2 Audit / quota contract [P2B]

- injection 시도는 quota pre-check 전에 일어난다.
- current quota implementation 은 request-local mutated shape 뒤에서 계속 진행하지만, 별도 bootstrap 전용 quota/audit 분기점은 없다.
- audit path 는 기존 request_id, tenant_id, api_key_id, model 기준을 재사용한다.
- raw injected block full text 를 audit payload 에 남기지 않는다.

#### 2.4.3 STRIDE 요약 [P2B]

| 자산 / 경계 | 신뢰 경계 | 위협 | 완화 |
|---|---|---|---|
| `HeaderMap` / `conversation_id` | HTTP client -> gateway | 다른 대화 id 위조, oversized header, trace 혼선 | blank/oversized 값 무시, request_id correlation |
| shared-surface digest | projection service -> renderer | 타 actor digest 노출, giant digest, internal string 누출 | actor-filtered projection, char cap, degraded summary only |
| synthetic system framing | gateway handler -> provider | wrong request 에 주입, duplicate prepend, system/user 경계 혼선 | request-local mutation only, deterministic insertion rules, no persistent storage |
| native resume boundary | gateway transcript -> Claude stdin | fresh digest 가 실제 stdin 에 미도달, stale session 을 truth 로 오인 | every-turn reassembly, session store is continuity only, PSL-1c follow-up 명시 |

컴플라이언스 매핑:

- SOC2 CC6.1: actor-filtered ingress + least privilege shared-context projection
- SOC2 CC6.7: inject/warn tracing correlation
- GDPR Art 32: secret/path/stack trace 를 injected block 에 넣지 않음
- HIPAA §164.312: PHI-specific contract 는 현재 범위 밖

#### 2.4.4 Runbook / Troubleshooting [P2B]

| 증상 | 확인 포인트 | 원인 | 운영자 조치 |
|---|---|---|---|
| inject trace 가 전혀 없음 | `enabled`, `penny_assembler` wiring | feature disabled 또는 wiring 누락 | config 및 startup wiring 확인 |
| warn trace 후 응답은 정상 | `target = "penny_shared_context"` warn | assembler build failure | projection source / request_id 기준 원인 확인 |
| same `conversation_id` 인데 Penny 가 최신 direct action 을 모름 | gateway inject trace 존재 여부 + session mode | native resume only path 에서 system framing 미전달 | stateless/native first path 로 재현해 분리 확인, PSL-1c 추적 |
| degraded awareness | inject trace 의 `health`, `degraded_reasons` | partial shared-surface failure | activity/candidate/approval source health 확인 |

### 2.5 의존성 [P2B]

- 신규 외부 crate 추가 없음
- 현재 load-bearing crate boundary:
  - `gadgetron-core::agent::config`
  - `gadgetron-core::agent::shared_context`
  - `gadgetron-gateway::penny::shared_context`
  - `gadgetron-gateway::handlers`
  - `gadgetron-penny::session`
  - `gadgetron-xaas` audit/quota existing hooks

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 크레이트 경계 [P2B]

| 위치 | 소유 책임 | 이유 |
|---|---|---|
| `gadgetron-core::agent::shared_context` | bootstrap DTO / trait | shared wire shape, D-12 준수 |
| `gadgetron-core::agent::config` | `SharedContextConfig` | config 는 core 소유 |
| `gadgetron-gateway::penny::shared_context` | assembler + renderer | request_id/auth/tenant truth boundary 가 gateway 이기 때문 |
| `gadgetron-gateway::handlers` | actual injection helper + trace contract | landed runtime orchestration 위치이기 때문 |
| `gadgetron-penny::session` | stdin mode semantics | subprocess lifecycle 소유 |
| `gadgetron-router` | provider dispatch | injection 이후 generic routing 유지 |

본 문서는 D-12 관점에서 shared DTO/config 는 core 에, ingress orchestration 은 gateway 에, stdin mode semantics 는 penny crate 에 남기는 것을 기본선으로 둔다.

### 3.2 데이터 흐름 다이어그램 [P2B]

```text
Browser / SDK
   |
   v
POST /v1/chat/completions
   |
   v
gateway auth + tenant context
   |
   +--> DefaultPennyTurnContextAssembler::build(...)
   |        |
   |        +--> recent activity
   |        +--> pending candidates
   |        +--> pending approvals
   |
   +--> render_penny_shared_context(...)
   |
   +--> inject_shared_context_block(&mut req.messages, ...)
   |
   +--> quota pre-check
   |
   v
router.chat_stream() / chat()
   |
   v
provider dispatch
   |
   v
ClaudeCodeSession
   |
   v
build_stdin_payload(FlattenHistory | NativeFirstTurn | NativeResumeTurn)
```

### 3.3 타 도메인 인터페이스 계약 [P2B]

- `gateway-router-lead`
  - `/v1/chat/completions` namespace, middleware order, tracing target 를 보존한다.
- `inference-engine-lead`
  - provider/session mode semantics 와 injected message ordering 간 충돌을 문서화해야 한다.
- `xaas-platform-lead`
  - quota/audit 는 inject 이후에도 기존 request lifecycle 안에서 동작한다.
- `qa-test-architect`
  - "gateway inject 성공"과 "Claude stdin 전달 성공"을 별도 assertion 으로 분리한다.
- `security-compliance-lead`
  - injected block / warn trace 어느 쪽에도 secret-bearing detail 이 들어가지 않도록 검증한다.

### 3.4 D-12 크레이트 경계표 준수 [P2B]

준수 판단:

- shared DTO/config stays in `gadgetron-core`
- actual HTTP mutation stays in `gadgetron-gateway`
- stdin mode logic stays in `gadgetron-penny`
- reverse dependency 추가 없음

따라서 현재 문서화된 slice 는 D-12 를 준수한다.

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위 [P2B]

| 대상 | 검증할 invariant |
|---|---|
| `inject_shared_context_block()` | text `System` message 가 있으면 prepend, 없으면 insert |
| `inject_shared_context_block()` + `Content::Parts` | structured system parts 를 파괴하지 않고 새 system message 를 앞에 삽입 |
| handler skip path | `enabled = false` 또는 `penny_assembler = None` 일 때 request 는 계속 진행되고 inject trace 가 생기지 않음 |
| handler degrade path | assembler error 가 5xx 를 만들지 않고 warn trace 만 남김 |
| every-turn reassembly | 같은 `conversation_id` 로 두 번 호출해도 assembler 가 두 번 실행됨 |
| `build_stdin_payload(NativeFirstTurn)` | 첫 system framing + 마지막 user message 를 전달 |
| `build_stdin_payload(NativeResumeTurn)` | 마지막 user message only. injected system framing 은 전달되지 않음 |
| config validation | `require_explicit_degraded_notice = false` 는 validation fail |
| trace redaction | rendered block 전문과 secret-bearing string 이 trace/audit 에 남지 않음 |

### 4.2 테스트 하네스 [P2B]

- gateway unit test:
  - fake `PennySharedSurfaceService`
  - fixed `TenantContext`
  - fixed `ChatRequest`
- penny session unit test:
  - existing `build_stdin_payload()` tests 재사용
  - injected system framing 이 `NativeResumeTurn` 에서 사라지는 현재 contract 를 negative control 로 고정
- tracing assertion:
  - existing subscriber harness 로 target, field, injection_mode 확인
- deterministic controls:
  - fixed `Uuid`
  - fixed bootstrap fixture ordering
  - `render_penny_shared_context()` pure function snapshot 재사용 가능

property-based test 필요 여부:

- 필수는 아니지만 `digest_summary_chars` clipping 과 prepend/insert 동작이 다국어 텍스트에서 panic 없이 동작하는지 property-based test 추가 가능

### 4.3 커버리지 목표 [P2B]

- `handlers.rs` injection helper + branch coverage 90%+
- `session.rs` affected resume/first-turn branches 100%
- `render_penny_shared_context()` clipping/degraded formatting 90%+
- perf verification:
  - existing `crates/gadgetron-gateway/benches/middleware_chain.rs` 위에 inject on/off delta 를 얹어 P99 < 1 ms overhead 재확인

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위 [P2B]

- 함께 테스트할 크레이트:
  - `gadgetron-gateway`
  - `gadgetron-penny`
  - `gadgetron-core`
  - `gadgetron-xaas`

통합 시나리오:

1. **stateless or first-turn injects system framing**
   fake assembler returns activity digest -> provider receives request whose first system frame contains `<gadgetron_shared_context>`.
2. **same conversation_id still rebuilds**
   두 요청 모두 gateway inject trace 가 발생하고 assembler call count 가 2 다.
3. **disabled / unwired skip path**
   request 는 200 으로 계속 가고 inject trace 가 없다.
4. **assembler failure degrades gracefully**
   warn trace 는 생기지만 chat response 는 계속 나온다.
5. **native resume boundary control**
   gateway request object 에 system framing 이 존재해도 `build_stdin_payload(NativeResumeTurn)` 결과에는 포함되지 않는다.
6. **degraded bootstrap render**
   partial failure fixture 로 `health: degraded` block 이 inject 된다.

### 5.2 테스트 환경 [P2B]

- external dependency 없음
- in-memory fake router/provider 충분
- existing gateway handler harness 재사용
- session resume control 은 `gadgetron-penny` unit/integration fixture 로 분리

### 5.3 회귀 방지 [P2B]

다음 변경은 반드시 테스트 또는 문서 재승인을 요구해야 한다.

- gateway 가 same `conversation_id` 에서 assembler 호출을 생략하는 변경
- injected system framing 을 trace/audit full text 로 남기는 변경
- `NativeResumeTurn` 이 더 이상 last-user-only 가 아니게 바뀌었는데 문서가 갱신되지 않는 변경
- skip/degrade/unwired 상태를 operator 가 구분할 수 없게 만드는 변경

---

## 6. Phase 구분

- [P2B]
  - gateway per-request bootstrap reassembly
  - synthetic system framing injection (`prepend_to_system` / `insert_new_system`)
  - stateless / native first-turn delivery
  - warn-based graceful degrade
- [P2C]
  - request-scoped `workbench.*` gadget bridge hardening
  - richer audit correlation between injected digest and downstream tool calls
- [P2C+]
  - native resume delivery correction
  - Penny-only branch narrowing at handler/provider boundary

---

## 7. 오픈 이슈 / 의사결정 필요

| ID | 내용 | 옵션 | 추천 | 상태 |
|---|---|---|---|---|
| Q-1 | injected shared-context block 을 future request-scoped `workbench.*` gadget bridge 와 어떻게 상관시킬 것인가 | A. 현행 system framing 유지 / B. internal envelope 타입 도입 / C. provider trait 확장 | A — current landed slice 는 framing contract 만 고정하고 gadget bridge 는 P2C 로 분리 | ⚪ follow-up, 비차단 |
| Q-2 | injection branch 를 언제 `req.model == "penny"` 로 좁힐 것인가 | A. 현행 assembler/config gate 유지 / B. handler 단계 Penny-only gate 추가 / C. provider-local gate 로 이동 | B — non-Penny drift 방지를 위해 explicit Penny-only gate 가 바람직하나 current trunk 구현은 아님 | 🟡 follow-up |
| Q-3 | native resume turn 에서 fresh shared context 를 어떻게 실제 stdin 까지 전달할 것인가 | A. 현행 system framing 유지 + first/resume 의미 분리 / B. latest user turn rewrite 로 전환 / C. session/provider contract 확장 | B — current session semantics 와 가장 잘 맞지만 별도 reviewed change 가 필요 | 🟡 PSL-1c follow-up |

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

### Round 1 (Conformance Amendment) — 2026-04-18 — @inference-engine-lead @xaas-platform-lead
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
- A1: current landed helper 가 `handlers.rs::inject_shared_context_block()` 기반 system prepend/insert 임을 본문과 보정 범위에 명시했다.
- A2: 문서가 더 이상 미존재 `turn_prep` module, Penny-only branch, latest-user rewrite 를 trunk reality 로 서술하지 않도록 교정했다.
- A3: gateway reassembly 보장과 native resume delivery 보장을 분리해 `session.rs::StdinMode` 진실표를 추가했다.

**Open Questions**:
- Q-2, Q-3 follow-up 유지.

**다음 라운드 조건**: 없음. Round 1.5 진행 가능.

### Round 1.5 (Conformance Amendment) — 2026-04-18 — @security-compliance-lead @dx-product-lead
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
- A1: current trunk 가 dedicated 503 code 없이 trace/warn 기반으로 skip/degrade 를 드러낸다는 점을 operator runbook 에 반영했다.
- A2: synthetic system framing 과 native resume boundary 를 별도 위협 항목으로 분리해 hidden-memory 오해를 줄였다.
- A3: `enabled = false` rollback 과 degraded notice invariant 를 혼동하지 않도록 config semantics 를 재정리했다.

**Open Questions**:
- 없음

**다음 라운드 조건**: 없음. Round 2 진행 가능.

### Round 2 (Conformance Amendment) — 2026-04-18 — @qa-test-architect
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
- A1: 테스트 범위를 current helper invariant (`prepend_to_system` / `insert_new_system`) 중심으로 교정했다.
- A2: "gateway inject 성공"과 "`NativeResumeTurn` stdin 전달 성공"을 별도 assertion 으로 분리했다.
- A3: skip/unwired/degraded/current-resume-boundary 시나리오를 통합 테스트 계획에 추가했다.

**Open Questions**:
- 없음

**다음 라운드 조건**: 없음. Round 3 진행 가능.

### Round 3 (Conformance Amendment) — 2026-04-18 — @chief-architect
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
- A1: runtime authority 를 실제 landed 파일인 `handlers.rs` / `session.rs` 로 다시 고정했다.
- A2: core/gateway/penny crate 경계가 현재 code ownership 과 일치하도록 표와 다이어그램을 정정했다.
- A3: current trunk 가 보장하지 않는 resume-safe delivery 를 architecture promise 로 오인하지 않도록 Q-3 로 분리했다.

**Open Questions**:
- 없음

**다음 라운드 조건**: 없음. 최종 승인 가능.

### 최종 승인 (Conformance Amendment) — 2026-04-18 — PM
**결론**: Approved

기존 승인 로그는 append-only 로 유지하고, 이번 revision 은 current `origin/main` implementation conformance correction 으로 별도 라운드에서 재검토했다. 본 문서는 이제 recent landed code, D-20260418-16 scope, 그리고 PSL-1c follow-up boundary를 서로 충돌 없이 설명한다.
