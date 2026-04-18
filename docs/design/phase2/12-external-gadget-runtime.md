# 12 — External Gadget Runtime

> **담당**: PM (Codex)
> **상태**: Approved
> **작성일**: 2026-04-18
> **최종 업데이트**: 2026-04-18
> **관련 크레이트**: `gadgetron-core`, `gadgetron-penny`, `gadgetron-gateway`, `gadgetron-xaas`, future `bundles/*`
> **Phase**: [P2B-alpha] / [P2B-beta] / [P2C]
> **관련 문서**: `docs/adr/ADR-P2A-10-bundle-plug-gadget-terminology.md`, `docs/adr/ADR-P2A-10-ADDENDUM-01-rbac-granularity.md`, `docs/design/phase2/10-penny-permission-inheritance.md`, `docs/design/core/knowledge-plug-architecture.md`, `docs/process/04-decision-log.md` D-20260418-07, D-20260418-08

---

## 1. 철학 & 컨셉 (Why)

### 1.1 해결하는 문제

Gadgetron 은 knowledge-first platform 이지만, 모든 capability 를 in-process Rust crate 로만 수용할 수는 없다. `graphify` 같은 graph/query utility, OCR/Whisper class tool, domain-specific visual gadget 은 **Penny-facing Gadget** 으로는 유용하지만, 코어 바이너리에 직접 링크할 이유가 없거나 오히려 보안·배포 리스크가 크다.

현재 문제는 세 가지다.

1. 외부 Gadget runtime 이 **누구의 권한으로** 동작하는지가 문서상 고정돼 있지 않다.
2. 외부 runtime 이 **어디까지의 filesystem / network / CPU / memory** 를 볼 수 있는지가 명확하지 않다.
3. 외부 runtime 호출이 audit / BI / incident review 에서 **append-only evidence** 로 남는 경로가 아직 load-bearing 문서로 닫혀 있지 않다.

이 문서는 그 공백을 메운다.

### 1.2 제품 비전과의 연결

`docs/00-overview.md §1` 의 Gadgetron 은 "지식 협업 플랫폼" 이다. 따라서 외부 runtime 도 generic plugin sandbox 가 아니라, **지식 작업과 operator control 을 확장하는 Gadget runtime** 이어야 한다. 이 문서는 다음 두 원칙을 동시에 지킨다.

- core 는 source-of-truth, identity, audit, policy 를 소유한다
- external Bundle 은 capability 를 기여하지만 caller 권한을 넘겨받아 대리 실행할 뿐이다

즉, 외부 runtime 은 "권한이 있는 작은 서버" 가 아니라 **core 가 통제하는 실행 슬롯** 이다.

### 1.3 고려한 대안과 채택하지 않은 이유

| 대안 | 장점 | 채택하지 않은 이유 |
|---|---|---|
| A. 외부 Gadget 금지, 전부 in-process Rust 만 허용 | 가장 단순 | graphify/OCR/third-party utility 확장성이 막힌다 |
| B. subprocess 문자열 실행만 허용 | 구현 빠름 | identity, audit, egress, resource ceiling 이 전부 흐려진다 |
| C. runtime 종류별 ad-hoc 문서 작성 | 초기엔 빠름 | subprocess/http/container/wasm 이 제각각 drift 한다 |
| D. core-defined runtime contract + per-kind adapter | 일관성, audit, policy, 확장성 확보 | 구현 계층이 추가되지만 장기적으로 정합성이 가장 높다 |

채택: **D. core-defined runtime contract + per-kind adapter**

### 1.4 핵심 설계 원칙과 trade-off

1. **Caller inheritance is absolute**
   외부 runtime 의 실행 주체는 항상 caller user 다. daemon identity 나 operator credential 을 직접 받지 않는다.
2. **Runtime is capability, not authority**
   외부 Gadget 은 일을 대신할 뿐, policy bypass 수단이 아니다.
3. **Default deny on everything except declared needs**
   workdir, egress, resource ceiling, transport 모두 명시 없이는 열리지 않는다.
4. **Audit must be persistent, structured, and append-only**
   stdout tracing 만으로는 release gate 를 통과하지 못한다.
5. **Subprocess first, but not subprocess only**
   P2B-alpha 는 subprocess 를 first-class 로 구현하되, HTTP/container/wasm 도 계약상 같은 floor 를 가진다.

