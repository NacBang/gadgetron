# Knowledge Plug Architecture

> **담당**: PM (Codex)
> **상태**: Approved
> **작성일**: 2026-04-18
> **최종 업데이트**: 2026-04-18
> **관련 크레이트**: `gadgetron-core`, `gadgetron-knowledge`, `gadgetron-cli`, `gadgetron-xaas`
> **Phase**: [P2B] primary, [P2C] graph/relation expansion, [P3] optional non-wiki canonical stores
> **관련 문서**: `docs/design/phase2/01-knowledge-layer.md`, `docs/design/phase2/05-knowledge-semantic.md`, `docs/design/phase2/06-backend-plugin-architecture.md`, `docs/design/phase2/09-knowledge-acl.md`, `docs/design/phase2/10-penny-permission-inheritance.md`, `docs/design/phase2/11-raw-ingestion-and-rag.md`, `docs/adr/ADR-P2A-10-bundle-plug-gadget-terminology.md`, `docs/process/04-decision-log.md` D-20260418-01, D-20260418-03

---

## 1. 철학 & 컨셉 (Why)

### 1.1 해결하는 문제

현재 knowledge layer 는 구현과 도구 surface 양쪽에서 **LLM Wiki 하나를 사실상의 엔진으로 가정**한다. 이 구조는 P2A 에서는 빠르게 기능을 내는 데 유리했지만, 다음 두 요구를 동시에 만족시키지 못한다.

1. 사람과 Penny 가 공통으로 읽고 수정할 수 있는 **canonical knowledge source**
2. graphify 같은 graph/query 엔진, pgvector 같은 semantic index, future extractor/ingest pipeline 을 **동일 knowledge plane 위에서 조합 가능한 구조**

결과적으로 지금 구조는 `wiki = knowledge` 라는 결합을 만들고 있고, 사용자가 제기한 "LLM Wiki 말고 graphify 나 다른 지식 기반도 호환되어야 하지 않나?" 라는 요구를 수용할 seam 이 부족하다.

### 1.2 제품 비전과의 연결

`docs/00-overview.md §1` 의 Gadgetron 은 "지식 협업 플랫폼" 이지 "LLM Wiki 전용 앱" 이 아니다. `docs/design/phase2/06-backend-plugin-architecture.md` 와 D-20260418-01 이 확정한 원칙도 동일하다.

- **Knowledge is core. Capabilities are pluggable.**
- flat peer Bundle/Plug/Gadget 구조
- core 는 plugin-agnostic contract 와 orchestration 을 소유

따라서 정답은 "모든 지식을 하나의 백엔드 엔진으로 밀어 넣는다" 가 아니라:

> **하나의 core knowledge contract 위에 여러 knowledge plug 를 조합한다.**

### 1.3 고려한 대안과 기각 이유

| 대안 | 장점 | 단점 | 결론 |
|---|---|---|---|
| A. 현 구조 유지 (`wiki = knowledge`) | 구현 단순, P2A 코드 재사용 극대화 | graph/query 계열 확장 불가, raw ingestion/ACL/semantic 가 모두 wiki 구현 세부에 종속 | 기각 |
| B. graphify 같은 graph backend 로 knowledge 를 전면 교체 | graph traversal 에 강함 | markdown/git/audit/human-editability 약화, 현재 wiki.write / ACL / seed page model 과 충돌 | 기각 |
| C. core knowledge contract + canonical store 1개 + derived/query plug N개 | source-of-truth 보존, graph/vector/query 조합 가능, bundle 구조와 정합 | orchestration layer 추가 필요 | **채택** |
| D. `KnowledgeEngine` 단일 trait 하나에 store/search/graph/reindex 전부 몰아넣기 | 인터페이스 수 감소 | 최저공배수 인터페이스가 되어 graph/vector/store 특성이 모두 희석됨 | 기각 |

### 1.4 핵심 설계 원칙과 trade-off

1. **Canonical store 와 derived engine 을 분리한다.**
   `llm-wiki` 는 기본 canonical store 이다. semantic index, graphify, keyword index 는 canonical write 의 파생물이다.
2. **Core 는 contract 와 orchestration 을 소유하고, backend-specific logic 는 Plug 로 분리한다.**
   이는 D-20260418-01 의 core-vs-plugin 판정 룰과 일치한다.
