# 05 — Semantic Knowledge Layer: pgvector + Embedding Provider

> **Status**: Draft v1 (2026-04-16) — office-hours 1 round + adversarial reviewer 2 rounds (16 issues found/fixed, 91/100 final)
> **Author**: PM (Claude) — user-directed via `/office-hours`
> **Parent**: `docs/design/phase2/00-overview.md` v3, `docs/design/phase2/01-knowledge-layer.md` v3
> **Drives**: ADR-P2A-07
> **Scope**: Semantic search extension for `gadgetron-knowledge` — pgvector + embedding provider + reindex job + AI auto-extraction. Supersedes `01-knowledge-layer.md §4` keyword-only search.
> **Implementation determinism**: every type signature, config field, SQL DDL, and CLI flag is explicit. No TBD.

## Table of Contents

1. Scope & Non-Scope
2. Architecture overview
3. Source of truth invariant
4. TOML frontmatter format
5. Chunking strategy
6. Embedding provider abstraction
7. Database schema (pgvector + FTS)
8. Write path (wiki.write hook)
9. Search path (hybrid semantic + keyword)
10. Reindex job
11. AI auto-extraction (agent prompt layer)
12. AI refinement suggestions (P2A minimal)
13. Configuration (`gadgetron.toml` additions)
14. CLI additions (`gadgetron reindex`, `gadgetron wiki audit`)
15. P2A defaults (pinned decisions)
16. Implementation order
17. Test plan
18. Open questions
19. Out of scope / deferred

---

## 1. Scope & Non-Scope

### In scope (P2A)

- `gadgetron-knowledge::wiki::frontmatter` — TOML frontmatter parser (new module)
- `gadgetron-knowledge::wiki::chunking` — heading-based chunking + token minmax (new module)
- `gadgetron-knowledge::embedding` — `EmbeddingProvider` trait + `OpenAiCompatEmbedding` impl (new module)
- `gadgetron-knowledge::wiki::store` — write hook: frontmatter parse + chunk + embed + store
- `gadgetron-knowledge::wiki::search` — hybrid search with RRF fusion (replaces pure keyword path)
- `gadgetron-xaas/migrations/` — `wiki_pages`, `wiki_chunks` tables + indexes
- `gadgetron-cli` — `gadgetron reindex` subcommand (incremental + full)
- `gadgetron-cli` — `gadgetron wiki audit` subcommand (stdout report only)
- `gadgetron.toml` — `[knowledge.embedding]`, `[knowledge.reindex]` sections
- Agent system prompt — knowledge extraction guidance (text only, not Rust)

### Out of scope — deferred to P2B

- AI 자동 병합 제안 (유사 페이지 병합 권고)
- 모순 감지 (다른 페이지가 충돌하는 주장)
- 로컬 임베딩 모델 (Ollama, bge-m3 등) — trait는 P2A, impl은 P2B
- 한국어 FTS 사전 (`pg_bigm` 또는 ICU)
- 백링크 인덱스 (위키링크 파서 연결)
- 청크 단위 snippet 개선
- 임베딩 retry queue
- 자동 approval loop (P2A는 에이전트가 제안 → 유저가 수락 → wiki.write 호출)

### Out of scope — deferred to P2C

- 멀티테넌트 tenant_id 필터링 (스키마는 P2A에서 `tenant_id TEXT NULL` 컬럼 준비)
- GraphRAG 재검토
- 다중 wiki registry (`wiki_registry: Vec<WikiEntry>` with `WikiScope::{Private, Team, Public, Plugin { owner }}`) — `Plugin` variant는 플러그인별 격리 네임스페이스를 원할 때 사용. P2A/P2B 기본은 공유 wiki (JARVIS 스타일 크로스도메인 추론 선호). 설계 문서: `docs/design/phase2/06-backend-plugin-architecture.md`.

### Explicit non-goals

- 이 문서는 도메인 특화 템플릿(인시던트 템플릿, 런북 포맷 등)을 정의하지 않는다. 그것들은 **convention**이지 코어 스키마가 아니다.
- 지식 레이어 core는 GPU 인시던트 관리에 한정되지 않는다. 범용 베이스다.

---

## 2. Architecture overview

