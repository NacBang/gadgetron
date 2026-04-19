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
| **워크벤치 승인 흐름 / 직접 액션 감사 (0.2.6+)** | [`manual/api-reference.md`](manual/api-reference.md) §Approvals + §GET /audit/events, [`manual/web.md`](manual/web.md) §승인 흐름 | [`adr/ADR-P2A-06`](adr/ADR-P2A-06-approval-flow-deferred-to-p2b.md) (Penny-side 는 여전히 deferred) |
| **운영자 Observability / 대시보드 (0.2.7+)** | [`manual/api-reference.md`](manual/api-reference.md) §GET /usage/summary + §GET /events/ws + §GET /audit/tool-events (0.2.8+), [`manual/web.md`](manual/web.md) §`/web/dashboard` | [`manual/troubleshooting.md`](manual/troubleshooting.md) §`/events/ws` lag + 401 |
| **외부 MCP 클라이언트 surface — tool discovery + invoke + cross-session audit (0.2.10 → 0.2.12)** | [`manual/api-reference.md`](manual/api-reference.md) §GET /v1/tools + §POST /v1/tools/{name}/invoke + §GET /audit/tool-events (cross-session filter via `owner_id`) | `GadgetCatalog` trait (core), L3 allowed-names gate via `Arc<dyn GadgetDispatcher>`; [`architecture/platform-architecture.md`](architecture/platform-architecture.md) Axis B gadgetron-core row. EPIC 2 / ISSUE 7 shipped end-to-end (TASKs 7.1/7.2/7.3, PRs #204/#205/#207). |
| **Plugin platform — DescriptorCatalog hot-reload (EPIC 3 ACTIVE, 0.4.1 → 0.4.4)** | [`manual/api-reference.md`](manual/api-reference.md) §POST /api/v1/web/workbench/admin/reload-catalog (Management-scoped; wire shape `{reloaded, action_count, view_count, source, source_path}`), [`manual/configuration.md`](manual/configuration.md) §`[web]` 의 `catalog_path` 키 (TOML file source) | `Arc<ArcSwap<CatalogSnapshot>>` (catalog + pre-compiled JSON-schema validators 번들, ISSUE 8 TASK 8.1 `Arc<ArcSwap<DescriptorCatalog>>` 플러밍 → TASK 8.3 `CatalogSnapshot` 로 rev, PR #211 + #214) + TASK 8.4 file-based TOML source (`DescriptorCatalog::from_toml_file()` with parse-failure guard, PR #216), Management-scope 격리 규칙 (`scope_guard_middleware`); [`architecture/platform-architecture.md`](architecture/platform-architecture.md) §2.E.2 `/api/v1/web/workbench/admin/*` 안정성 행 + Axis B gadgetron-gateway 행; [`ROADMAP.md`](ROADMAP.md) §EPIC 3 (TASK 8.5 fs-watcher + SIGHUP 남음); harness Gate 7q.1 (swap + read-path cross-check) + 7q.2 (OpenAiCompat → 403). EPIC 3 close → tag `v0.5.0`. |
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
