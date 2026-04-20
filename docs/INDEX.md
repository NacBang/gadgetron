# Gadgetron 문서 인덱스

> 어떤 문서부터 읽어야 할지 막힐 때 여기서 출발하세요.
> 본 인덱스는 **길잡이**일 뿐이며 내용의 출처(SSOT)는 각 문서 본문에 있습니다.

---

## 1. 독자별 진입점

| 상황 | 먼저 읽을 문서 | 후속 참조 |
|---|---|---|
| "Gadgetron이 뭐 하는 제품인지 5분에" | [`00-overview.md`](00-overview.md) §1–2 | [`README.md`](../README.md) |
| 단일 바이너리 실행·설정 | [`manual/quickstart.md`](manual/quickstart.md) | [`manual/configuration.md`](manual/configuration.md), [`manual/troubleshooting.md`](manual/troubleshooting.md) |
| OpenAI API 사용자 / SDK 호출 | [`manual/api-reference.md`](manual/api-reference.md) | [`manual/auth.md`](manual/auth.md) |
| Penny 쓰기 | [`manual/penny.md`](manual/penny.md) | [`design/phase2/02-penny-agent.md`](design/phase2/02-penny-agent.md) |
| 크레이트/모듈 구현 작업 | [`modules/`](modules/) 해당 모듈 + [`architecture/platform-architecture.md`](architecture/platform-architecture.md) Axis B | [`design/phase2/`](design/phase2/) active work |
| 배포·운영·장애 대응 | [`architecture/platform-architecture.md`](architecture/platform-architecture.md) Axis C, F | [`modules/deployment-operations.md`](modules/deployment-operations.md) |
| 성능·레이턴시 분석 | [`architecture/platform-architecture.md`](architecture/platform-architecture.md) Axis H | |
| 현재 EPIC / ISSUE / TASK 계획 | [`ROADMAP.md`](ROADMAP.md) (canonical, PR #186 이후) | [`design/phase2/00-overview.md`](design/phase2/00-overview.md) (설계 세부), [`design/phase2/01..04`](design/phase2/) |
| 버전·릴리스 태그 정책 | [`ROADMAP.md`](ROADMAP.md) §Release tagging + [`process/06-versioning-policy.md`](process/06-versioning-policy.md) | `Cargo.toml` `[workspace.package] version` (SSOT) |
| 확정된 설계 결정 확인 | [`adr/README.md`](adr/README.md) (ADR 인덱스) | [`reviews/pm-decisions.md`](reviews/pm-decisions.md), [`process/04-decision-log.md`](process/04-decision-log.md) |
| 에이전트 역할 조회 | [`agents/`](agents/) | [`process/00-agent-roster.md`](process/00-agent-roster.md), [`AGENTS.md`](../AGENTS.md) |
| 설계 문서 작성·리뷰 규정 | [`process/01-workflow.md`](process/01-workflow.md) | [`process/02-document-template.md`](process/02-document-template.md), [`process/03-review-rubric.md`](process/03-review-rubric.md) |

---

## 2. 문서 3층 구조 (아키텍처 계통)

Gadgetron 아키텍처 지식은 **세 개의 레이어**로 쌓여 있고, 각 층은 서로 참조·보완합니다.

```
00-overview.md         (orientation — 1 page)
  ├── 제품 비전 / 3 Plane / 로드맵
  └── 크레이트 한 줄 요약 → 상세는 아래 두 층
          │
          ▼
architecture/platform-architecture.md    (integration view — 8 Axis SSOT)
  ├── Axis A: System Level (요청 흐름 / 엔트리포인트)
  ├── Axis B: Cross-Cutting (gateway/router/provider/scheduler/node/xaas)
  ├── Axis C: Deployment (Docker/K8s/Helm/Systemd)
  ├── Axis D: State Management
  ├── Axis E: Phase Evolution
  ├── Axis F: Failure Modes & Recovery
  ├── Axis G: Domain Model & Glossary
  └── Axis H: Performance Model
          │
          ▼
modules/*.md           (per-domain detailed spec — lead owner 별)
  ├── gateway-routing.md         — API Lead
  ├── model-serving.md           — Serving Lead
  ├── gpu-resource-manager.md    — Infra Lead
  ├── deployment-operations.md   — Ops Lead
  └── xaas-platform.md           — @xaas-platform-lead
```

**역할 경계**:
- `00-overview.md`는 **orientation**이므로 크레이트 API 시그니처를 반복하지 않고 링크만 제공
- `platform-architecture.md`는 **cross-cutting integration**이므로 "요청이 시스템을 어떻게 통과하는가"를 다룸
- `modules/*.md`는 **domain detail**이므로 한 도메인 안의 깊이 (인터페이스 전체, 에러 경로, 테스트 전략)를 다룸

같은 사실이 두 층에 나타나면 **modules/가 정답**, platform-architecture은 그 facts를 묶어 설명, 00-overview는 링크만 남깁니다.

---

## 3. 주제별 지도

| 영역 | 주 문서 | 보조 |
|---|---|---|
| **제품 비전 / 로드맵** | [`ROADMAP.md`](ROADMAP.md) (EPIC/ISSUE/TASK 트리, 릴리스 태그, 활성 EPIC) | [`00-overview.md`](00-overview.md) §1, §5 (역사적), [`README.md`](../README.md) (요약) |
| **크레이트 의존성 그래프** | [`00-overview.md`](00-overview.md) §2.1, §6 | [`architecture/platform-architecture.md`](architecture/platform-architecture.md) §2.A |
| **HTTP API 표면** | [`manual/api-reference.md`](manual/api-reference.md) | [`modules/gateway-routing.md`](modules/gateway-routing.md) |
| **라우팅 전략 (6종)** | [`modules/gateway-routing.md`](modules/gateway-routing.md) | [`architecture/platform-architecture.md`](architecture/platform-architecture.md) Axis B |
| **프로바이더 어댑터 (6종)** | [`modules/model-serving.md`](modules/model-serving.md) | [`design/provider/`](design/) (비어있음, archive 참조) |
| **VRAM 스케줄링 / Eviction** | [`modules/gpu-resource-manager.md`](modules/gpu-resource-manager.md) | [`modules/model-serving.md`](modules/model-serving.md) |
| **Multi-tenancy / quota / audit (chat-side)** | [`modules/xaas-platform.md`](modules/xaas-platform.md), [`design/xaas/phase1.md`](design/xaas/phase1.md) | [`architecture/platform-architecture.md`](architecture/platform-architecture.md) Axis B |
| **EPIC 4 XaaS — quota pipeline + billing telemetry + multi-user identity + auth plumbing + audit persistence/query + billing per-user attribution (end-to-end) + audit_log contamination fix + billing-insert SLO counter + Prometheus /metrics scrape surface** (ACTIVE, toward `v1.0.0`; ISSUEs 11 / 12 (telemetry) / 14 / 15 / 16 / 17 / 19 / 20 / 21 / 22 / 23 / 24 / 25 / 26 / 27 complete, 0.5.0 → 0.5.19) | [`manual/api-reference.md`](manual/api-reference.md) §`quota_exceeded` (PR #230) + §`GET /quota/status` (PR #234) + §Cookie-session endpoints (PR #248 / ISSUE 15) + §`GET /admin/audit/log` (PR #269 / ISSUE 22 + PR #293 / ISSUE 25 actor_user_id population) + §`GET /admin/billing/events` with `actor_user_id` column (PR #271 / ISSUE 23 + PR #289 / ISSUE 24 chat/action populate) + §`GET /admin/billing/insert-failures` (PR #299 / ISSUE 26) + §`GET /metrics` Prometheus text (PR #301 / ISSUE 27), [`manual/configuration.md`](manual/configuration.md) §`[quota_rate_limit]` (PR #231) + §`[auth.bootstrap]` (PR #246 / ISSUE 14), [`manual/auth.md`](manual/auth.md) §Cookie-session auth (ISSUE 15 + ISSUE 16 unified middleware + ISSUE 17 user_id + ISSUE 19 AuditEntry + ISSUE 20 plumbing + ISSUE 21 pg consumer + ISSUE 22 admin read), [`modules/xaas-platform.md`](modules/xaas-platform.md) §5.6 enforcement stack | [`ROADMAP.md`](ROADMAP.md) §EPIC 4 for the canonical per-ISSUE / per-TASK / per-PR / per-version breakdown (full chain of citations too long to duplicate here). Remaining gate items before `v1.0.0`: **ISSUE 18** (web UI login form, React/Tailwind in `gadgetron-web`). **ISSUE 28** (`/web` pre-auth landing polish — operator-report triage) is not a `v1.0.0` gate. Deferred post-1.0 (commercialization layer, 2026-04-20 scope direction): ISSUE 12.3 / 12.4 / 12.5 + ISSUE 13 (HF catalog). |
| **워크벤치 승인 흐름 / 직접 액션 감사 (0.2.6+)** | [`manual/api-reference.md`](manual/api-reference.md) §Approvals + §GET /audit/events, [`manual/web.md`](manual/web.md) §승인 흐름 | [`adr/ADR-P2A-06`](adr/ADR-P2A-06-approval-flow-deferred-to-p2b.md) (Penny-side 는 여전히 deferred) |
| **운영자 Observability / 대시보드 (0.2.7+)** | [`manual/api-reference.md`](manual/api-reference.md) §GET /usage/summary + §GET /events/ws + §GET /audit/tool-events (0.2.8+), [`manual/web.md`](manual/web.md) §`/web/dashboard` | [`manual/troubleshooting.md`](manual/troubleshooting.md) §`/events/ws` lag + 401 |
| **외부 MCP 클라이언트 surface — tool discovery + invoke + cross-session audit (0.2.10 → 0.2.12)** | [`manual/api-reference.md`](manual/api-reference.md) §GET /v1/tools + §POST /v1/tools/{name}/invoke + §GET /audit/tool-events (cross-session filter via `owner_id`) | `GadgetCatalog` trait (core), L3 allowed-names gate via `Arc<dyn GadgetDispatcher>`; [`architecture/platform-architecture.md`](architecture/platform-architecture.md) Axis B gadgetron-core row. EPIC 2 / ISSUE 7 shipped end-to-end (TASKs 7.1/7.2/7.3, PRs #204/#205/#207). |
| **Plugin platform — DescriptorCatalog hot-reload + bundle manifests + bundle marketplace** (EPIC 3 CLOSED `v0.5.0`, 2026-04-20, PR #228; 0.4.1 → 0.5.0; ISSUEs 8/9/10 all landed) | [`manual/api-reference.md`](manual/api-reference.md) §reload-catalog + §`GET /admin/bundles` + §`POST /admin/bundles` (install) + §`DELETE /admin/bundles/{bundle_id}` (uninstall), [`manual/configuration.md`](manual/configuration.md) §`[web]` (`catalog_path` / `bundles_dir` / `[bundle]` table with `required_scope`) + §`[web.bundle_signing]` (Ed25519 trust anchors) | `Arc<ArcSwap<CatalogSnapshot>>` substrate (catalog + pre-compiled JSON-schema validators). Canonical per-TASK breakdown: [`ROADMAP.md`](ROADMAP.md) §EPIC 3 (ISSUE 8 TASK 8.1–8.5 PRs #211/#213/#214/#216/#217; ISSUE 9 TASK 9.1–9.3 PRs #219/#220/#222; ISSUE 10 TASK 10.1–10.4 PRs #223/#224/#226/#227). Architecture cross-ref: [`architecture/platform-architecture.md`](architecture/platform-architecture.md) §2.E.2 + Axis B `gadgetron-gateway` row. Harness Gates 7q.1–7q.8 (8 gates covering reload + discovery + install + path-traversal + uninstall). Released as `v0.5.0`. fs-watcher deferred as optional TASK 8.6 if demand materializes. |
| **Docker / K8s / Helm 배포** | [`modules/deployment-operations.md`](modules/deployment-operations.md) | [`architecture/platform-architecture.md`](architecture/platform-architecture.md) Axis C |
| **장애 모드 / 복구 절차** | [`architecture/platform-architecture.md`](architecture/platform-architecture.md) Axis F | [`manual/troubleshooting.md`](manual/troubleshooting.md) |
| **Phase 2 Knowledge / Penny / Web / MCP** | [`design/phase2/`](design/phase2/) | [`manual/penny.md`](manual/penny.md), [`manual/web.md`](manual/web.md) |
| **보안 (M1–M8)** | [`design/phase2/00-overview.md`](design/phase2/00-overview.md) §8 | [`adr/ADR-P2A-01`](adr/ADR-P2A-01-allowed-tools-enforcement.md), [`adr/ADR-P2A-02`](adr/ADR-P2A-02-dangerously-skip-permissions-risk-acceptance.md) |

---

## 4. 문서 상태 범례

각 문서 헤더의 `상태:` 필드는 다음 값을 가집니다:

| 상태 | 의미 |
|---|---|
| **Draft** | 초안. 미해결 gap 있음. 구현 진입 전. |
| **Round N in progress** | N회차 크로스 리뷰 중. [`process/03-review-rubric.md`](process/03-review-rubric.md) 참조. |
| **Round N Approved** | N회차 승인 통과. 구현 가능. |
| **Trunk** | 실제 출시된 기능을 기술. `manual/*.md` 대부분. |
| **Archived** | 더 이상 유효하지 않음. [`archive/`](archive/) 하위. 역사 보존용. |

리뷰·승인 워크플로는 [`process/01-workflow.md`](process/01-workflow.md), 문서 템플릿은 [`process/02-document-template.md`](process/02-document-template.md)에 있습니다.

---

## 5. 아카이브 정책

[`archive/`](archive/) 하위 문서는 다음 중 하나입니다:

- 완료된 Phase 1 sprint / hotfix 설계 (구현 반영 후 보존)
- 상위 버전(v2+)으로 대체된 리뷰 snapshot (이력 보존)
- 더 이상 유효하지 않지만 ADR이 명시적으로 참조해 삭제 불가

새로운 문서는 archive에 추가하지 않습니다. 이동 시 원본 참조는 아카이브 경로로 업데이트하거나 "archived" 주석을 답니다.

---

## 6. 이 인덱스를 업데이트할 때

- 새 주제가 생기면 §3 표에 한 줄 추가
- 새 레이어(예: `docs/api/`)가 생기면 §2 3층 구조 재검토
- 역할이 중복되는 문서를 추가하지 말 것 — 기존 문서에 병합 또는 링크만 달기
- 문서를 archive로 옮길 때 §1·§3의 링크가 깨지지 않는지 확인
- ROADMAP.md 가 EPIC closure 로 크게 갱신되면 §1 "현재 EPIC/ISSUE/TASK 계획" 행의 EPIC 번호 + §3 "제품 비전 / 로드맵" 행을 함께 점검