```
┌──────────────────────────────────────────────────┐
│  에이전트 (Claude Code / Kairos)                  │
│  - 대화 중 "wiki-worthy?" 매 턴 판단              │
│  - create / append / update 제안                  │
│  - 검색 결과 해석 + 답변 생성                      │
│  (구현: 시스템 프롬프트, 이 문서 §11)              │
└──────────┬──────────────────────┬────────────────┘
           │ MCP tools             │
    ┌──────▼──────┐        ┌──────▼──────┐
    │ wiki.write  │        │ wiki.search │
    │ wiki.get    │        │ (hybrid)    │
    │ wiki.list   │        └──────┬──────┘
    └──────┬──────┘               │
           │                      │
    ┌──────▼──────────────────────▼──────┐
    │  gadgetron-knowledge (Rust)         │
    │                                     │
    │  Write path (§8):                   │
    │  1. 프론트매터 파싱/생성              │
    │  2. 보안 7단계 파이프라인             │
    │  3. 마크다운 → 디스크 → git commit   │
    │  4. 청킹 → 임베딩 → pgvector 저장    │
    │     (async, 실패해도 write 성공)     │
    │                                     │
    │  Search path (§9):                  │
    │  1. 쿼리 임베딩 생성                 │
    │  2. pgvector 코사인 유사도 (top 20)  │
    │  3. ts_rank BM-lite (top 20)        │
    │  4. RRF fusion                      │
    │  5. 상위 10개 + snippet 반환        │
    │                                     │
    │  Reindex (§10):                     │
    │  - `gadgetron reindex` CLI          │
    │  - 파일 스캔 → DB diff → 갱신        │
    │  - on_startup 옵션                   │
    └──────┬────────────────┬─────────────┘
           │                │
    ┌──────▼──────┐  ┌─────▼──────┐
    │ 파일시스템   │  │ PostgreSQL  │
    │ (git repo)  │  │ + pgvector  │
    │ = 원본      │  │ = 파생 인덱스│
    └─────────────┘  └────────────┘
```

---

## 3. Source of truth invariant

**파일시스템의 마크다운(in git) = source of truth.**
**pgvector = derived index, 재생성 가능.**

- DB가 완전히 손실되어도 `gadgetron reindex --full`로 모든 인덱스 복구 가능
- Git repository가 손상되면 복구 불가 — git이 최종 안전망
- 구현 결과: 수동 파일 편집, `git pull`, `git merge`, 외부 도구로 편집한 변경 사항도 reindex job으로 인덱싱 반영됨

---

## 4. TOML frontmatter format

### 스키마

```markdown
---
tags = ["H100", "ECC", "부팅실패"]
type = "incident"
created = 2026-04-16T10:30:00Z
updated = 2026-04-16T11:00:00Z
source = "conversation"
confidence = "high"
---

# 페이지 본문
...
```

### 필드 규약 (convention, enforcement 안 함)

| 필드 | 타입 | 권장 값 | 비고 |
|---|---|---|---|
| `tags` | `[String]` | 자유 | 태그 인덱스에 사용 |
| `type` | `String` | `"incident"` \| `"runbook"` \| `"decision"` \| `"note"` \| 자유 | 도메인 자유 |
| `created` | RFC 3339 datetime | 생성 시각 | 파서가 자동 설정 |
| `updated` | RFC 3339 datetime | 마지막 수정 시각 | 파서가 자동 설정 |
| `source` | `String` | `"user"` \| `"conversation"` \| `"reindex"` \| `"seed"` | 닫힌 enum 권장, 파서는 미지 값에 warn. `"seed"`는 core 또는 플러그인이 주입한 시작 문서 |
| `confidence` | `String` | `"high"` \| `"medium"` \| `"low"` | AI 추출은 "medium", 유저 직접은 "high" |
| `plugin` | `String` (optional) | 예: `"gadgetron-core"`, `"ai-infra"`, `"cicd"` | seed 페이지의 소유자. `source = "seed"`일 때 필수. `"gadgetron-core"`는 바이너리에 embed된 기본 시드. 플러그인은 자기 이름(kebab-case). uninstall/archive 시 `WHERE frontmatter->>'plugin' = '...'` 필터 사용 |
| `plugin_version` | `String` (optional) | 예: `"0.2.0"` | 해당 시드의 버전. upgrade 시 재주입 판단에 사용 |

### 파서 규약

- 파일 시작이 `---\n`이 아니면 프론트매터 없음 → `WikiFrontmatter::default()` 반환 (에러 아님)
- 파일 시작이 `---\n`이면 다음 `---\n`까지를 TOML로 파싱
- TOML 파싱 실패 → `WikiError::Frontmatter { reason }` 반환, 페이지 저장 거부
- 알 수 없는 필드는 `extra: HashMap<String, toml::Value>`로 보존
- `source`, `confidence` 값이 권장 enum 밖이면 `tracing::warn!`만 emit, 저장은 진행

