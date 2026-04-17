# ADR-P2A-08 — Multi-user + Knowledge ACL Foundation (P2B)

| Field | Value |
|---|---|
| **Status** | ACCEPTED (detailed designs in `docs/design/phase2/08-identity-and-users.md`, `09-knowledge-acl.md`, `10-penny-permission-inheritance.md`) |
| **Date** | 2026-04-18 |
| **Author** | PM (Claude) — user-directed via 2026-04-18 session (interview mode B) |
| **Parent docs** | `docs/process/04-decision-log.md` D-20260418-02; `docs/adr/ADR-P2A-05-agent-centric-control-plane.md`; `docs/adr/ADR-P2A-06-approval-flow-deferred-to-p2b.md`; `docs/adr/ADR-P2A-07-semantic-wiki-pgvector.md` |
| **Blocks** | P2B 의 multi-user 기능 전체 — 세 설계 문서 구현 PR |
| **Supersedes (partial)** | `docs/design/xaas/phase1.md` §1.1–§2.2 의 "API key ↔ Tenant 직결" 전제 — user 레이어 삽입으로 확장 |

---

## Context

Phase 2A 는 **단일 운영자** 가정으로 설계되었다. Phase 2A 후기 (2026-04-18 세션) 에서 사용자가 방향 확정:

> 큰 틀에서는 멀티유저, 공유지식, 지식접근권한을 기획해야합니다.

현재 상태의 gap:

1. **user 개념 부재**. `gadgetron-xaas` 는 `tenants` + `api_keys` 만 있고, API key 는 tenant 에 직결. "누가 이 행위를 했는가" 에 대한 persistent identity 없음
2. **wiki 공유 모델 없음**. `gadgetron-knowledge` 의 wiki 는 단일 path + 단일 git repo. "이 페이지는 내 개인 노트" / "이건 팀 런북" 같은 visibility 구분 불가
3. **Penny 권한 모델 미정**. Penny subprocess 가 작동 중 어떤 user 의 권한으로 tool 을 쓰는지 정의되지 않음. prompt injection 시 권한 상승 방어 불가
4. **ACL 필터링 없는 검색**. pgvector 하이브리드 검색은 ADR-P2A-07 에 설계되었으나 user-scope 필터 레이어 없음 — 모든 페이지가 모든 호출에 후보로 노출

Phase 2A 에는 이게 수용 가능했지만 (사용자 1인 = 모든 권한), 실제 사용 시나리오 (SRE 팀, 개발팀) 로 확장하려면 foundation 먼저 확정 필요. 이 결정은 **모든 plugin, wiki, MCP tool 에 걸쳐** 유효하므로 개별 plugin 설계 (07-plugin-server.md 등) 보다 **한 층 아래** 에 놓임.

## Decision

**P2B 에 multi-user (single-tenant) foundation 을 도입한다.** 8 개 sub-decision (D1–D8) 을 한 묶음으로 확정:

### 요약 테이블 (D-20260418-02 축약)

| ID | 결정 | 한 줄 |
|:---:|---|---|
| **D1** | `Tenant 1:N User 1:N ApiKey` | Key 는 user 소속. 모든 요청의 primary actor 는 user_id |
| **D2** | P2B = single-tenant multi-user | 기본 tenant `"default"` 자동. tenant_id NOT NULL placeholder. multi-tenant enforcement 는 P2C |
| **D7** | `teams` + `team_members` Postgres 테이블 | REST/UI 로 admin 관리. `admins` 는 built-in virtual team |
| **D8** | `users.role ∈ {member, admin, service}` | bootstrap admin 은 config/env. 이후 DB 가 권위 |
| **D3** | 3-level scope: `private` / `team:<id>` / `org` | `plugin` 필드는 orthogonal lifecycle 마커 |
| **D4** | Read = scope, Write = scope-member + admin | `locked = true` 예외로 owner+admin only 승격 |
| **D6** | SQL pre-filter (scope/team/admin) + pgvector HNSW | post-filter 는 accessible 결과 손실로 반려 |
| **D5** | Strict Penny inheritance | env 6 필드 + `AuthenticatedContext` + `ADMIN_ONLY_TOOLS` const |

### 5 개 핵심 원칙

1. **Identity-first** — 모든 행위는 `user_id` 로 기록되어야 함. API key, Penny subprocess, 자동화 스크립트 모두 user 로 해소.
2. **Tenancy 미래 호환** — P2B 는 single-tenant 지만 스키마는 multi-tenant 를 이미 가정. `tenant_id` 필드 어디에나 박되 값은 `"default"` 고정.
3. **Penny 는 권한을 올릴 수 없다** — caller 의 권한만 상속. `ADMIN_ONLY_TOOLS` 는 컴파일 시 차단, env 누락은 런타임 거부. prompt injection 이 발생해도 blast radius 는 caller 권한 내.
4. **Wiki 는 동시에 공개·팀·개인** — 한 인스턴스 내 3-level scope 공존. frontmatter 가 권위, 경로는 UX 편의.
5. **검색 결과에 "제한됨" 표시하지 않는다** — ACL pre-filter 는 존재를 숨김. "1개 결과 제한됨" 같은 메타는 info leakage 이므로 금지.