3. **P2B 는 wiki replacement 가 아니라 wiki decoupling 이다.**
   즉, P2B 의 목표는 "wiki를 없앤다" 가 아니라 "wiki가 knowledge plane 의 한 구현이 되게 만든다" 이다.
4. **기존 `wiki.*` Gadget surface 는 유지한다.**
   Penny prompt, CLI, Web UI, 테스트를 한 번에 깨지 않기 위해 P2B 에서는 `wiki.*` 를 compatibility surface 로 유지한다.
5. **graph 계열 backend 는 먼저 relation/query plug 로 도입한다.**
   canonical source 까지 graph 로 바꾸는 것은 [P3] 이후 별도 검토다. P2B/P2C 에서는 graphify 를 derived relation engine 으로 수용한다.

Trade-off 는 분명하다.

- 장점: 구조 유연성, source-of-truth 안정성, reindex 복구 가능성 유지
- 비용: trait/registry/orchestration 계층이 하나 더 생김
- 수용 이유: 현재와 같은 wiki-hardcoded 구조는 Phase 2 이후 지식 백엔드 확장 비용이 더 커지기 때문

---

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API

P2B 에서 `gadgetron-core` 는 새 `knowledge` 모듈을 추가하고, `gadgetron-knowledge` 는 기존 `Wiki` 중심 구현을 `KnowledgeService` orchestration 으로 감싼다.

#### 2.1.1 `gadgetron-core::knowledge` 타입/트레이트

```rust
// crates/gadgetron-core/src/knowledge/mod.rs

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::bundle::PlugId;
use crate::error::GadgetronError;

pub type KnowledgeResult<T> = std::result::Result<T, GadgetronError>;

/// 08/09/10 문서에서 확정될 caller identity 타입.
/// 이 문서는 knowledge contract 가 caller 권한을 직접 받는다는 사실만 고정한다.
pub struct AuthenticatedContext;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeWriteConsistency {
    /// canonical store 성공만 write success 로 본다. 파생 엔진 반영 실패는 audit + 재색인 대상.
    StoreOnly,
    /// canonical store 성공 후 모든 derived engine 반영까지 기다린다.
    AwaitDerived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeQueryMode {
    Keyword,
    Semantic,
    Hybrid,
    Relations,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeHitKind {
    Canonical,
    SearchIndex,
    RelationEdge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeDocument {
    pub path: String,
    pub title: Option<String>,
    pub markdown: String,
    pub frontmatter: serde_json::Value,
    pub canonical_plug: PlugId,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgePutRequest {
    pub path: String,
    pub markdown: String,
    #[serde(default)]
    pub create_only: bool,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeWriteReceipt {
    pub path: String,
    pub canonical_plug: PlugId,
    pub revision: String,
    pub derived_failures: Vec<PlugId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeQuery {
    pub text: String,
    pub limit: u32,
    pub mode: KnowledgeQueryMode,
    #[serde(default)]
    pub include_relations: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeHit {
    pub path: String,
    pub title: Option<String>,
    pub snippet: String,
    pub score: f32,
    pub source_plug: PlugId,
    pub source_kind: KnowledgeHitKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeTraversalQuery {
    pub seed_path: String,
    pub relation: Option<String>,
    pub max_depth: u8,
    pub limit: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeTraversalResult {
    pub nodes: Vec<KnowledgeHit>,
    pub edges: Vec<KnowledgeRelationEdge>,
    pub source_plug: PlugId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeRelationEdge {
    pub from_path: String,
    pub to_path: String,
    pub relation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KnowledgeChangeEvent {
    Upsert { document: KnowledgeDocument },
    Delete { path: String, deleted_at: DateTime<Utc> },
    Rename { from: String, to: String, document: KnowledgeDocument },
}

#[async_trait]
pub trait KnowledgeStore: Send + Sync + std::fmt::Debug {
    fn plug_id(&self) -> &PlugId;

    async fn list(&self, actor: &AuthenticatedContext) -> KnowledgeResult<Vec<String>>;
    async fn get(
        &self,
        actor: &AuthenticatedContext,
        path: &str,
    ) -> KnowledgeResult<Option<KnowledgeDocument>>;
    async fn put(
        &self,
        actor: &AuthenticatedContext,
        request: KnowledgePutRequest,
    ) -> KnowledgeResult<KnowledgeWriteReceipt>;
    async fn delete(&self, actor: &AuthenticatedContext, path: &str) -> KnowledgeResult<()>;
    async fn rename(
        &self,
        actor: &AuthenticatedContext,
        from: &str,
        to: &str,
    ) -> KnowledgeResult<KnowledgeWriteReceipt>;
}

#[async_trait]
pub trait KnowledgeIndex: Send + Sync + std::fmt::Debug {
    fn plug_id(&self) -> &PlugId;
    fn mode(&self) -> KnowledgeQueryMode;

    async fn search(
        &self,
        actor: &AuthenticatedContext,
        query: &KnowledgeQuery,
    ) -> KnowledgeResult<Vec<KnowledgeHit>>;
    async fn reset(&self) -> KnowledgeResult<()>;
    async fn apply(
        &self,
        actor: &AuthenticatedContext,
        event: KnowledgeChangeEvent,
    ) -> KnowledgeResult<()>;
}

#[async_trait]
pub trait KnowledgeRelationEngine: Send + Sync + std::fmt::Debug {
    fn plug_id(&self) -> &PlugId;

    async fn traverse(
        &self,
        actor: &AuthenticatedContext,
        query: &KnowledgeTraversalQuery,
    ) -> KnowledgeResult<KnowledgeTraversalResult>;
    async fn reset(&self) -> KnowledgeResult<()>;
    async fn apply(
        &self,
        actor: &AuthenticatedContext,
        event: KnowledgeChangeEvent,
    ) -> KnowledgeResult<()>;
}
```