### Rust 타입

```rust
// crates/gadgetron-knowledge/src/wiki/frontmatter.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WikiFrontmatter {
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub created: Option<DateTime<Utc>>,
    #[serde(default)]
    pub updated: Option<DateTime<Utc>>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub confidence: Option<String>,
    /// Plugin that seeded/owns this page (optional). When `source =
    /// "plugin_seed"`, this SHOULD be set so uninstall / archive flows can
    /// filter by plugin. Backend-plugin architecture: see
    /// `docs/design/phase2/06-backend-plugin-architecture.md`.
    #[serde(default)]
    pub plugin: Option<String>,
    /// Version of the plugin that seeded this page. Used by the plugin
    /// `initialize()` hook to decide whether to re-inject updated seeds.
    #[serde(default)]
    pub plugin_version: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, toml::Value>,
}

pub struct ParsedPage {
    pub frontmatter: WikiFrontmatter,
    pub body: String,
}

pub fn parse_page(raw: &str) -> Result<ParsedPage, WikiError> { /* ... */ }
pub fn serialize_page(fm: &WikiFrontmatter, body: &str) -> String { /* ... */ }
```

---

## 5. Chunking strategy

**목적**: 페이지를 의미 단위로 쪼개서 임베딩 품질과 검색 정확도를 높인다.

### 알고리즘

1. 프론트매터 제거
2. 마크다운 파서로 본문을 AST로 변환
3. `##` (h2) 헤딩 기준으로 섹션 분리. h1은 페이지 제목이므로 무시.
4. 각 섹션의 토큰 수를 측정 (tiktoken cl100k_base 근사 또는 단순 word split)
5. 섹션이 **max 512 토큰** 초과 시 문단(`\n\n`) 단위로 재분할
6. 청크가 **min 64 토큰** 미만이면 인접 청크(다음 우선, 없으면 이전)와 병합
7. 헤딩이 전혀 없는 페이지 → 전체 본문을 단일 청크로 처리 (토큰 수 무관)

### 청크 메타데이터

각 청크에 다음 정보를 부여:

```rust
pub struct Chunk {
    pub page_name: String,
    pub chunk_index: usize,      // 0부터 시작, 페이지 내 순서
    pub section: Option<String>, // h2 제목, 없으면 None
    pub content: String,
}
```

### Rust API

```rust
// crates/gadgetron-knowledge/src/wiki/chunking.rs
pub const CHUNK_MAX_TOKENS: usize = 512;
pub const CHUNK_MIN_TOKENS: usize = 64;

pub fn chunk_page(page_name: &str, body: &str) -> Vec<Chunk>;
pub fn count_tokens(text: &str) -> usize; // 근사 구현
```

### 청킹 결정 기록

- **왜 h2 기준?** h1은 페이지 제목, h3는 너무 세분화. h2가 일반적인 섹션 단위.
- **왜 max 512?** OpenAI text-embedding-3-small context 8191 tokens의 여유. 대부분 섹션 커버.
- **왜 min 64?** 너무 짧은 청크는 임베딩 품질 저하. 예: "TODO" 같은 한 줄 청크.

---

## 6. Embedding provider abstraction

### Trait

```rust
// crates/gadgetron-knowledge/src/embedding/mod.rs
use async_trait::async_trait;

#[async_trait]
pub trait EmbeddingProvider: Send + Sync + 'static {
    /// N개 텍스트를 임베딩. 반환: 각 텍스트에 대한 벡터.
    /// 모든 벡터는 self.dimension() 길이여야 함.
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError>;

    /// 이 provider가 생성하는 벡터의 차원.
    fn dimension(&self) -> usize;

    /// 모델 식별자 (로그/디버깅용). 예: "text-embedding-3-small".
    fn model_name(&self) -> &str;
}

#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    #[error("embedding provider HTTP error: {0}")]
    Http(String),
    #[error("embedding provider returned {got} dimensions, expected {expected}")]
    DimensionMismatch { got: usize, expected: usize },
    #[error("embedding provider response parse failed")]
    Parse,
    #[error("embedding provider timeout after {seconds}s")]
    Timeout { seconds: u64 },
    #[error("embedding provider auth failed")]
    Auth,
}
```

### P2A 구현체: OpenAI-compat