## Alternatives considered

| 대안 | 평가 | 기각 사유 |
|---|---|---|
| **A. Multi-tenant 완전 구현을 P2B 에 포함** | 격리/생성/billing/SSO 까지 | 범위 폭증. 내부 팀 초기 배포에 multi-tenant 는 YAGNI. P2C 로 연기하되 스키마 미래 호환 유지 |
| **B. User 개념 도입 없이 API key-only 다수화** | 현재 xaas 의 slim 연장 | web UI 로그인·Penny 대리·audit actor 의 identity 일원화 불가. "누가 이 wiki 를 썼나" 의 답이 key name 문자열 수준에서 멈춤 |
| **C. Unix-style per-page rwx ACL** | 최대 유연성 | UI 과중, 위키식 협업 친화성 저하, 검색 ACL 계산 비용 ↑. 필요 시 P3+ 로 승격 |
| **D. Penny 에 전용 "penny" service account 부여** | admin-lite 권한 | prompt injection 시 권한 상승 경로. 전면 반려 — D5 에서 strict inheritance 로 확정 |
| **E. GitOps 팀 정의 (TOML/wiki 기반)** | 변경 이력 자동 | 런타임 변경 빈도·무결성·UI 친화성 측면에서 DB 보다 열등. P3+ 에 "팀 manifest 를 git 에서 파생" 경로는 옵션으로 남음 |

## Trade-offs (explicit)

| 차원 | 이득 | 비용 |
|---|---|---|
| **인증 복잡도** | 일원화된 identity → 감사 선명, session 표준화 | web UI 로그인 경로·bootstrap admin 생성·user 관리 API/UI 구현 필요 (P2B 범위 증가) |
| **스키마 확장** | 미래 multi-tenant 마이그레이션 최소화 | 모든 테이블에 `tenant_id`, `owner_user_id` 필드. 기존 xaas 마이그레이션 4–5 건 |
| **Penny 보안** | prompt injection 시 권한 상승 불가 | 모든 MCP tool invoke 시그니처 변경 (`AuthenticatedContext`). Breaking change, 기존 plugin 코드 전부 touch |
| **검색 정확성** | accessible 결과 손실 없음 | pre-filter 쿼리 복잡도 ↑, team membership cache 필요 (moka LRU) |
| **Wiki 협업** | 오픈 편집으로 위키 전통 유지 | `locked = true` 의 오남용 가능 (과도한 잠금). UX 가이드 필요 |

## Cross-model verification

인터뷰 방식(B)로 사용자 직접 확답. D1 → D2 → D7 → D8 → D3 → D4 → D6 → D5 순차. 각 결정이 다음 결정의 전제를 이루는 흐름:

- D1 의 user 레이어 도입이 D7 (teams) / D8 (role) 의 전제
- D2 의 single-tenant 범위가 D6 의 검색 쿼리 복잡도를 줄임
- D3 의 3-level scope 가 D4 의 write rule 을 단순화
- D1–D4 의 user/team/scope/write 조합이 D5 의 strict Penny inheritance 를 자연스럽게 유도 (다른 선택지는 D1–D4 의 무결성을 깸)

세 detailed 설계 문서 간 결합부는:
- 08 → 09: user/team 레이어 위에 wiki scope 가 얹음
- 09 → 10: wiki scope 와 ACL 검사 함수가 Penny 의 `AuthenticatedContext` 가 소비하는 대상
- 10 → 08: Penny env 주입이 08 의 인증 세션 모델에 의존

## Mitigations

**M-MU1 — Admin bootstrap 오남용**
- `gadgetron.toml [auth.bootstrap]` 는 **`users` 테이블 empty** 일 때만 동작. empty 아니면 설정 **무시** + 기동 로그 warning
- bootstrap password 는 env 참조 (`password_env = "GADGETRON_BOOTSTRAP_ADMIN_PASSWORD"`) 만 허용, 평문 금지
- 첫 로그인 후 admin 이 bootstrap 설정 제거 권고 (08 doc §bootstrap 가이드)

**M-MU2 — Env 위조로 Penny 권한 우회**
- Penny subprocess 는 gadgetron-penny 가 spawn → env 주입이 parent process 에서 일어남
- subprocess 내부에서 env 변조는 본인 프로세스에만 영향. MCP tool 은 parent (gadgetron-gateway) 의 `AuthenticatedContext` 와 교차 검증 — subprocess 가 env 를 수정해도 tool 결과에 반영되지 않음
- 상세: 10-penny-permission-inheritance.md §6

