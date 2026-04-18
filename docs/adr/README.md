# Architecture Decision Records

Gadgetron 의 확정된 설계 결정을 시간순으로 기록한다. 각 ADR 은 단일 결정을 다루며, `Status` 가 ACCEPTED 인 경우 구현 단계에서 그대로 따른다. supersedes / amends 관계는 각 ADR 상단 헤더 표에 명시된다.

## 목록

| ADR | Date | Status | 제목 | 요약 |
|---|---|---|---|---|
| [P2A-01](ADR-P2A-01-allowed-tools-enforcement.md) | 2026-04-13 | ACCEPTED | `--allowed-tools` enforcement verification | Claude Code 2.1.104 `-p` 의 `--allowed-tools` 실제 차단 동작을 행동 테스트로 검증 (Part 1 PASS) + `feed_stdin` Option B 최종 확정 (Part 2) |
| [P2A-02](ADR-P2A-02-dangerously-skip-permissions-risk-acceptance.md) | 2026-04-13 | ACCEPTED (P2A only) | `--dangerously-skip-permissions` 단일유저 리스크 수락 | P2A 단일유저 스코프에서만 허용. `[P2C-SECURITY-REOPEN]` 태그로 P2C 재검토 고정 |
| [P2A-03](ADR-P2A-03-searxng-privacy-disclosure.md) | 2026-04-13 | ACCEPTED | SearXNG 쿼리 프라이버시 디스클로저 | 외부 인스턴스 사용 시 `docs/manual/penny.md` 에 고정 문구 삽입 (머지 전 게이트) |
| [P2A-04](ADR-P2A-04-chat-ui-selection.md) | 2026-04-14 | ACCEPTED | Web Chat UI: assistant-ui (drop OpenWebUI) | OpenWebUI 2025-04 라이선스 변경 + 단일 바이너리 embed 제약으로 drop, assistant-ui 채택 |
| [P2A-05](ADR-P2A-05-agent-centric-control-plane.md) | 2026-04-14 | ACCEPTED (amended by 06) | Agent-Centric Control Plane + MCP Tool Registry | Penny 를 `LlmProvider` 로 등록, MCP 툴 registry 가 지식·인프라·스케줄러·클러스터 tool surface 를 묶는 scaffold |
| [P2A-06](ADR-P2A-06-approval-flow-deferred-to-p2b.md) | 2026-04-14 | ACCEPTED | 대화형 승인 흐름 P2B 연기 | `04 v1` 리뷰 4회차 24 blockers 발생 → P2A 는 scaffold 만 구현, UI 승인 카드 + cross-process approval 브리지는 P2B 이관 (ADR-P2A-05 §(d)(e) amend) |
| [P2A-07](ADR-P2A-07-semantic-wiki-pgvector.md) | 2026-04-16 | ACCEPTED | Semantic Wiki: pgvector + embedding provider 추상화 | 키워드 전용 역인덱스를 하이브리드 (pgvector + ts_rank) 로 승격. 임베딩 provider 는 추상화되어 cloud/local 교체 가능 |
| [P2A-08](ADR-P2A-08-multi-user-foundation.md) | 2026-04-18 | ACCEPTED | Multi-user + Knowledge ACL Foundation (P2B) | `Tenant 1:N User 1:N ApiKey` 계층, 3-level wiki scope (private/team/org), admin role, Penny strict inheritance. 8 sub-decision (D1–D8) 을 `docs/process/04-decision-log.md` D-20260418-02 에 확정 |
| [P2A-09](ADR-P2A-09-raw-ingestion-pipeline.md) | 2026-04-18 | ACCEPTED | RAW Ingestion Pipeline + RAG Foundation | Core 에 `BlobStore`/`Extractor` trait + `IngestPipeline` ; Plugin 이 format extractor 제공 (`plugin-document-formats`, `plugin-web-scrape`); A+ 저장 (wiki markdown + blob 보존); Hybrid 청킹 (heading + fixed-size). 7 sub-decision (I1–I7) 을 D-20260418-03 에 확정. ADR-P2A-07 §Context 의 청킹 TODO supersede |
| [P2A-10](ADR-P2A-10-bundle-plug-gadget-terminology.md) | 2026-04-18 | ACCEPTED (amended by ADDENDUM-01) | Bundle / Plug / Gadget 용어 확정 | `BackendPlugin`→`Bundle`, `McpToolProvider`→`GadgetProvider`, `McpToolRegistry`→`GadgetRegistry`. Plug 는 core-facing (Rust trait impl), Gadget 은 Penny-facing (MCP tool). `Driver` → `Plug` amendment (D-20260418-05). Blocks P2B bundle work, `gadgetron install` CLI, `bundle.toml` schema |
| [P2A-10-ADDENDUM-01](ADR-P2A-10-ADDENDUM-01-rbac-granularity.md) | 2026-04-18 | ACCEPTED (rev5) | Bundle/Plug/Gadget RBAC 세분화 | 3-axis RBAC, per-Plug enablement, `requires_plugs` cascade, external Gadget runtime 보안, `GadgetronBundlesHome` resolver. W3 knowledge-layer priority reframing 포함. `BundleRegistry` is metadata-only (live values dropped after `install`). 6 agent synthesis (security/chief-architect/xaas/devops/dx/qa) + codex-chief-advisor validation |

## ADR 프로세스

- 새 ADR 은 `ADR-P<phase>-NN-<slug>.md` 패턴으로 이 디렉터리에 추가
- Status 단계: **Draft** → **Proposed** → **ACCEPTED** → (선택) **Superseded** / **Amended by X**
- supersedes / amends 관계는 각 ADR 상단 헤더 표에 반드시 명시
- 전체 PM 결정 이력(ADR 로 승격되기 전 단기 결정 포함) 은 [`../process/04-decision-log.md`](../process/04-decision-log.md) 에 있다

## 이 인덱스 업데이트 규칙

새 ADR 이 추가·상태 변경되면 이 파일 §목록 표를 갱신한다. `README.md`·`docs/INDEX.md` 는 **이 인덱스로 링크만** 하므로 상위 문서를 건드릴 필요가 없다.