```rust
// crates/gadgetron-knowledge/src/embedding/openai_compat.rs
pub struct OpenAiCompatEmbedding {
    http: reqwest::Client,
    base_url: reqwest::Url,
    model: String,
    dimension: usize,
    api_key: SecretString,
}

impl OpenAiCompatEmbedding {
    pub fn new(config: &EmbeddingConfig, env: &dyn EnvResolver) -> Result<Self, EmbeddingError> { /* ... */ }
}

#[async_trait]
impl EmbeddingProvider for OpenAiCompatEmbedding {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        // POST {base_url}/embeddings
        // body: { "model": self.model, "input": texts }
        // headers: Authorization: Bearer {api_key}
        // Response: { "data": [ { "embedding": [...], "index": N }, ... ] }
        //
        // Runtime validation: len(embedding) == self.dimension
        // Mismatch → EmbeddingError::DimensionMismatch, INSERT 차단
        // ...
    }
    fn dimension(&self) -> usize { self.dimension }
    fn model_name(&self) -> &str { &self.model }
}
```

### 지원 제공자 (P2A)

- **OpenAI**: `text-embedding-3-small` (1536d, 기본), `text-embedding-3-large` (3072d)
- **로컬 Ollama**: `nomic-embed-text` (768d) — base_url만 바꾸면 동작. P2B에서 정식 지원.

---

## 7. Database schema (pgvector + FTS)

### Migration SQL

`crates/gadgetron-xaas/migrations/20260417000001_knowledge_semantic.sql`:

```sql
-- pgvector extension (Postgres 15+)
CREATE EXTENSION IF NOT EXISTS vector;

-- 페이지 메타데이터
CREATE TABLE wiki_pages (
    page_name    TEXT PRIMARY KEY,
    frontmatter  JSONB NOT NULL DEFAULT '{}',
    content_hash TEXT NOT NULL,  -- SHA-256 hex of raw file bytes
    tenant_id    TEXT NULL,      -- P2C multi-tenant prep (always NULL in P2A)
    indexed_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX wiki_pages_tags_idx ON wiki_pages USING GIN ((frontmatter->'tags'));
CREATE INDEX wiki_pages_tenant_id_idx ON wiki_pages (tenant_id) WHERE tenant_id IS NOT NULL;
-- type 인덱스는 P2B에서 type-filter 검색 추가 시 생성:
--   CREATE INDEX wiki_pages_type_idx ON wiki_pages ((frontmatter->>'type'));

-- 청크 + 임베딩
-- 주의: vector(N) 차원은 [knowledge.embedding].dimension config와 일치해야 함.
-- 모델 변경 시 `gadgetron reindex --full` + 이 DDL ALTER 수동 실행 필요.
CREATE TABLE wiki_chunks (
    id          BIGSERIAL PRIMARY KEY,
    page_name   TEXT NOT NULL,
    chunk_index INT NOT NULL,
    section     TEXT,
    content     TEXT NOT NULL,
    content_tsv TSVECTOR GENERATED ALWAYS AS (to_tsvector('simple', content)) STORED,
    embedding   vector(1536),    -- P2A default: text-embedding-3-small
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(page_name, chunk_index)
);

-- HNSW index for cosine similarity
CREATE INDEX wiki_chunks_embedding_idx
    ON wiki_chunks USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- Keyword search index
CREATE INDEX wiki_chunks_tsv_idx ON wiki_chunks USING GIN (content_tsv);

-- Page fetch index
CREATE INDEX wiki_chunks_page_name_idx ON wiki_chunks (page_name);
```

### 업서트 전략

페이지 재임베딩 시:
```sql
-- 1. 모든 기존 청크 삭제
DELETE FROM wiki_chunks WHERE page_name = $1;

-- 2. 새 청크 INSERT
INSERT INTO wiki_chunks (page_name, chunk_index, section, content, embedding)
VALUES ($1, $2, $3, $4, $5::vector);

-- 3. wiki_pages UPSERT
INSERT INTO wiki_pages (page_name, frontmatter, content_hash, indexed_at)
VALUES ($1, $2, $3, NOW())
ON CONFLICT (page_name) DO UPDATE SET
    frontmatter = EXCLUDED.frontmatter,
    content_hash = EXCLUDED.content_hash,
    indexed_at = NOW();
```

트랜잭션으로 감싸서 DELETE + INSERT 원자성 보장.

---

## 8. Write path (wiki.write hook)

### 실행 순서