#### 2.1.2 `gadgetron-knowledge::service::KnowledgeService`

```rust
// crates/gadgetron-knowledge/src/service.rs

use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};

use gadgetron_core::knowledge::{
    AuthenticatedContext, KnowledgeChangeEvent, KnowledgeDocument, KnowledgeHit,
    KnowledgeIndex, KnowledgePutRequest, KnowledgeQuery, KnowledgeQueryMode,
    KnowledgeRelationEngine, KnowledgeStore, KnowledgeTraversalQuery,
    KnowledgeTraversalResult, KnowledgeWriteConsistency, KnowledgeWriteReceipt,
};

pub struct KnowledgeService {
    canonical_store: Arc<dyn KnowledgeStore>,
    indexes: Vec<Arc<dyn KnowledgeIndex>>,
    relation_engines: Vec<Arc<dyn KnowledgeRelationEngine>>,
    write_consistency: KnowledgeWriteConsistency,
}

impl KnowledgeService {
    pub async fn list(&self, actor: &AuthenticatedContext) -> Result<Vec<String>, GadgetronError>;
    pub async fn get(
        &self,
        actor: &AuthenticatedContext,
        path: &str,
    ) -> Result<Option<KnowledgeDocument>, GadgetronError>;
    pub async fn write(
        &self,
        actor: &AuthenticatedContext,
        request: KnowledgePutRequest,
    ) -> Result<KnowledgeWriteReceipt, GadgetronError>;
    pub async fn delete(&self, actor: &AuthenticatedContext, path: &str)
        -> Result<(), GadgetronError>;
    pub async fn rename(
        &self,
        actor: &AuthenticatedContext,
        from: &str,
        to: &str,
    ) -> Result<KnowledgeWriteReceipt, GadgetronError>;
    pub async fn search(
        &self,
        actor: &AuthenticatedContext,
        query: &KnowledgeQuery,
    ) -> Result<Vec<KnowledgeHit>, GadgetronError>;
    pub async fn traverse(
        &self,
        actor: &AuthenticatedContext,
        query: &KnowledgeTraversalQuery,
    ) -> Result<Vec<KnowledgeTraversalResult>, GadgetronError>;
    pub async fn reindex_all(&self, actor: &AuthenticatedContext)
        -> Result<(), GadgetronError>;
}
```

#### 2.1.3 Plug registration contract

`docs/design/phase2/06-backend-plugin-architecture.md` 와 ADR-P2A-10 의 future `BundleContext` shape 에 맞춰 knowledge-related Plug registration 은 다음 식으로 고정한다.