Trade-off:

- 런타임 계약이 다소 무겁고 구현 난도가 올라간다.
- 그러나 이 비용을 문서화 단계에서 지불하지 않으면, knowledge plug / graphify pilot / audit evidence / security review 가 매 PR 마다 다시 흔들린다.

---

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API

```rust
// gadgetron-core::bundle::runtime

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    bundle::{BundleName, GadgetName},
    error::GadgetronError,
    identity::{Role, TeamSet, TenantId, UserId},
};

pub type RuntimeResult<T> = Result<T, GadgetronError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalRuntimeKind {
    Subprocess,
    Http,
    Container,
    Wasm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTransport {
    McpStdio,
    McpHttp,
    JsonRpcStdio,
    JsonRpcHttp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeResourceLimits {
    pub memory_mb: u32,
    pub open_files: u32,
    pub cpu_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeEgressPolicy {
    pub allow: Vec<String>, // host:port
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalRuntimeSpec {
    pub kind: ExternalRuntimeKind,
    pub transport: RuntimeTransport,
    pub entry: String,
    pub args: Vec<String>,
    pub workdir_subpath: Option<String>,
    pub limits: RuntimeResourceLimits,
    pub egress: RuntimeEgressPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalInvocationEnvelope {
    pub bundle: BundleName,
    pub gadget: GadgetName,
    pub request_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub caller_user_id: UserId,
    pub tenant_id: TenantId,
    pub role: Role,
    pub teams: Vec<String>,
    pub args: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeMetaV1 {
    pub kind: ExternalRuntimeKind,
    pub bundle: String,
    pub version: String,
    pub transport: RuntimeTransport,
}

#[derive(Debug, Clone)]
pub struct RuntimeLaunchContext {
    pub spec: ExternalRuntimeSpec,
    pub workdir: PathBuf,
    pub env: BTreeMap<String, String>,
    pub audit_meta: RuntimeMetaV1,
}

#[async_trait::async_trait]
pub trait ExternalRuntimeLauncher: Send + Sync + std::fmt::Debug {
    fn kind(&self) -> ExternalRuntimeKind;

    async fn invoke(
        &self,
        launch: RuntimeLaunchContext,
        envelope: ExternalInvocationEnvelope,
    ) -> RuntimeResult<serde_json::Value>;
}
```

### 2.2 내부 구조

핵심 구성요소는 네 층으로 분리한다.

1. `BundleRegistry` / `GadgetRegistry`
   bundle manifest 와 gadget declaration 에서 `ExternalRuntimeSpec` 를 해석한다.
2. `ExternalRuntimeLauncher`
   `kind` 별 adapter 를 선택하지만, caller envelope / audit meta / fail-closed validation 은 공통 경로로 유지한다.
3. `RuntimeGuard`
   canonicalized workdir, limits, egress policy, injected env 를 조립한다. 이 계층이 trust-boundary validator 다.
4. `AuditWriter`
   launch lifecycle event 와 `external_runtime_meta` 를 append-only sink 에 기록한다.

상태 전이는 단순해야 한다.

```text
resolved -> validated -> launched -> completed
                 |           |
                 v           v
               denied      failed
```

- `resolved`: bundle/gadget 가 runtime spec 로 매핑됨
- `validated`: workdir, limits, egress, identity floor 통과
- `launched`: per-kind adapter 가 실제 invoke 시작
- `completed`: caller-facing payload 생성
- `denied`: launch 전 policy/config 단계에서 fail-closed
- `failed`: launch 후 transport/runtime 단계에서 실패

동시성 모델:

- registry read path 는 immutable config snapshot 기반으로 동작한다.
- invocation state 는 request-local 이며 cross-request shared mutable state 를 만들지 않는다.
- audit write 는 비동기 sink 이지만, "쓰기 예약 성공" 이 아니라 "persistent append 성공" 을 completion 조건으로 본다.

### 2.3 런타임 종류별 floor

- **Subprocess**: P2B-alpha ship target. `Command::current_dir(workdir)` + strict env injection + stdio transport.
- **Http**: P2B-beta. loopback or allowlisted internal endpoint only. signed invocation envelope required.
- **Container**: P2C. same identity/audit/limits floor but runtime isolation stronger.
- **Wasm**: P2C/P3. host capability imports only through explicit views.