1. (기존) 프론트매터 파싱 — 없으면 기본값 생성, `created`/`updated` 자동 설정
2. (기존) 보안 7단계 파이프라인: 크기 → BLOCK creds → AUDIT creds → 경로 → mkdir → O_NOFOLLOW → autocommit
3. (기존) 마크다운 디스크 쓰기
4. (기존) `git add` + `git commit` (autocommit 활성 시)
5. **(신규)** SHA-256 content_hash 계산
6. **(신규)** 청킹 (§5)
7. **(신규)** 각 청크를 임베딩 provider로 전송 (배치)
8. **(신규)** 트랜잭션: `wiki_chunks` DELETE + INSERT, `wiki_pages` UPSERT

### 동기/비동기 모드

```toml
[knowledge.embedding]
write_mode = "async"  # "async" (default) | "sync"
```

- `"async"` (default): `wiki.write`가 디스크 쓰기 + git commit 완료 즉시 반환. 5-8단계는 백그라운드 tokio task.
- `"sync"`: 5-8단계 완료까지 대기. 응답이 느려지지만 검색 결과 즉시 일관성.

### 실패 처리

- 임베딩 API 실패 → write는 성공 처리, `tracing::warn!` emit, 해당 페이지는 reindex에서 catch-up
- Dimension mismatch → write는 성공, 청크만 INSERT 스킵, `tracing::error!` emit
- Postgres 연결 실패 → write는 성공 (git에 저장됨), 다음 reindex에서 반영
- P2A에서는 별도 retry queue 없음. P2B에서 필요 시 추가.

---

## 9. Search path (hybrid)

### MCP 도구 스키마 변경

`wiki.search`에 `limit` 파라미터 추가 (기본 10, 최대 50):

```json
{
  "name": "wiki.search",
  "description": "Semantic + keyword search over the wiki. Returns pages ranked by relevance.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query": { "type": "string", "minLength": 1, "maxLength": 512 },
      "limit": { "type": "integer", "minimum": 1, "maximum": 50, "default": 10 }
    },
    "required": ["query"],
    "additionalProperties": false
  }
}
```

### 검색 흐름

1. 쿼리 텍스트 → `EmbeddingProvider::embed(&[query])`로 벡터 생성
2. **시맨틱 검색**:
   ```sql
   SELECT page_name, chunk_index, content, section,
          1 - (embedding <=> $1::vector) AS cosine_score
   FROM wiki_chunks
   ORDER BY embedding <=> $1::vector
   LIMIT 20;
   ```
3. **키워드 검색**:
   ```sql
   SELECT page_name, chunk_index, content, section,
          ts_rank(content_tsv, plainto_tsquery('simple', $1)) AS ts_score
   FROM wiki_chunks
   WHERE content_tsv @@ plainto_tsquery('simple', $1)
   ORDER BY ts_score DESC
   LIMIT 20;
   ```
4. **RRF fusion** (Rust):
   ```
   final_score(doc) = w_sem * 1/(k + rank_sem(doc))
                    + w_key * 1/(k + rank_key(doc))
   where k = 60, w_sem = 0.6, w_key = 0.4
   ```
5. 같은 `page_name`의 여러 청크가 매치되면 최고 점수만 유지 (page-level dedup)
6. 상위 `limit`개 페이지 반환 + snippet (매치 청크의 content 앞 200자)

### 반환 스키마

```json
{
  "hits": [
    {
      "page_name": "incidents/2026-04-15-server-A-boot-failure",
      "score": 0.87,
      "section": "진단 과정",
      "snippet": "전원은 들어오지만 POST에서 멈춤. BIOS 설정 확인..."
    }
  ]
}
```

### Keyword fallback

`plainto_tsquery('simple', ...)`가 0 결과 반환 시 RRF는 시맨틱 결과만 사용 (정상 동작).

---

## 10. Reindex job

### CLI

```
gadgetron reindex [OPTIONS]

Options:
  --full              모든 wiki_chunks + wiki_pages TRUNCATE 후 재구축
  --incremental       (default) 변경된 페이지만 재인덱싱
  --dry-run           스캔만 하고 실제 변경 없이 리포트
  --verbose           페이지별 진행 로그
```

### 알고리즘 (incremental)

1. 파일시스템 스캔: `wiki_path/**/*.md` 모든 파일 목록 수집
2. 각 파일에 대해 SHA-256 content_hash 계산
3. `wiki_pages` 테이블에서 `page_name`별 `content_hash` 조회
4. 판단:
   - 파일 있고 DB 없음 → INSERT
   - 파일 있고 DB 해시 다름 → 청크 DELETE + 재임베딩 + UPSERT
   - 파일 있고 DB 해시 같음 → skip
   - 파일 없고 DB 있음 → DELETE (청크 포함)