```rust
ctx.plugs.knowledge_stores.register(
    PlugId::new("llm-wiki")?,
    Arc::new(LlmWikiStore::new(cfg)?),
);

ctx.plugs.knowledge_indexes.register(
    PlugId::new("wiki-keyword")?,
    Arc::new(WikiKeywordIndex::new()),
);

ctx.plugs.knowledge_indexes.register(
    PlugId::new("semantic-pgvector")?,
    Arc::new(SemanticPgVectorIndex::new(pool, embedder)?),
);

ctx.plugs.knowledge_relations.register(
    PlugId::new("graphify")?,
    Arc::new(GraphifyRelationEngine::new(runtime_cfg)?),
);
```

`llm-wiki`, `wiki-keyword`, `semantic-pgvector` 는 built-in/bundle-local Plug 이고, `graphify` 는 external runtime pilot bundle candidate 다. external runtime wiring 자체는 `12-external-gadget-runtime.md` 에서 고정한다.

### 2.2 내부 구조

#### 2.2.1 레이어 분리

P2B 이후 내부 구조는 다음 4층으로 나눈다.

1. **Core contract (`gadgetron-core`)**
   `KnowledgeStore` / `KnowledgeIndex` / `KnowledgeRelationEngine` / 공통 타입
2. **Knowledge orchestration (`gadgetron-knowledge`)**
   `KnowledgeService`, result fusion, reindex fanout, gadget adapter
3. **Canonical/default implementations (`gadgetron-knowledge`)**
   `LlmWikiStore`, `WikiKeywordIndex`, `SemanticPgVectorIndex`
4. **Optional backend plugs (future bundles)**
   `graphify`, domain-specific relation engines, alternate search engines

#### 2.2.2 현재 코드의 매핑

현재 구현은 다음처럼 재배치한다.

| 현재 | P2B 이후 |
|---|---|
| `wiki::Wiki` | `llm_wiki::LlmWikiStore` (`impl KnowledgeStore`) |
| `wiki::index` inverted index | `keyword_index::WikiKeywordIndex` (`impl KnowledgeIndex`) |
| semantic pgvector path | `semantic_index::SemanticPgVectorIndex` (`impl KnowledgeIndex`) |
| `KnowledgeGadgetProvider` | `KnowledgeGadgetProvider { service: Arc<KnowledgeService> }` |
| `maintenance::run_reindex` | `KnowledgeService::reindex_all()` 호출부로 흡수 |

핵심은 `KnowledgeGadgetProvider` 가 더 이상 `Wiki` 를 직접 잡지 않고, 항상 `KnowledgeService` 를 통해 canonical store + derived engines 를 오케스트레이션한다는 점이다.

#### 2.2.3 쓰기 경로

`wiki.write`, `wiki.delete`, `wiki.rename`, `wiki.import` 는 모두 다음 파이프라인을 탄다.

```text
Gadget/CLI/Web
  -> KnowledgeService
  -> canonical_store (authoritative)
  -> KnowledgeChangeEvent 생성
  -> indexes/relation_engines fanout
  -> audit/tracing
```

write algorithm:

1. caller ACL/validation 은 canonical store 직전에서 수행
2. canonical store 성공이 user-visible success 의 기준이다
3. 이후 `KnowledgeChangeEvent` 를 생성해 enabled index/relation plug 로 fanout 한다
4. `write_consistency = store_only` 이면 파생 반영 실패는 `derived_failures` 로 receipt 에 기록하고 write 자체는 성공
5. `write_consistency = await_derived` 이면 파생 반영 실패를 호출 오류로 승격한다

**기본값은 `store_only`** 다. 이유는 `docs/design/phase2/05-knowledge-semantic.md` 의 source-of-truth invariant 를 유지해야 하기 때문이다. filesystem+git canonical store 가 성공했는데 graphify 나 pgvector 장애 때문에 write 를 실패 처리하면 operator 경험이 급격히 나빠진다.

#### 2.2.4 검색 경로

검색은 query mode 에 따라 분기한다.

