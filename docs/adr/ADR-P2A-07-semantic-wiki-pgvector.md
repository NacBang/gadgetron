# ADR-P2A-07 — Semantic Wiki: pgvector + Embedding Provider Abstraction

| Field | Value |
|---|---|
| **Status** | ACCEPTED (detailed design in `docs/design/phase2/05-knowledge-semantic.md`) |
| **Date** | 2026-04-16 |
| **Author** | PM (Claude) — user-directed decision via `/office-hours` session |
| **Parent docs** | `docs/design/phase2/01-knowledge-layer.md` v3; `docs/adr/ADR-P2A-05-agent-centric-control-plane.md` |
| **Blocks** | `docs/design/phase2/05-knowledge-semantic.md` implementation PRs |
| **Supersedes (partial)** | `docs/design/phase2/01-knowledge-layer.md §4` wiki search subsystem — keyword-only inverted index replaced by hybrid semantic + ts_rank |

---

## Context

Phase 2A의 지식 레이어(`gadgetron-knowledge`)는 마크다운 파일 + git autocommit + 키워드 전용 역인덱스 검색으로 설계되었다. 2026-04-16 office hours 세션에서 다음이 확인되었다:

1. **키워드 검색의 한계**: "서버가 안 켜져요"로 검색하면 "부팅 실패"라고 쓴 페이지를 못 찾는다. 저빈도 반복 환경(다양한 고객 환경, 다른 용어 사용)에서 키워드 매칭은 실용적으로 쓸 수 없다.
2. **프론트매터 누락**: `01-knowledge-layer.md §4`에 TOML 프론트매터(`WikiFrontmatter`) 스펙이 있으나 구현되지 않았다. 페이지에 구조화된 메타데이터 없이 타입/태그별 필터링이 불가능하다.
3. **인덱스 캐싱 없음**: `wiki.search` 호출 시마다 전체 파일을 읽어 역인덱스를 재구축한다. 500페이지에서 눈에 띄는 지연.
4. **수동 편집 비커버**: 현재 설계는 `wiki.write`를 통한 수정만 가정한다. 직접 파일 편집, `git pull` 등 외부 경로로 들어온 변경은 인덱싱되지 않는다.
5. **AI 자동 추출 파이프라인 부재**: 대화에서 지식을 자동 추출하는 메커니즘이 없다.

또한 근본적인 보정이 있었다: **지식 레이어는 GPU 인시던트 관리에 한정되지 않는다.** 인시던트 트러블슈팅은 use case 중 하나이지 시스템의 identity가 아니다. 개발 지식, 비즈니스 메모, 개인 노트 등 어떤 도메인에도 확장 가능한 범용 베이스여야 한다.

## Decision

**pgvector 기반 시맨틱 검색을 `gadgetron-knowledge`에 추가한다.** PostgreSQL + pgvector extension을 사용하고, 임베딩 생성은 `EmbeddingProvider` trait으로 추상화하여 OpenAI-compat API로 외부 API(OpenAI text-embedding-3-small)와 로컬 모델(Ollama 등)을 모두 지원한다.

### 핵심 결정 5가지

1. **pgvector over SQLite-vss**
   - PostgreSQL은 이미 `gadgetron-xaas`에서 사용 중(tenants, api_keys, audit_log, tool_audit_events)
   - pgvector HNSW 인덱스는 검증된 프로덕션 스택
   - P2B 멀티테넌트 전환 시 마이그레이션 불필요 (tenant_id 필터링 네이티브)
   - SQLite-vss는 실험적, 유지보수 불안정
   - 트레이드오프: P2A 단일유저도 Postgres 필수 의존성. 수용 가능 — "지식 없는 Gadgetron은 의미가 없음."

2. **파일시스템 마크다운 = source of truth, pgvector = derived index**
   - DB가 손실되어도 `gadgetron reindex --full`로 복구 가능
   - Git이 없으면 복구 불가 — git 히스토리가 최종 안전망
   - 저장 순서: 마크다운 디스크 쓰기 → git commit → 청킹 → 임베딩 → pgvector 저장

3. **하이브리드 검색 (시맨틱 + ts_rank 키워드, RRF fusion)**
   - 시맨틱: pgvector 코사인 유사도 (top 20)
   - 키워드: PostgreSQL `ts_rank` with `'simple'` dictionary (언어 비종속, 한국어/영어 혼용 지원) (top 20)
   - Reciprocal Rank Fusion으로 병합 (초기 가중치 시맨틱:키워드 = 0.6:0.4)
   - BM25는 PostgreSQL 기본 제공이 아니라 거부 (ParadeDB 별도 설치 필요, YAGNI)