5. 각 "재임베딩 필요" 페이지에 대해 write path 5-8단계 실행

### on_startup

```toml
[knowledge.reindex]
on_startup = true          # (default) gadgetron serve 시작 시 자동 실행
on_startup_mode = "async"  # "async" (default) | "sync" | "incremental" | "full"
```

- `"async"` (default): serve 시작 후 백그라운드에서 incremental reindex 실행. API는 즉시 listening.
- `"sync"`: reindex 완료까지 serve 시작 대기. 테스트/CI용.

### --full 시 TRUNCATE 순서

```sql
BEGIN;
TRUNCATE wiki_chunks CASCADE;
TRUNCATE wiki_pages CASCADE;
COMMIT;
-- 이후 파일시스템 전체 스캔 + 인덱싱
```

### 관찰성

- `tracing::info!`로 진행 상황 emit: `"reindex: {scanned}/{total} pages, {reembedded} re-embedded, {deleted} deleted"`
- 완료 시 summary log + stdout 리포트

---

## 11. AI auto-extraction (agent prompt layer)

**구현 위치**: 에이전트 시스템 프롬프트. Rust 크레이트 코드 아님.

**경로**: `crates/gadgetron-kairos/src/session.rs`에서 subprocess 시작 시 system message에 포함되는 프롬프트 텍스트.

### 프롬프트 지침 (한국어)

```
## 지식 관리 역할

너는 사용자의 AI 파트너로서, 대화에서 유용한 지식을 위키에 저장하는 역할을 한다.

**매 응답 후 다음을 판단하라:**
1. 이 대화에서 나중에 재사용 가능한 지식이 생성됐는가?
   - 문제 해결 방법
   - 환경 설정 정보
   - 결정 사항과 그 이유
   - 발견한 사실
   - 도메인 지식
2. 저장할 가치가 있다면:
   a. `wiki.search`로 관련 기존 페이지 확인
   b. 기존 페이지에 append할지, 새 페이지를 만들지, update할지 판단
   c. 사용자에게 명확히 제안: "이 내용을 위키에 [action] 할까요? [요약]"
   d. 사용자 승인 시 `wiki.write` 호출. 프론트매터에 반드시 포함:
      - source = "conversation"
      - confidence = "medium"
      - tags = [적절한 태그들]
      - type = 내용에 맞는 분류

**저장하지 말 것:**
- 사용자의 사적 정보, 자격증명
- 한 번 쓰고 버릴 정보 (예: 특정 고객 ID, 일회성 질문)
- 사용자가 명시적으로 "저장하지 마"라고 한 내용

**저장 제안 형식 (예시):**
"방금 대화에서 H100 부팅 실패 해결 방법을 다뤘습니다. 
위키에 [new page: incidents/h100-boot-failure-bios-update] 저장할까요?"
```

### 개선 여지

- P2B에서 에이전트가 자동으로 제안 없이 `wiki.write` 직접 호출하는 `auto` mode 추가 (T2 tool policy 준수)
- P2B에서 추출 품질 평가 메트릭 (얼마나 재사용되는가)

---

## 12. AI refinement suggestions (P2A minimal)

### `gadgetron wiki audit` CLI

stdout에 다음 리포트 출력:

```
Wiki audit report — 2026-04-16 14:30:00
Wiki path: ~/.gadgetron/wiki
Total pages: 47

## Stale pages (updated more than 90 days ago)
- incidents/2026-01-15-cooling-issue-server-B
  updated: 2026-01-15 (91 days ago)
  suggestion: review for current relevance

- runbooks/h100-nvlink-check
  updated: 2026-01-20 (86 days ago)
  suggestion: review for current relevance

## Pages without frontmatter
- old-notes/misc-commands
  suggestion: add frontmatter (tags, type, created)
```

### P2A 범위

- 오래된 페이지 리스트 (config로 임계값 조정, 기본 90일)
- 프론트매터 없는 페이지 리스트
- **실행 없음** — 순수 리포트

### P2B 확장

- 시맨틱 유사도로 중복/병합 후보 감지
- 모순 감지 (같은 주제 다른 결론)
- 승인 기반 자동 병합/업데이트

---

## 13. Configuration

### 새 섹션: `[knowledge.embedding]`