- `Keyword`: `mode() == Keyword` 인 index plug 만 조회
- `Semantic`: `mode() == Semantic` 인 index plug 만 조회
- `Hybrid`: keyword + semantic 을 병렬 조회 후 RRF fusion
- `Relations`: relation engine 만 조회
- `Auto`: enabled search plug 를 병렬 조회하고 relation 은 `include_relations = true` 일 때만 추가

result fusion 규칙:

1. 동일 `path` hit 는 `path` 기준으로 dedup
2. score 는 raw score 그대로 노출하지 않고 fused rank score 로 재계산
3. snippet 은 canonical markdown snippet 우선, 없으면 backend snippet 사용
4. relation engine hit 는 `source_kind = RelationEdge` 로 태깅

#### 2.2.5 동시성 모델

- `KnowledgeService` 자체는 immutable `Arc` graph 이며 global lock 을 두지 않는다
- 병렬 검색은 `FuturesUnordered` 로 fanout 한다
- canonical store 내부 lock 전략은 구현체 소유다
  - `LlmWikiStore`: git/index/filesystem critical section 중심
  - `SemanticPgVectorIndex`: DB transaction 중심
  - `GraphifyRelationEngine`: external runtime RPC timeout 중심
- 비동기 apply fanout 이 필요한 경우 `tokio::sync::mpsc` worker 를 쓴다
- hot path 는 canonical `get/list/search` 이므로 `Vec<Arc<dyn Trait>>` + read-only dispatch 를 유지하고 `DashMap`/dynamic registration 은 도입하지 않는다

### 2.3 설정 스키마

P2B config 는 **선택/조합** 과 **구현별 상세 설정** 을 분리한다.

```toml
[knowledge]
canonical_store = "llm-wiki"
search_plugs = ["wiki-keyword", "semantic-pgvector"]
relation_plugs = []
write_consistency = "store_only"      # store_only | await_derived
derived_apply_timeout_ms = 3000
fallback_to_keyword_when_no_semantic_hits = true

[knowledge.store.llm-wiki]
wiki_path = "/srv/gadgetron/wiki"
wiki_autocommit = true
wiki_git_author = "Penny <penny@gadgetron.local>"
wiki_max_page_bytes = 1048576

[knowledge.index.semantic-pgvector]
enabled = true
pool = "xaas-postgres"
embedder = "openai-embedding"
query_timeout_ms = 1500

[knowledge.relation.graphify]
enabled = false
top_k = 8
rpc_timeout_ms = 1500
```

#### 2.3.1 backward-compatible sugar

기존 P2A config 는 그대로 허용한다.

- `[knowledge] wiki_path`, `wiki_autocommit`, `wiki_git_author`, `wiki_max_page_bytes`
  -> `[knowledge.store.llm-wiki]` 로 내부 변환
- `[knowledge.embedding]`
  -> `[knowledge.index.semantic-pgvector]` 로 내부 변환
- `[knowledge.search]`
  -> `wiki-keyword` 또는 external web search gadget 설정으로 계속 사용

#### 2.3.2 검증 규칙

1. `canonical_store` 는 정확히 하나의 등록된 `KnowledgeStore` plug 여야 한다
2. `search_plugs` 는 `KnowledgeIndex` 로 등록된 plug id 만 참조할 수 있다
3. `relation_plugs` 는 `KnowledgeRelationEngine` 으로 등록된 plug id 만 참조할 수 있다
4. 같은 plug id 가 여러 capability registry 에 중복 등록되면 startup fail
5. `write_consistency = await_derived` 일 때 `derived_apply_timeout_ms` 는 필수, 범위 `[100, 30000]`
6. disabled plug 를 `canonical_store`/`search_plugs`/`relation_plugs` 에 참조하면 startup fail
7. `graphify` 같은 external relation plug 가 enable 되어도 canonical store 는 [P2B] 에서 `llm-wiki` 만 허용

### 2.4 에러 & 로깅

#### 2.4.1 에러 모델

P2B 에서 knowledge plane 전용 nested error kind 를 추가한다.

```rust
// crates/gadgetron-core/src/error.rs

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnowledgeErrorKind {
    BackendNotRegistered { plug: String },
    BackendUnavailable { plug: String },
    DocumentNotFound { path: String },
    InvalidQuery { reason: String },
    DerivedApplyFailed { plug: String },
}

// GadgetronError 에 nested variant 추가
Knowledge { kind: KnowledgeErrorKind, message: String },
```