중요한 점은 구현 순서와 계약 순서를 분리하는 것이다. P2B-alpha 는 subprocess 를 구현해도, doc 12 는 네 가지 모두에 **같은 security/audit floor** 를 부여한다.

### 2.4 `bundle.toml` / `gadgetron.toml` 계약

```toml
# bundle.toml
[bundle.runtime]
kind = "subprocess"
transport = "mcp_stdio"
entry = "graphify-mcp"
args = ["serve"]

[bundle.runtime.limits]
memory_mb = 2048
open_files = 256
cpu_seconds = 300

[bundle.runtime.egress]
allow = ["api.openai.com:443"]
```

```toml
# gadgetron.toml
[bundles.graphify.runtime]
enabled = true
workdir = "/var/lib/gadgetron/bundles/graphify"

[bundles.graphify.runtime.limits]
memory_mb = 1024   # operator can tighten, never loosen beyond bundle floor in P2B-alpha
open_files = 128
cpu_seconds = 180

[bundles.graphify.runtime.egress]
allow = ["api.openai.com:443"]
```

검증 규칙:

- `limits` 가 bundle/operator 어디에도 없으면 startup fail-closed
- `egress.allow` 가 비어 있으면 default-deny, no-network 로 해석
- `workdir` 는 canonicalize 후 tenant root 하위인지 확인
- `kind=subprocess` 에서 `transport=mcp_http` 같은 불일치 조합은 config error

운영자 touchpoint:

- `gadgetron bundle info <bundle>` 는 effective runtime kind / transport / limits / egress / workdir root 를 출력해야 한다.
- daemon startup validation 에서 fail-closed 되면, 에러 메시지는 반드시 "무엇이 잘못됐는지 / 왜 막혔는지 / 어떻게 고칠지" 3요소를 포함해야 한다.
- graphify 같은 pilot bundle 은 dry-run validation 경로를 제공해, 실제 launch 전 manifest 와 operator override 정합성을 확인할 수 있어야 한다.

### 2.5 Identity / filesystem / egress / ceiling enforcement

`ADR-P2A-10-ADDENDUM-01 §5` 의 7 floor 를 wire contract 로 고정한다.

1. **Credentials**: `ExternalInvocationEnvelope` 는 caller identity 만 전달한다.
2. **Workdir**: `GadgetronBundlesHome::tenant_workdir(tenant_id, bundle)` 하위만 허용한다.
3. **Filesystem view**: runtime 은 `TMPDIR=$WORKDIR/.tmp` 를 쓰고, wiki/blob 접근은 direct path 가 아니라 core-provided read views 로 제한한다.
4. **Tenant identity**: env 와 envelope 둘 다 `tenant_id` 를 싣되, runtime 응답 전 core 가 mismatch 를 검사한다.
5. **Audit principal**: subject 는 항상 caller. runtime 은 metadata다.
6. **Resource ceilings**: subprocess 는 RLIMIT + cgroup v2, macOS dev 는 best-effort ulimit/noop fallback 문서화.
7. **Egress policy**: default-deny. Linux 는 namespace+nftables, macOS dev 는 app-layer proxy fallback.

### 2.6 Audit sink와 `external_runtime_meta`

이 문서의 MUST-LAND gate 두 개:

1. `target: "gadgetron_audit"` 이벤트가 **append-only persistent sink** 로 흘러야 한다.
2. `external_runtime_meta` 는 임의 JSON 이 아니라 `RuntimeMetaV1` strict validator 를 거쳐야 한다.

DB shape:

```sql
ALTER TABLE tool_audit_events
  ADD COLUMN external_runtime_meta JSONB NULL;
```

writer contract:

- in-core Gadget: `external_runtime_meta = NULL`
- external Gadget: strict-serialized `RuntimeMetaV1`
- unknown field / oversized string / invalid enum value -> write reject + runtime invocation fail

### 2.7 에러 & 로깅

- `GadgetronError::Config`: invalid runtime kind/transport, missing limits, invalid workdir
- `GadgetronError::Penny::GadgetIntegrity` 또는 equivalent gadget/runtime integrity error: tenant/workdir mismatch, signature mismatch, canonicalize escape
- `GadgetError::{Denied, Execution, GadgetNotAvailable}` 는 caller-facing surface. `admin_detail` 은 `render_gadget_error_for_caller()` choke-point 에서만 admin 노출