```toml
[knowledge.embedding]
provider = "openai_compat"               # "openai_compat" (P2A), "local_ollama" (P2B)
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"           # env var name, not the key itself
model = "text-embedding-3-small"
dimension = 1536
write_mode = "async"                     # "async" | "sync"
timeout_secs = 30
```

### 새 섹션: `[knowledge.reindex]`

```toml
[knowledge.reindex]
on_startup = true                        # gadgetron serve 시작 시 자동 실행
on_startup_mode = "async"                # "async" | "sync" | "incremental" | "full"
stale_threshold_days = 90                # gadgetron wiki audit의 낡은 페이지 기준
```

### 유효성 검사 (V19-V22)

- **V19**: `embedding.dimension`이 [1, 8192] 범위
- **V20**: `embedding.base_url` 유효한 http/https URL
- **V21**: `embedding.api_key_env` 환경변수가 실제로 설정되어 있어야 함 (validate 시점)
- **V22**: `reindex.stale_threshold_days`가 [1, 3650] 범위

---

## 14. CLI additions

### `gadgetron reindex`

§10 참조. `crates/gadgetron-cli/src/main.rs::cmd_reindex` 추가.

### `gadgetron wiki audit`

§12 참조. `crates/gadgetron-cli/src/main.rs::cmd_wiki_audit` 추가.

### `gadgetron init` 템플릿 업데이트

기존 템플릿에 `[knowledge.embedding]`, `[knowledge.reindex]` 섹션 추가. 기본값은 주석 처리된 예시:

```toml
# [knowledge.embedding]
# provider = "openai_compat"
# base_url = "https://api.openai.com/v1"
# api_key_env = "OPENAI_API_KEY"
# model = "text-embedding-3-small"
# dimension = 1536
```

---

## 15. P2A defaults (pinned decisions)

- 임베딩 모델: **text-embedding-3-small** (1536d)
  - 비용: $0.02 / 1M tokens, 개인 dogfood에서 월 $1 미만 예상
  - 로컬 전환: P2B
- RRF 가중치: **시맨틱 0.6 : 키워드 0.4** (실측 후 조정)
- 청크 크기: **max 512 토큰, min 64 토큰**
- `write_mode`: **async** (default)
- `on_startup`: **true** (default)
- `on_startup_mode`: **async** (default)
- FTS dictionary: **'simple'** (언어 비종속)
- 한국어 사전: P2B에서 `pg_bigm` 또는 ICU

---

## 16. Implementation order

| 순서 | 작업 | 크레이트 | 테스트 |
|---|---|---|---|
| 1 | `wiki/frontmatter.rs` 파서 구현 | gadgetron-knowledge | 8 unit tests (§17.1) |
| 2 | `wiki/chunking.rs` 구현 | gadgetron-knowledge | 10 unit tests (§17.2) |
| 3 | `embedding/` trait + OpenAI-compat | gadgetron-knowledge | 6 unit tests + 1 integ test (§17.3) |
| 4 | `migrations/20260417000001_*.sql` | gadgetron-xaas | migration round-trip test |
| 5 | `wiki/store.rs` write path에 5-8단계 훅 추가 | gadgetron-knowledge | 5 integ tests (§17.4) |
| 6 | `wiki/search` 하이브리드로 교체 | gadgetron-knowledge | 8 integ tests (§17.5) |
| 7 | `gadgetron reindex` CLI 서브커맨드 | gadgetron-cli | 4 integ tests (§17.6) |
| 8 | `gadgetron wiki audit` CLI 서브커맨드 | gadgetron-cli | 3 integ tests |
| 9 | 시스템 프롬프트 지식 추출 지침 (에이전트/프롬프트 레이어, Rust 아님) | gadgetron-kairos | E2E 테스트 (수동) |
| 10 | `gadgetron init` 템플릿 + 매뉴얼 업데이트 | gadgetron-cli, docs | 문서 확인 |

각 단계는 독립 PR로 land 가능. TDD: Red → Green → Refactor.

---

## 17. Test plan

### 17.1 Frontmatter parser

1. `frontmatter_absent_returns_default` — `---\n` 없는 마크다운
2. `frontmatter_parses_full_fields` — tags, type, created, updated, source, confidence
3. `frontmatter_malformed_toml_returns_error`
4. `frontmatter_unknown_fields_preserved_in_extra`
5. `frontmatter_source_unknown_value_warns_not_errors`
6. `frontmatter_confidence_unknown_value_warns_not_errors`
7. `serialize_round_trip_preserves_fields`
8. `serialize_absent_frontmatter_emits_none`