기존 `WikiErrorKind` 는 삭제하지 않는다. `llm-wiki` store 내부 구현 에러는 여전히 `WikiErrorKind` 를 쓸 수 있지만, `KnowledgeService` boundary 바깥으로 나갈 때는 `KnowledgeErrorKind` 로 정규화한다.

예시:

- canonical store 미등록 -> `Knowledge::BackendNotRegistered`
- graphify runtime timeout -> `Knowledge::BackendUnavailable`
- traversal query malformed -> `Knowledge::InvalidQuery`
- `wiki.get` on missing page -> `Knowledge::DocumentNotFound`

#### 2.4.2 tracing

필수 span/event:

| span/event | level | 필드 |
|---|---|---|
| `knowledge.write` | INFO | `path`, `canonical_plug`, `write_consistency`, `actor_user_id` |
| `knowledge.apply_change` | WARN on failure | `event_kind`, `target_plug`, `path`, `elapsed_ms` |
| `knowledge.search` | INFO | `mode`, `search_plugs`, `relation_plugs`, `limit`, `actor_user_id` |
| `knowledge.traverse` | INFO | `seed_path`, `relation`, `max_depth`, `target_plug` |
| `knowledge.reindex` | INFO | `canonical_plug`, `index_count`, `relation_count` |

#### 2.4.3 STRIDE threat model 요약

| 자산 | 신뢰 경계 | 위협 | 완화 |
|---|---|---|---|
| canonical markdown + git history | user -> gadget / store | Tampering | canonical store 단일 권위, git audit, ACL 검사 |
| derived semantic/graph indexes | service -> backend plug | Tampering / DoS | `store_only` 기본값, `reindex_all`, timeout, health check |
| caller 권한 정보 | gateway -> knowledge service | Spoofing / EoP | `AuthenticatedContext` 상속, store/index/relation 모두 actor 전달 |
| external relation backend RPC | core -> external runtime | Info disclosure / DoS | loopback/bearer/runtime sandbox, timeout, no raw secret logging |
| search result surface | backend -> Penny/UI | Repudiation / injection-like contamination | source_plug tagging, snippet normalization, citation path 유지 |

보안 원칙:

- default-deny: unregistered/disabled backend 는 절대 선택되지 않는다
- derived backend 실패 메시지는 runtime 내부 경로/stack trace 를 노출하지 않는다
- external graph engine 은 canonical write 권한을 갖지 않는다
- `graphify` 결과는 항상 citation 가능한 canonical path 로 귀결되어야 한다

### 2.5 의존성

P2B core/knowledge 작업에는 새 heavyweight dependency 를 도입하지 않는다.

- `gadgetron-core`
  - 추가: 없음 (`async-trait`, `serde`, `chrono`, `uuid` 기존 workspace 사용)
- `gadgetron-knowledge`
  - 추가: 없음 (`futures`, `tokio` 이미 workspace 사용)
- external graph/relation runtime
  - `12-external-gadget-runtime.md` 가 정의하는 subprocess/HTTP runtime 을 사용
  - graphify crate/python package 자체는 core dependency 가 아니라 bundle/runtime dependency 로 격리

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 상위/하위 의존 구도

```text
Penny / CLI / Web UI
    -> KnowledgeGadgetProvider
        -> KnowledgeService                  (gadgetron-knowledge)
            -> KnowledgeStore               (core trait)
            -> KnowledgeIndex[*]            (core trait)
            -> KnowledgeRelationEngine[*]   (core trait)

Default implementations:
KnowledgeService
    -> LlmWikiStore                         (gadgetron-knowledge)
    -> WikiKeywordIndex                     (gadgetron-knowledge)
    -> SemanticPgVectorIndex                (gadgetron-knowledge)

Future bundles:
KnowledgeService
    -> GraphifyRelationEngine               (bundle / external runtime)
```

### 3.2 데이터 흐름 다이어그램