**M-MU3 — prompt injection → privilege escalation**
- Penny 에 admin 권한 계정이 없음 (D5). caller 가 admin 이면 그 상한까지만
- `ADMIN_ONLY_TOOLS` const 가 컴파일 시 plugin 이 실수로 admin tool 을 등록하는 것을 차단
- output quarantine (07 doc §6.5) 과 합쳐 3 중 방어

**M-MU4 — Team 변경 시 캐시 stale**
- TeamCache (moka LRU, 1분 TTL) 가 stale 일 수 있음
- `POST /api/v1/teams/{id}/members` 시 cache invalidate pub/sub (in-process broadcast)
- stale window = 최대 1분, 그 시간 동안 과거 멤버가 팀 페이지 읽기 가능 — 수용 가능 (감사 로그에는 정확히 기록)
- 보안 민감한 경우 admin 이 user 강제 logout 가능 (P3 기능)

**M-MU5 — Wiki scope 누락 페이지**
- frontmatter `scope` 필드가 없는 기존 페이지 → 기본 `"private"` 승격
- 마이그레이션 스크립트: 모든 기존 wiki 페이지에 `scope = "org"` (기존 운영자 단일 user 였으므로 사실상 공유 지식) + `owner_user_id = <bootstrap-admin>`
- 09 doc §5 에 마이그레이션 절차 명시

## Consequences

### Immediate

- `docs/process/04-decision-log.md` D-20260418-02 등록 (완료)
- `docs/design/phase2/08-identity-and-users.md` 작성 (본 ADR 이 block)
- `docs/design/phase2/09-knowledge-acl.md` 작성
- `docs/design/phase2/10-penny-permission-inheritance.md` 작성
- `docs/adr/README.md` §목록에 ADR-P2A-08 추가

### P2B 구현 (본 ADR 이 block)

- `gadgetron-xaas` 스키마 마이그레이션 4–5 건 (08 doc §schema 참조)
- `gadgetron-core` 의 `AuthenticatedContext` + `ADMIN_ONLY_TOOLS` 신설
- 모든 MCP tool 의 `invoke` 시그니처 변경 (breaking change 이지만 compile error 가 방어막)
- `gadgetron-penny` env 주입 6 필드
- `gadgetron-knowledge` 검색 함수 시그니처 확장 (`user_id`, `tenant_id` 파라미터 필수화)
- `gadgetron-gateway` web UI 로그인 middleware
- `gadgetron-cli` user/team CLI 서브커맨드
- `gadgetron-web` 로그인 UI + admin 콘솔 + 페이지 scope 컨트롤

### Deferred to P2C

- Multi-tenant enforcement (tenant 생성/삭제 UI, cross-tenant 격리 활성화)
- SSO (OIDC / SAML)
- LDAP user 디렉토리 연동
- Materialized per-user accessible_ids cache (D6 C 옵션)
- Row-level encryption
- Fine-grained RBAC (team-admin, billing-viewer 등 role 세분화)

### Deferred to P3

- Unix-style per-page rwx (C 옵션)
- Manifest-as-code 팀 정의 (GitOps)
- 외부 KMS 통합
- Row-level 암호화

## Verification

구현 PR merge 전 검증:

1. `cargo test -p gadgetron-xaas` — users/teams/team_members 마이그레이션 + CRUD
2. `cargo test -p gadgetron-knowledge` — scope pre-filter 쿼리, admin bypass, scope 마이그레이션
3. `cargo test -p gadgetron-penny` — env 주입, `AuthenticatedContext` 생성, `ADMIN_ONLY_TOOLS` panic
4. Integration test: T1–T5 위협 시나리오 (10 doc §STRIDE) 회귀 방지 케이스
5. Integration test: bootstrap admin 생성 → member 승격 → member → admin 권한 부여 시퀀스
6. Integration test: user A 가 만든 private 페이지가 user B 의 `wiki.search` 결과에 절대 나타나지 않음
7. Integration test: Penny 가 admin caller 상속 시에만 `server.exec` cluster-wide 실행 가능
8. `docs/manual/authentication.md` (신설) / `docs/manual/admin-operations.md` (신설) 존재 + 운영자 워크스루

## Sources

- `docs/process/04-decision-log.md` D-20260418-02 — 8 sub-decision 상세
- `docs/design/phase2/08-identity-and-users.md` — D1/D2/D7/D8 구현
- `docs/design/phase2/09-knowledge-acl.md` — D3/D4/D6 구현
- `docs/design/phase2/10-penny-permission-inheritance.md` — D5 구현 + STRIDE
- `docs/adr/ADR-P2A-05-agent-centric-control-plane.md` — agent-centric 원칙 (D5 의 상속 모델이 이 원칙 확장)
- `docs/adr/ADR-P2A-06-approval-flow-deferred-to-p2b.md` — approval flow 가 D4/D5 의 destructive write 방어와 결합
- `docs/adr/ADR-P2A-07-semantic-wiki-pgvector.md` — pgvector 검색 위에 D6 pre-filter 얹음
- 2026-04-18 세션: interview mode B, D1–D8 순차 확답
