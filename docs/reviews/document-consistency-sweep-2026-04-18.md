# Document Consistency Sweep — 2026-04-18

> **목적**: Gadgetron 문서 정합성 회복을 위한 active tracker
> **상태**: Open
> **권위 규칙**: `docs/process/07-document-authority-and-reconciliation.md`

---

## 1. 이번 sweep 의 기준

이 tracker 는 다음 질문에 대해 문서들이 같은 답을 주는지 점검한다.

- 제품 용어가 무엇인가
- 어떤 영역이 core 인가 / Bundle 인가
- 현재 구현과 목표 구조를 문서가 어떻게 구분하는가
- 사용/운영/개발 문서가 실제 runnable path 를 설명하는가

---

## 2. Conflict clusters

| ID | 축 | 충돌 내용 | 대표 파일 | 우선순위 | 상태 |
|---|---|---|---|---|---|
| C-1 | 용어 | `plugin` vs `Bundle / Plug / Gadget` | `README.md`, `docs/00-overview.md`, `docs/design/phase2/06-backend-plugin-architecture.md`, `docs/process/04-decision-log.md` | P0 | **Closed** — README/00-overview: 정렬 완료. 06: legacy draft 선언. decision-log: `plugin.enable`→`bundle.enable` + "plugin이"→"Bundle이" 수정(D-20260418-02). 나머지 `plugin` 참조는 D-20260418-01 역사 기록 (D-20260418-04 가 용어 부분 supersede 명시) |
| C-2 | 경계 | `ADR-P2A-10` 의 Plug consumer wording / crate-path note 가 `router` 를 core-owned 로 오독하게 만들 수 있었음 | `docs/adr/ADR-P2A-10-bundle-plug-gadget-terminology.md`, `README.md`, `docs/00-overview.md`, `docs/architecture/glossary.md` | P0 | Closed |
| C-3 | 실행 경로 | `README` / manual 의 no-db, plain Postgres, `demo.sh`, pgvector 전제가 서로 다름 | `README.md`, `docs/manual/quickstart.md`, `docs/manual/web.md`, `docs/manual/installation.md` | P0 | Closed |
| C-4 | legacy 설계 문서 | `06-backend-plugin-architecture.md`, `07-plugin-server.md` 가 legacy 용어를 강하게 드러내지 못함 | `docs/design/phase2/06-backend-plugin-architecture.md`, `docs/design/phase2/07-plugin-server.md` | P1 | Closed |
| C-5 | seed/frontmatter | `plugin`, `plugin_version`, `plugin_seed` 호환 필드와 canonical 용어 사이 설명 부족 | `docs/architecture/glossary.md`, `crates/gadgetron-knowledge/src/wiki/frontmatter.rs`, `crates/gadgetron-knowledge/seeds/*` | P1 | Partial — `glossary.md` §Seed page 에 "Frontmatter field migration (P2B)" 표 추가. `frontmatter.rs` deprecated 주석 + serde alias 구현은 P2B 코드 작업으로 남음 |
| C-6 | deep architecture docs | `platform-architecture.md`, older module docs, review docs 의 legacy naming 잔존 | `docs/architecture/platform-architecture.md`, `docs/modules/*`, `docs/reviews/*` | P2 | Open |
| C-7 | authority entrypoint | `README.md` 가 hard-coded ADR count/range 를 유지해 최신 accepted ADR set 을 축소해서 보임 | `README.md`, `docs/adr/README.md` | P0 | Closed |

---

## 3. Canonical answers to enforce

### 3.1 용어

- Canonical vocabulary: **Bundle / Plug / Gadget**
- `plugin` 은 legacy, 인용, 외부 생태계 고유 명칭, 호환성 필드 설명을 제외하고 금지

### 3.2 경계

- `gateway` 는 core
- `router` 의 **canonical ownership** 은 `ai-infra` Bundle
- `scheduler`, `provider`, `node` 도 같은 migration cluster 로 관리
- 현재 디렉토리 레이아웃은 부연일 뿐, 구현자가 따라야 할 기준은 canonical ownership 이다

### 3.3 실행 경로

- 로컬 제공의 canonical operator loop 는 `demo.sh`
- `pgvector` 없는 plain PostgreSQL 은 기본 knowledge-backed runtime 에서 불충분
- embedded Web UI 는 release binary rebuild 없이는 소스 변경이 반영되지 않음