```text
                write/delete/rename/import
User/Penny/UI  ------------------------------+
                                             v
                                   +------------------+
                                   | KnowledgeService |
                                   +--------+---------+
                                            |
                                            v
                                   +------------------+
                                   | canonical store  |
                                   |   llm-wiki       |
                                   +--------+---------+
                                            |
                                KnowledgeChangeEvent
                       +--------------------+--------------------+
                       |                                         |
                       v                                         v
             +--------------------+                   +----------------------+
             | search index plugs |                   | relation engine plugs|
             | keyword / semantic |                   | graphify / future    |
             +--------------------+                   +----------------------+

search:
KnowledgeGadgetProvider -> KnowledgeService -> search index plugs -> RRF fusion
traverse:
KnowledgeGadgetProvider -> KnowledgeService -> relation engine plugs
```

### 3.3 타 서브에이전트 도메인과의 인터페이스 계약

1. **`09-knowledge-acl.md`**
   모든 store/index/relation trait method 는 caller `AuthenticatedContext` 를 받는다. ACL filtering 을 `KnowledgeService` 밖으로 밀어내지 않는다.
2. **`10-penny-permission-inheritance.md`**
   Penny 는 knowledge tool 호출 시 caller identity 를 그대로 상속한다. derived backend fanout 도 동일 actor 로 audit 된다.
3. **`11-raw-ingestion-and-rag.md`**
   `wiki.import` 는 canonical store write 뒤 `KnowledgeChangeEvent::Upsert` 를 발행한다. blob/extractor pipeline 과 knowledge plug fanout 은 같은 write pipeline 내에서 연결된다.
4. **`06-backend-plugin-architecture.md` / D-20260418-01**
   future bundles 는 flat peer 로 knowledge plug 를 제공하되, core knowledge plane contract 는 `gadgetron-core` 가 소유한다.

### 3.4 D-12 크레이트 경계표 준수 여부

본 설계는 D-12 와 충돌하지 않는다.

- `gadgetron-core`
  - trait + config + error + wire types 만 추가
- `gadgetron-knowledge`
  - orchestration + default implementation 보유
- bundle/external runtime
  - graph/query backend-specific logic 보유

즉, core leaf 원칙을 유지한다. `graphify` 같은 backend-specific transport/client 코드는 core 로 올라오지 않는다.

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

#### `gadgetron-core`

| 대상 | 검증 invariant |
|---|---|
| `KnowledgeWriteConsistency` serde | TOML/JSON round-trip 이 안정적이어야 함 |
| `KnowledgeQueryMode` parse/serde | unknown mode 거부, wire string 고정 |
| `KnowledgeChangeEvent` serde | `Upsert/Delete/Rename` round-trip |
| config validation helper | canonical store/search/relation plug 참조 무결성 |
| `KnowledgeErrorKind` mapping | HTTP/error code 분기용 string 안정성 |

#### `gadgetron-knowledge`

| 대상 | 검증 invariant |
|---|---|
| `KnowledgeService::write` | canonical store 성공 후 fanout 실패가 `store_only` 에서 write 전체를 깨지 않음 |
| `KnowledgeService::write` (`await_derived`) | derived failure 가 전체 실패로 승격됨 |
| `KnowledgeService::search` | mode 별 plug fanout 범위가 정확함 |
| result fusion | 동일 path dedup + rank 안정성 |
| `KnowledgeService::reindex_all` | canonical docs 기준으로 search/relation backend reset+apply |
| `KnowledgeGadgetProvider` adapter | 기존 `wiki.*` 가 service 호출로 정확히 위임됨 |

### 4.2 테스트 하네스

- fake canonical store: in-memory `Vec<KnowledgeDocument>` 기반
- fake search index: deterministic hit list 반환
- fake relation engine: path graph fixture 반환
- existing tempdir wiki harness: `LlmWikiStore` 단위 테스트용 재사용
- semantic index: 현재 `FakeEmbeddingProvider` + test Postgres harness 재사용
- property-based test:
  - result fusion dedup property
  - rename/delete/apply sequence invariants

### 4.3 커버리지 목표

- `gadgetron-core::knowledge`: line 90%+, branch 80%+
- `gadgetron-knowledge::service`: line 90%+, branch 85%+
- backward compatibility adapter (`KnowledgeGadgetProvider`): 기존 `wiki.*` gadget 회귀 케이스 100%

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

1. **LLM Wiki only**
   `wiki.write -> wiki.search -> wiki.get`