4. **Write-path 임베딩 + reindex job**
   - `wiki.write` 내부에서 청킹 → 임베딩 → 저장 훅 (보안 7단계 파이프라인 + git commit 이후)
   - Codex chief advisor가 지적: wiki.write 훅만으로는 수동 편집/git pull 커버 불가
   - **`gadgetron reindex` CLI 명령 필수**: 파일시스템/git 상태 스캔으로 DB diff → 갱신
   - `gadgetron serve` 시작 시 incremental reindex 자동 실행 옵션 (`knowledge.reindex.on_startup = true`, default)
   - **청킹 알고리즘** — 이 ADR 작성 시점 미결. **ADR-P2A-09 I3 로 확정**: Hybrid chunking (heading 1차 + fixed-size 2차, target 500 / max 1500 / min 100 / overlap 50 tokens). 상세: `docs/design/phase2/11-raw-ingestion-and-rag.md §2.5`.

5. **AI 자동 추출 + 유저 승인**
   - Claude Code `-p` 모드는 "대화 종료" 개념이 없으므로, 에이전트가 매 턴 "이 대화에서 저장할 지식이 있는가?"를 판단
   - 시스템 프롬프트에 지식 추출 지침 포함 (에이전트/프롬프트 레이어, Rust 크레이트 아님)
   - 유저 승인 시 `wiki.write` 호출, 프론트매터에 `source = "conversation"`, `confidence = "medium"` 표시

### 범용 베이스 원칙

지식 레이어 core는 도메인 비종속. 인시던트 템플릿, 런북 포맷, 고객 환경 구조 등은 **사용 관례(convention)** 이지 코어 스키마가 아니다. 프론트매터 필드(`tags`, `type`, `source`, `confidence`)는 자유 필드로 유지하되 값에 대한 닫힌 enum 권장:
- `source`: `"user"` | `"conversation"` | `"reindex"`
- `confidence`: `"high"` | `"medium"` | `"low"`
- `type`: 자유 (`"incident"`, `"runbook"`, `"decision"`, `"note"`, 등)

파서는 미지 값에 warn, reject 하지 않음. 도메인별 템플릿/자동화는 P2B 이후.

## Alternatives considered

| 대안 | 평가 | 기각 사유 |
|---|---|---|
| **A. SQLite 사이드카 + brute-force 벡터** | CC 1-2일, Postgres 우회 | Postgres가 이미 스택에 있어 우회 이득 없음. SQLite → Postgres 마이그레이션 비용 존재. |
| **B. pgvector 정석 (SELECTED)** | CC 3-4일, 정식 하이브리드 검색 | 선택 |
| **C. 에이전트 네이티브 (벡터DB 없이)** | CC 반나절, wiki.list + wiki.get으로 직접 탐색 | 50페이지 이상에서 토큰 비용 폭발. 검색 품질이 에이전트 모델에 종속. |
| **D. GraphRAG (Microsoft)** | 엔티티 그래프 + RAG | P2A에 과함. 실시간 업데이트가 어렵고 구축 비용이 높음. P2C에서 재검토 가능. |
| **E. 기존 키워드 역인덱스 유지 + TF-IDF/BM25 추가** | 현재 코드 업그레이드 | 시맨틱 검색 부재로 근본 문제 해결 안 됨. |

## Trade-offs (explicit)

| 차원 | 이득 | 비용 |
|---|---|---|
| 의존성 | pgvector는 검증됨, Postgres는 이미 사용 | P2A 단일유저도 Postgres 필수 (Docker 한 줄이면 되지만 의존 추가) |
| 검색 품질 | 의미 매칭 + 키워드 fusion | 임베딩 provider 필요 (OpenAI API 비용 또는 로컬 GPU) |
| 복원 | git + reindex로 완전 복구 가능 | reindex 실행 시간 (500페이지 기준 ~분 단위 예상) |
| 스케일 | HNSW로 100만 벡터까지 <10ms | 청크 단위 저장으로 row 수 증가 |
| 개발 | 범용 베이스 (도메인 비종속) | 도메인 특화 기능은 별도 레이어 필요 |

## Cross-model verification