caller-facing error catalog 원칙:

- config 실패: "external runtime 설정이 유효하지 않습니다 / limits 또는 workdir 규칙을 통과하지 못했습니다 / `bundle.toml` 또는 `gadgetron.toml` 의 해당 필드를 수정하세요"
- egress 차단: "외부 Gadget 네트워크 접근이 차단되었습니다 / allowlist 에 없는 목적지입니다 / operator 가 runtime egress allowlist 를 검토하세요"
- runtime unavailable: "외부 Gadget 을 지금 실행할 수 없습니다 / runtime transport 또는 dependency 가 준비되지 않았습니다 / audit event 와 bundle diagnostics 를 확인하세요"

필수 tracing/audit events:

- `external_runtime_launch_started`
- `external_runtime_launch_denied`
- `external_runtime_egress_blocked`
- `external_runtime_completed`
- `external_runtime_audit_persist_failed`

STRIDE 요약:

| 자산 | 신뢰 경계 | 위협 | 완화 |
|---|---|---|---|
| caller identity / tenant context | core -> external runtime envelope | Spoofing | envelope 는 core 가 생성, runtime 응답의 tenant mismatch 는 reject |
| runtime config / workdir | bundle.toml + gadgetron.toml -> launcher | Tampering | canonicalize, kind/transport validation, tenant root escape 차단 |
| audit evidence | runtime lifecycle -> audit sink | Repudiation | append-only persistent sink + strict `RuntimeMetaV1` validator |
| wiki/blob data / provider key | core read views / env injection | Information Disclosure | raw path direct mount 금지, caller-scoped secret only, redaction layer |
| host resources | runtime process/container | Denial of Service | mandatory limits, RLIMIT/cgroup, default-deny egress |
| daemon/operator authority | caller -> Penny -> launcher | Elevation of Privilege | doc 10 inheritance 유지, daemon credential 전달 금지 |

컴플라이언스 매핑:

- SOC2 CC6.6: append-only audit sink 와 blocked egress evidence
- GDPR Art.32: tenant-isolated workdir, least-privilege identity inheritance
- HIPAA 164.312(b): runtime invocation / denial / completion 감사 기록

### 2.8 의존성

- `gadgetron-core`: 신규 외부 의존성 없음
- `gadgetron-penny` / runtime launcher layer:
  - subprocess path는 기존 `tokio` + stdlib `Command`
  - Linux namespace / cgroup integration은 platform-gated 구현체로 분리
- `gadgetron-xaas`:
  - JSONB validator는 existing `serde` / `sqlx` 재사용
  - 별도 free-form JSON crate 추가 금지

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 데이터 흐름

```text
caller -> gateway auth -> Penny / gadget dispatch
                         |
                         v
               BundleRegistry + GadgetRegistry
                         |
                         v
               ExternalRuntimeLauncher(kind-specific)
                         |
          +--------------+-------------+
          |              |             |
          v              v             v
       workdir        egress        audit sink
          |              |             |
          +--------------+-------------+
                         |
                         v
                  runtime response
```

### 3.2 타 모듈과의 인터페이스 계약

- `08-identity-and-users.md`: caller identity source
- `09-knowledge-acl.md`: external runtime does not bypass wiki/blob ACL
- `10-penny-permission-inheritance.md`: external runtime subject remains caller user
- `core/knowledge-plug-architecture.md`: graphify 는 external runtime pilot bundle candidate

### 3.3 D-12 크레이트 경계 준수

- runtime types / traits / ids: `gadgetron-core`
- launch implementation and transport adapters: `gadgetron-penny` or dedicated runtime layer crate
- audit persistence / JSONB write path: `gadgetron-xaas`
- bundle-specific manifests and entrypoints: `bundles/*`

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

- `runtime_spec_missing_limits_rejected`
- `workdir_escape_via_symlink_rejected`
- `runtime_meta_v1_rejects_unknown_fields`
- `runtime_meta_v1_rejects_oversized_bundle_name`
- `egress_allowlist_empty_means_default_deny`
- `subprocess_env_contains_caller_identity_but_not_raw_api_key`
- `render_gadget_error_hides_admin_detail_from_non_admin`

### 4.2 테스트 하네스

- workdir canonicalize tests use tempdir + symlink fixtures
- JSONB validator tests use `serde_json::from_value` deny-unknown-field path
- resource-limit config tests are pure parse/validate unit tests