2. **Keyword + Semantic hybrid**
   canonical wiki write 후 semantic + keyword 결과가 함께 검색되는지
3. **Derived backend failure tolerance**
   semantic 또는 relation backend down 상태에서 `store_only` write 가 성공하고 audit/tracing 이 남는지
4. **Graph relation plug**
   canonical wiki write -> relation engine apply -> `knowledge.traverse`/future relation query 가 canonical path 기반 결과를 내는지
5. **Fresh-start recovery**
   wiki 파일은 존재하지만 semantic/relation state 가 비어 있을 때 `reindex_all` 로 복구되는지

### 5.2 테스트 환경

- tempdir wiki root (`git2` 실 repo)
- PostgreSQL + pgvector test harness (기존 semantic test 재사용)
- fake relation engine 는 unit/integration 기본값
- external runtime smoke test [P2C]:
  loopback HTTP 또는 stdio stub relation bundle 로 1개 시나리오

### 5.3 회귀 방지

다음 변경은 통합 테스트가 반드시 실패시켜야 한다.

- canonical store success 인데 semantic failure 때문에 `wiki.write` 가 무조건 실패하는 회귀
- `wiki.search` 가 semantic backend 존재 시 filesystem keyword fallback 을 완전히 잃는 회귀
- relation engine hit 가 canonical path 없는 opaque node 로만 반환되는 회귀
- `reindex_all` 이 delete/rename 이벤트를 반영하지 못하는 회귀
- `wiki.*` gadget surface 가 service bypass 로 다시 `Wiki` 구현 세부에 직접 결합되는 회귀

---

## 6. Phase 구분

### [P2B]

- `gadgetron-core::knowledge` contract 추가
- `KnowledgeService` 추가
- `LlmWikiStore`, `WikiKeywordIndex`, `SemanticPgVectorIndex` 로 현재 구현 분리
- 기존 `wiki.*` Gadget surface 를 `KnowledgeService` 뒤로 재배선
- config backward-compat sugar 유지
- `KnowledgeErrorKind` 추가

### [P2C]

- `KnowledgeRelationEngine` 실제 bundle integration
- `graphify` external runtime pilot
- `knowledge.traverse` 또는 relation-aware Gadget surface 추가
- external runtime health/timeout/approval integration

### [P3]

- `llm-wiki` 외 canonical store 허용 여부 재검토
- multi-wiki registry 와 knowledge plug selection 결합
- canonical store migration tooling (`wiki -> alt-store`) 검토

---

## 7. 오픈 이슈 / 의사결정 필요

| ID | 내용 | 옵션 | 추천 | 상태 |
|-----|------|------|------|------|
| 없음 | 현재 문서 기준 구현 진입 blocker 없음 | N/A | N/A | 닫힘 |

---

## 리뷰 로그 (append-only)

### Round 1 — 2026-04-18 — @gateway-router-lead @xaas-platform-lead
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
- 없음

**다음 라운드 조건**:
- Round 1.5 진행

### Round 1.5 — 2026-04-18 — @security-compliance-lead @dx-product-lead
**결론**: Pass

(`03-review-rubric.md §1.5` 기준)

- [x] 위협 모델
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
- [x] CLI/API/config defaults 안전성
- [x] 하위 호환

**Action Items**:
- 없음

**다음 라운드 조건**:
- Round 2 진행

### Round 2 — 2026-04-18 — @qa-test-architect
**결론**: Pass

(`03-review-rubric.md §2` 기준)

- [x] 단위 테스트 범위
- [x] mock 가능성
- [x] 결정론
- [x] 통합 시나리오
- [x] CI 재현성
- [x] 성능 검증
- [x] 회귀 테스트
- [x] 테스트 데이터

**Action Items**:
- 없음

**다음 라운드 조건**:
- Round 3 진행

### Round 3 — 2026-04-18 — @chief-architect
**결론**: Pass

(`03-review-rubric.md §3` 기준)

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
- 없음

**다음 라운드 조건**:
- PM 최종 승인

### 최종 승인 — 2026-04-18 — PM

- 구현 진입 조건 충족
- canonical store / derived backend / relation engine 분리가 명시적
- 기존 `wiki.*` surface 를 보존하면서 knowledge plug 도입 가능