---

## 4. Sweep order

1. `README.md`
2. `docs/00-overview.md`
3. `docs/architecture/glossary.md`
4. `docs/manual/quickstart.md`
5. `docs/manual/web.md`
6. `docs/manual/installation.md`
7. `docs/design/phase2/06-backend-plugin-architecture.md`
8. `docs/design/phase2/07-plugin-server.md`
9. `docs/architecture/platform-architecture.md` and remaining deep docs

---

## 5. Open questions

| ID | 질문 | 현재 추천 | 상태 |
|---|---|---|---|
| Q-1 | `07-plugin-server.md` 는 즉시 `07-bundle-server.md` 로 rename 할지 | 내용 sweep 후 rename | **Closed** — `07-bundle-server.md` 로 rename 완료. `04-mcp-tool-registry.md`, `06-backend-plugin-architecture.md` rename 은 의도적 연기 (cross-reference 20+ 건 보호). ADR-P2A-10 §Consequences #4 에 implementation note 추가로 기록 완료 |
| Q-2 | seed/frontmatter 호환 필드 rename 은 문서 우선인지 코드 migration 우선인지 | 문서에 deprecated note 선명화 후 코드 migration | Open |

---

## 5.1 Sweep notes

- 2026-04-18 reconciliation pass:
  - `README.md`, `docs/00-overview.md` 의 남은 `McpToolRegistry` / `KnowledgeToolProvider` drift 제거
  - `docs/manual/quickstart.md`, `installation.md`, `web.md` 를 `demo.sh` + pgvector 기준의 canonical local operator loop 로 재작성/정렬
  - `docs/design/phase2/07-plugin-server.md` 에 canonical terminology / authority note 추가
- 2026-04-18 reconciliation pass (continued):
  - `docs/manual/configuration.md`, `docs/manual/penny.md`, `docs/manual/README.md` 를 trunk reality 에 맞게 정정
  - `docs/design/phase2/00-overview.md`, `01-knowledge-layer.md`, `02-penny-agent.md`, `04-mcp-tool-registry.md` 의 current-name mapping 과 visible type names 정렬
  - `docs/design/ops/*`, `ADR-P2A-05`, `ADR-P2A-06`, `09-knowledge-acl.md`, `10-penny-permission-inheritance.md` 에 canonical terminology note 추가
  - 결과: entrypoint/operator/active-design 문서의 top-level 해석은 정렬되었고, 잔여 드리프트는 deep body code blocks / historical references / compatibility-field docs 로 축소
- 2026-04-18 reconciliation pass (ADR index entrypoint):
  - `README.md` 의 stale ADR count/range 문구를 제거하고 `docs/adr/README.md` 를 유일한 maintained index 로 재고정
  - tracker 에 C-7 을 추가하고 같은 패스에서 Closed 처리
- 2026-04-18 reconciliation pass (C-1 close — decision-log D-20260418-02 terminology):
  - `docs/process/04-decision-log.md` D-20260418-02 §D5-a: `plugin.enable` → `bundle.enable`, "plugin 이" → "Bundle 이" (active ACL design section, not historical)
  - 잔여 `plugin` 참조는 전부 D-20260418-01 역사 기록이며 D-20260418-04가 용어 부분 supersede를 명시하므로 변경 불필요
  - C-1 Closed
- 2026-04-18 reconciliation pass (ownership authority note):
  - `ADR-P2A-10` 이 Plug 의 consumer boundary 와 crate ownership 을 분리해서 설명하도록 정정
  - `ADR-P2A-10` 의 current-path note 를 `router/provider/scheduler/node` canonical ownership (`ai-infra`, `server`, `gpu`) 과 충돌하지 않게 정렬
  - 결과: authority-layer boundary answer 는 `README.md` / `docs/00-overview.md` / glossary / decision log 와 일치하며 C-2 를 Closed 처리

---

## 6. Exit condition

아래가 모두 만족되면 이 tracker 를 Closed 로 바꾼다.

- C-1 ~ C-5 가 Closed
- C-6 는 최소 “legacy / historical / deep reference” 로 독자 오해를 만들지 않는 상태
- `README.md`, `docs/00-overview.md`, `docs/architecture/glossary.md`, manual entrypoints 가 canonical answers 와 충돌하지 않음