### 17.2 Chunking

1. `chunk_single_h2_section_produces_one_chunk`
2. `chunk_multiple_h2_sections_splits_correctly`
3. `chunk_no_headings_single_chunk`
4. `chunk_oversize_section_splits_by_paragraph`
5. `chunk_undersize_merges_with_next`
6. `chunk_undersize_at_end_merges_with_prev`
7. `chunk_preserves_section_metadata`
8. `chunk_index_is_zero_based_sequential`
9. `chunk_h1_is_ignored_as_section_boundary`
10. `chunk_empty_body_produces_no_chunks`

### 17.3 Embedding provider

1. `openai_compat_embed_success_returns_correct_dimension`
2. `openai_compat_embed_dimension_mismatch_errors`
3. `openai_compat_embed_http_4xx_errors_auth`
4. `openai_compat_embed_http_5xx_errors_http`
5. `openai_compat_embed_timeout_errors_timeout`
6. `openai_compat_embed_parse_failure_errors_parse`
7. (integ) `openai_compat_embed_roundtrip_local_mock_server` — mock HTTP 서버

### 17.4 Write path (integ)

1. `write_creates_pages_row_and_chunks_row`
2. `write_reembed_deletes_old_chunks`
3. `write_async_mode_returns_before_embedding_completes`
4. `write_sync_mode_waits_for_embedding`
5. `write_embedding_failure_does_not_block_disk_write`

### 17.5 Search (integ)

1. `search_semantic_matches_synonyms`
2. `search_keyword_matches_exact`
3. `search_hybrid_rrf_fuses_correctly`
4. `search_page_level_dedup_across_chunks`
5. `search_limit_parameter_respected`
6. `search_no_keyword_match_falls_back_to_semantic_only`
7. `search_snippet_is_first_200_chars_of_match_chunk`
8. `search_empty_wiki_returns_empty_hits`

### 17.6 Reindex (integ)

1. `reindex_incremental_picks_up_manually_edited_file`
2. `reindex_incremental_removes_deleted_file_from_db`
3. `reindex_full_truncates_and_rebuilds`
4. `reindex_dry_run_makes_no_db_changes`

### 17.7 E2E (수동 또는 게이트된 통합 테스트)

- "서버가 안 켜져요" → "부팅 실패" 페이지 top-3 안에 반환
- 대화에서 AI가 "저장할까요?" 제안 → 승인 → 프론트매터 포함 wiki.write → 검색으로 다시 찾기

---

## 18. Open questions

1. **프론트매터 마이그레이션**: 기존 페이지(프론트매터 없음) → reindex가 `frontmatter = {}`로 처리. 별도 마이그레이션 CLI 필요 여부는 dogfood 후 결정.
2. **청크 크기 도메인 최적화**: GPU 트러블슈팅 vs 일반 노트에서 최적 크기가 다를 수 있음. dogfood 후 조정.
3. **API 키 로테이션**: `api_key_env`가 가리키는 환경변수가 중간에 바뀌면? Hot reload 없음, serve 재시작 필요 — 명시.

---

## 19. Out of scope / deferred

### P2B

- 로컬 임베딩 모델 정식 지원 (Ollama, bge-m3)
- AI 자동 병합/모순 감지
- 한국어 FTS 사전 (pg_bigm / ICU)
- 백링크 인덱스 + 그래프 탐색
- 청크 단위 snippet 개선 (하이라이팅)
- 임베딩 retry queue
- Approval-based auto merge/update
- Multi-wiki registry (`wiki_registry: Vec<WikiEntry>` with scope)

### P2C

- 멀티테넌트 tenant_id 필터링 (스키마만 P2A에 준비)
- GraphRAG
- Cross-tenant 지식 공유 권한 모델

---

## 20. Review provenance

- Office hours session (2026-04-16, /office-hours): 3-step questioning, Codex cross-model second opinion, premise challenge (16 issues), revised doc 91/100.
- Original: `~/.gstack/projects/NacBang-gadgetron/junghopark-main-design-20260416-113323.md`
- ADR: `docs/adr/ADR-P2A-07-semantic-wiki-pgvector.md`
- Reviewer issues addressed: BM25→ts_rank, dimension mismatch migration, sync/async config naming, Korean FTS path, frontmatter-less pages, source enum, chunk min size, wiki audit scope, content_hash algorithm, reindex on_startup config key.

*End of 05-knowledge-semantic.md draft v1.*