### 4.3 커버리지 목표

- runtime config validator branch coverage 90% 이상
- `RuntimeMetaV1` strict validator + redaction choke-point branch coverage 95% 이상

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

- fake subprocess Gadget invocation end-to-end
- audit sink persistence end-to-end (`tool_audit_events.external_runtime_meta`)
- blocked egress path -> audit row -> caller-facing denial
- graphify-style bundle manifest parse -> launcher invoke -> response roundtrip

### 5.2 테스트 환경

- Linux CI job: subprocess + network namespace / nftables integration
- macOS dev fallback tests: skip-FD-scan / proxy-only mode
- Postgres testcontainers for audit persistence

### 5.3 회귀 방지

- limits declaration 제거 시 startup success 하면 실패
- arbitrary JSON 이 `external_runtime_meta` 에 들어가면 실패
- non-admin caller 에게 `admin_detail` 이 노출되면 실패
- tenant/workdir mismatch 를 허용하면 실패

---

## 6. Phase 구분

### 6.1 P2B-alpha

- subprocess runtime
- `RuntimeMetaV1` strict validator
- append-only audit sink wiring
- workdir canonicalize + limits + default-deny egress contract

### 6.2 P2B-beta

- HTTP runtime
- operator-visible `bundle info` runtime metadata / diagnostics
- graphify pilot bundle

### 6.3 P2C

- container runtime
- wasm runtime
- stronger platform-specific isolation and multi-tenant runtime policies

---

## 7. 오픈 이슈 / 의사결정 필요

| ID | 내용 | 옵션 | 추천 | 상태 |
|---|---|---|---|---|
| Q-1 | HTTP runtime signed envelope key rotation | A. startup-generated key / B. operator-supplied key | **A** v1, operator override later | 🟡 |
| Q-2 | Operator override 가 bundle-declared limits 를 얼마나 줄일 수 있나 | A. tighten-only / B. free override | **A** | 🟢 |
| Q-3 | macOS dev egress enforcement 기본값 | A. proxy-only / B. warn-only | **A** | 🟡 |
| Q-4 | graphify pilot 의 transport | A. subprocess stdio / B. loopback HTTP | **A** first | 🟢 |

---

## 8. Out of scope

- external Plug runtime (core-facing trait impl out-of-process) — 별도 문서
- full seccomp/apparmor profile catalog
- marketplace/install UX
- non-Gadget background daemon lifecycle orchestration

---

## 리뷰 로그 (append-only)

### Round 0 — 2026-04-18 — PM draft
**결론**: Draft v0. external runtime contract floor 정리.

**체크리스트**:
- [x] runtime kinds / transport / envelope
- [x] audit sink / RuntimeMetaV1
- [ ] review log / template finalization

### Round 1 — 2026-04-18 — @gateway-router-lead @xaas-platform-lead
**결론**: Conditional Pass

**체크리스트**:
- [x] 인터페이스 계약
- [x] 크레이트 경계
- [x] 의존성 방향
- [x] 레거시 결정 준수

**Action Items**:
- A1: audit sink 와 JSONB validator 를 MUST-LAND gate 로 명시
- A2: D-12 crate ownership 을 본문에 고정

### Round 1.5 — 2026-04-18 — @security-compliance-lead @dx-product-lead
**결론**: Pass

**체크리스트**:
- [x] 위협 모델
- [x] 신뢰 경계 입력 검증
- [x] 인증·인가
- [x] 시크릿/에러 누출 방지
- [x] 감사 로그
- [x] operator touchpoint
- [x] defaults 안전성

**Action Items**:
- 없음

### Round 2 — 2026-04-18 — @qa-test-architect
**결론**: Pass

**체크리스트**:
- [x] 단위 테스트 범위
- [x] 통합 시나리오
- [x] CI 재현성
- [x] 회귀 테스트

**Action Items**:
- 없음

### Round 3 — 2026-04-18 — @chief-architect
**결론**: Pass

**체크리스트**:
- [x] Rust 관용구
- [x] trait shape
- [x] 에러 전파
- [x] dependency discipline
- [x] observability

**Action Items**:
- 없음

### 최종 승인 — 2026-04-18 — PM
**결론**: Approved. graphify pilot 및 external Gadget runtime 구현의 기준 문서로 사용 가능.