Codex chief advisor (2026-04-16):
- "에이전트 네이티브 지식 기판(substrate). markdown + TOML in git은 사람이 읽을 수 있는 source of truth. 메타데이터/임베딩/정제 제안은 파생 레이어."
- "이건 노트앱이 아니라 작업 지식의 캡처 시스템. 진짜 어려운 문제는 검색이 아니라 추출 판단, 출처 추적, 중복/업데이트 결정, 승인 UX."
- 전제 오류 지적: "wiki.write 안에서 임베딩하면 수동 편집도 커버된다"는 거짓 → reindex job 추가로 수정

Spec review (2라운드):
- Round 1: 16개 이슈 발견 (6/10)
- Round 2: 모든 이슈 해결, 4개 마이너 잔여 (91/100)
- 최종 수정 후 approve

## Mitigations

**M-KS1 — 임베딩 API 비용/의존성**
- OpenAI text-embedding-3-small 기준 $0.02 / 1M tokens. 개인용 dogfood 단계에서 월 $1 미만 예상.
- P2B에서 로컬 Ollama 전환 경로 확보 (같은 trait 구현체만 추가).

**M-KS2 — Dimension mismatch**
- DDL의 `vector(N)`은 `embedding.dimension` config와 일치해야 함.
- 모델 변경 시 `gadgetron reindex --full` + DDL ALTER 수동 실행.
- 런타임 검증: `embed()` 반환 벡터 길이 != config.dimension → `EmbeddingError::DimensionMismatch`, INSERT 차단.

**M-KS3 — Korean FTS**
- P2A: `to_tsvector('simple', ...)` 언어 비종속 토크나이저. 한국어도 동작하지만 stemming/형태소 없음.
- P2B: `pg_bigm` 또는 ICU 기반 한국어 사전 추가 고려.

**M-KS4 — 임베딩 provider 실패 시**
- Write는 성공 처리. 실패 청크는 reindex에서 catch-up.
- P2A에서는 별도 retry queue 없음. P2B에서 필요 시 추가.

## Consequences

### Immediate

- `docs/design/phase2/05-knowledge-semantic.md` 작성 (이 ADR이 blocking)
- `docs/design/phase2/01-knowledge-layer.md §4`에 05 문서 포인터 추가
- `gadgetron-knowledge`에 `wiki/frontmatter.rs`, `wiki/chunking.rs`, `embedding/` 모듈 추가
- `gadgetron-xaas` migrations에 `wiki_chunks`, `wiki_pages` 테이블 추가
- `gadgetron-cli`에 `gadgetron reindex` 서브커맨드 추가
- `gadgetron.toml`에 `[knowledge.embedding]`, `[knowledge.reindex]` 섹션 추가
- 시스템 프롬프트 지식 추출 지침 추가 (에이전트 레이어)

### Deferred to P2B

- AI 정리 제안 자동화 (병합, 모순 감지). P2A는 `wiki audit` 리포트 출력만.
- 로컬 임베딩 모델 전환 (Ollama 등)
- 한국어 FTS 사전 (pg_bigm / ICU)
- 백링크 인덱스 (위키링크 파서 연결)
- 청크 단위 snippet 개선

### Deferred to P2C

- 멀티테넌트 tenant_id 필터링 (스키마는 P2A에서 준비)
- GraphRAG 재검토
- 임베딩 retry queue

## Verification

구현 PR merge 전 검증:

1. `cargo test -p gadgetron-knowledge` 전체 통과
2. "서버가 안 켜져요" 쿼리로 "부팅 실패" 페이지가 top-3 안에 반환되는 E2E 테스트
3. 프론트매터 없는 기존 페이지에 대한 graceful handling 테스트
4. `gadgetron reindex --incremental` 후 수동 편집 파일이 검색에 반영되는 테스트
5. Dimension mismatch 시 `EmbeddingError::DimensionMismatch` 반환 테스트
6. `ts_rank` 'simple' dictionary로 한국어 쿼리 동작 확인 (부분적)
7. `docs/manual/knowledge.md` "Semantic search setup" 섹션 존재

## Sources

- `docs/design/phase2/05-knowledge-semantic.md` — 상세 설계 문서
- Office hours session: `~/.gstack/projects/NacBang-gadgetron/junghopark-main-design-20260416-113323.md`
- Codex chief advisor review (2026-04-16)
- [pgvector documentation](https://github.com/pgvector/pgvector)
- [OpenAI embeddings API](https://platform.openai.com/docs/guides/embeddings)
- [Microsoft GraphRAG](https://github.com/microsoft/graphrag) (P2C 재검토 대상)
- [Reciprocal Rank Fusion paper](https://plg.uwaterloo.ca/~gvcormac/cormacksigir09-rrf.pdf)
