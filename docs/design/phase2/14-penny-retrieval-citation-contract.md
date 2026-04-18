# 14 — Penny Retrieval & Citation Contract

> **담당**: PM (Codex)
> **상태**: Approved
> **작성일**: 2026-04-18
> **최종 업데이트**: 2026-04-18
> **관련 크레이트**: `gadgetron-penny`, `gadgetron-knowledge`, `gadgetron-core`, `plugins/plugin-document-formats`, `gadgetron-gateway`, `gadgetron-web`
> **Phase**: [P2B] primary / [P2C] anchored evidence enrichment / [P3] rich citation renderer
> **관련 문서**: `docs/design/phase2/02-penny-agent.md`, `docs/design/phase2/11-raw-ingestion-and-rag.md`, `docs/design/phase2/13-penny-shared-surface-loop.md`, `docs/design/gateway/workbench-projection-and-actions.md`, `docs/design/web/expert-knowledge-workbench.md`, `docs/process/04-decision-log.md` D-20260418-13
> **보정 범위**: 현재 trunk 기준 retrieved-answer / footnote / parser / evidence surface 계약은 이 문서가 authoritative 이다. `11-raw-ingestion-and-rag.md`의 §8.3 PDF 예시와 §9.3 hyperlink footnote 예시는 ingest foundation 문맥으로 유지하되, 현재 operator-visible citation wire contract 는 본 문서가 우선한다.

---

## 1. 철학 & 컨셉 (Why)

### 1.1 이 문서가 닫는 공백

2026-04-18 기준 `origin/main` 은 이미 다음을 갖고 있다.

1. `crates/gadgetron-penny/src/spawn.rs` 에 search-first RAG prompt 규칙이 들어갔다.
2. `crates/gadgetron-penny/src/citation.rs` 가 footnote definition parser 를 노출한다.
3. `plugins/plugin-document-formats/src/pdf.rs` 가 페이지 단위 PDF text extraction 과 `StructureHint::PageBreak` 생성을 구현한다.
4. `crates/gadgetron-knowledge/tests/rag_citation_e2e.rs` 가 import -> search -> footnote round-trip 을 검증한다.
5. `docs/design/gateway/workbench-projection-and-actions.md` 와 `docs/design/web/expert-knowledge-workbench.md` 가 request-scoped evidence surface 를 전제로 한다.

하지만 문서 SSOT 는 아직 분산되어 있다.

- `11-raw-ingestion-and-rag.md` 는 citation 포맷을 broader ingest 설계 안의 한 subsection 으로만 다룬다.
- 같은 문서의 PDF 예시는 trunk 구현보다 낡았고, `extract_text_from_mem_by_pages` 기반 page-break anchor 계약을 반영하지 않는다.
- 현재 Penny 응답이 **plain markdown footnote body** 를 내보낸다는 사실과, UI/evidence layer 가 이를 어떻게 소비해야 하는지가 독립 문서로 닫혀 있지 않다.
- `wiki.search` 결과의 `section` 이 best-effort display hint 인지, stable anchor 인지에 대한 경계가 문서로 고정돼 있지 않다.

이 seam 이 열려 있으면 다음 문제가 생긴다.

1. manual / web / future renderer 가 각자 다른 regex 와 다른 footnote 해석을 갖게 된다.
2. operator 는 현재 trunk 가 hyperlink citation 인지 plain footnote 인지 혼동한다.
3. PDF import 가 실제로는 bundle-registered extractor 경로에 걸려 있는데, 기본 path 도 동일하게 동작한다고 오해하기 쉽다.
4. workbench evidence surface 가 actor-verified provenance 대신 model markdown 을 과신하는 위험이 생긴다.

이 문서는 그 공백을 닫는다.

### 1.2 제품 비전과의 연결

`docs/00-overview.md §1` 의 Gadgetron 은 "지식 협업 플랫폼" 이다. 따라서 Penny 의 retrieved answer 는 단순히 "그럴듯한 답변" 이 아니라, 다음 세 성질을 동시에 만족해야 한다.

1. **검색 근거가 먼저다.** 지식성 질문에는 먼저 `wiki.search` 를 호출한다.
2. **인용이 복사 가능해야 한다.** operator 는 Penny 답변을 티켓, 회고, runbook 초안으로 그대로 옮길 수 있어야 한다.
3. **증거 해석은 shared surface 로 돌아와야 한다.** chat transcript 만으로 provenance 를 독점하지 않고, gateway/workbench evidence surface 가 같은 사실 모델을 재구성할 수 있어야 한다.

즉, 현재 trunk 의 citation surface 는 "예쁜 링크 렌더링" 보다 **정확한 page path 보존** 을 우선한다.

### 1.3 고려한 대안과 채택하지 않은 이유

| 대안 | 설명 | 채택하지 않은 이유 |
|---|---|---|
| A. `11-raw-ingestion-and-rag.md` subsection 만 계속 authoritative 로 둔다 | 문서 수가 적다 | ingest foundation, prompt contract, parser contract, workbench evidence seam 이 한 문단에 과적재된다 |
| B. prompt text 와 parser 규약을 코드 주석으로만 관리한다 | 구현자에겐 가깝다 | operator-facing contract, review log, security/usability gate 를 통과하지 못한다 |
| C. footnote body 를 바로 hyperlink markdown 으로 강제한다 | UI 가 보기 좋다 | current trunk parser / renderer / request_evidence 경계와 맞지 않고 copy-paste 내구성이 낮다 |
| D. plain markdown footnote + shared parser + actor-verified evidence enrichment 를 별도 설계 문서로 고정한다 | current trunk 와 가장 정합적, future renderer 확장도 명확 | 문서가 하나 늘지만 seam 이 분리된다 |

채택: **D. plain markdown footnote + shared parser + actor-verified evidence enrichment**

### 1.4 핵심 설계 원칙과 trade-off

1. **Search precedes answer**
   Penny 는 knowledge-relevant 질문에 대해 먼저 `wiki.search` 를 호출한다. "이미 아는 것 같다" 는 이유로 citation 없는 답변을 우선하지 않는다.
2. **Path fidelity beats pretty formatting**
   현재 trunk citation body 는 렌더용 hyperlink 보다 backticked `page_name` 의 verbatim 보존을 우선한다.
3. **Parser is shared, not reimplemented**
   `gadgetron-penny::citation` 이 footnote definition 의 단일 문법이다. Web UI, CLI, future renderer 는 별도 regex 를 만들지 않는다.
4. **Display hint is not stable anchor**
   `wiki.search` payload 의 `section` 은 현재 best-effort hint 이다. stable byte anchor 는 아니다.
5. **Evidence enrichment is actor-verified**
   parser 가 뽑은 footnote body 는 "보여준 것" 이고, shared surface 가 확인한 request evidence 가 "사실 확인된 provenance" 다. 둘을 구분한다.
6. **Graceful degradation is explicit**
   section/page/blob deep link 가 없으면 citation 은 page path 수준으로 degrade 한다. 빈 anchor 를 꾸며내지 않는다.
7. **PDF support is capability-gated**
   PDF citation path 는 `PdfExtractor` 가 등록된 배포에서만 operator-visible contract 다. built-in markdown-only path 에서 PDF import 가 된다고 문서가 약속하지 않는다.

Trade-off:

- 현재 footnote body 는 hyperlink-rich UI 보다 투박하다.
- 대신 current trunk 의 prompt, parser, copy-paste workflow, headless test path 와 정확히 맞는다.

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API

현재 trunk 에서 load-bearing public surface 는 세 층이다.

1. `wiki.search` 의 gadget payload
2. Penny markdown footnote parser
3. PDF page-break anchor contract

#### 2.1.1 `wiki.search` payload (`gadgetron-knowledge`)

```rust
// crates/gadgetron-knowledge/src/gadget.rs

#[derive(Debug, Clone, Serialize)]
pub struct SearchHitPayload {
    pub page_name: String,
    pub score: f32,
    pub section: Option<String>,
    pub snippet: Option<String>,
}
```

Contract:

- `page_name` 는 citation 의 canonical identity 다. Penny 는 이 값을 그대로 footnote body 에 사용한다.
- `section` 은 현재 `KnowledgeHit.title` 기반 best-effort hint 다.
  - `KnowledgeHitKind::SearchIndex` / `Canonical` hit 는 `title` 값을 `section` 으로 노출할 수 있다.
  - `KnowledgeHitKind::RelationEdge` hit 는 `section = None` 이다.
- `section` 은 stable chunk id 나 byte offset 이 아니다. renderers 는 deep link anchor 로 오해하면 안 된다.
- `snippet` 은 prose 작성 보조다. citation identity 는 아니다.

#### 2.1.2 Citation parser (`gadgetron-penny`)

```rust
// crates/gadgetron-penny/src/citation.rs

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitationRef {
    pub label: String,
    pub body: String,
}

pub fn extract_citation_refs(markdown: &str) -> Vec<CitationRef>;
pub fn extract_referenced_labels(markdown: &str) -> Vec<String>;
```

Contract:

- definition line 은 line-start footnote definition 만 인정한다: `[^label]: body`
- `label` 은 1-32 printable non-space chars (`alpha-1` 같은 named label 허용)
- 반환 순서는 source order 유지
- duplicate label 은 parser 가 dedup 하지 않는다
- inline reference 추출은 definition line 의 `[^N]:` 를 제외한다
- code-fence aware parser 가 아니다. fence 내부 footnote-like line 도 capture 될 수 있다
- 따라서 UI/CLI/renderers 가 parser 위에서 추가 전처리를 하고 싶다면, pre-strip 같은 보강은 가능하지만 **대체 문법** 을 만들어서는 안 된다

#### 2.1.3 PDF page-break anchor (`plugin-document-formats` + `gadgetron-core`)

```rust
// plugins/plugin-document-formats/src/pdf.rs
pub const PAGE_SEPARATOR: char = '\x0C';

// crates/gadgetron-core/src/ingest/extractor.rs
pub enum StructureHint {
    // ...
    PageBreak {
        byte_offset: usize,
        page_number: u32,
    },
}
```

Contract:

- `PdfExtractor` 는 `pdf_extract::extract_text_from_mem_by_pages` 결과를 `PAGE_SEPARATOR` 로 join 한다
- page 경계마다 `StructureHint::PageBreak` 를 emit 한다
- downstream chunker 는 이를 `source_page_hint` seed 로 사용한다
- current P2B citation body 는 page-level footnote 를 강제하지 않는다
- page 번호를 operator-facing surface 로 노출할 때는 request evidence / chunk metadata 를 통해 검증된 경우에만 `p.<N>` 을 붙인다

#### 2.1.4 Penny retrieved-answer prompt contract

P2B current contract 의 규칙은 다음과 같다.

```text
1. 지식성 질문이면 먼저 wiki.search 를 호출한다.
2. query 는 명사구/엔티티 중심 3~8 keywords 로 만든다.
3. hit relevance 가 애매하면 wiki.get 으로 page 본문을 확인한다.
4. retrieval 기반 factual claim 마다 [^N] footnote reference 를 붙인다.
5. 응답 끝에는 [^N]: <body> definition block 을 나열한다.
6. 관련 hit 가 없으면 "위키에 관련 페이지를 찾지 못했습니다" 를 명시한다.
7. page path / section / imported date 를 지어내지 않는다.
```

#### 2.1.5 Footnote body grammar

현재 trunk 의 canonical body shape:

```text
[^1]: `imports/quarterly-review` (imported 2026-04-18)
[^2]: `incidents/fan-boot` §Symptom
[^3]: `imports/gpu-runbook` §ECC (imported 2026-04-18)
```

Rules:

- base identity 는 항상 backticked `page_name`
- `section` 이 있으면 ` §<section>` 을 suffix 로 덧붙인다
- imported page 인 것이 확인되면 `(imported YYYY-MM-DD)` 를 뒤에 덧붙인다
- 동일 source identity 재사용 시 label 을 재사용한다
- current P2B answer body 는 `/#/wiki/...` 나 `/api/v1/blobs/...` hyperlink 를 직접 내장하지 않는다
- hyperlink enrichment 는 [P2C]의 request-evidence / hover-card layer 로 미룬다

### 2.2 내부 구조

#### 2.2.1 Retrieval -> prose -> evidence 3단 분리

1. **Retrieval**
   `wiki.search` 가 page-level hit 를 반환한다.
2. **Prose assembly**
   Penny 가 답변 본문과 footnote definition 을 작성한다.
3. **Evidence enrichment**
   parser 가 footnote body 를 추출하고, gateway/workbench 가 request-scoped provenance 와 actor-filtered evidence 를 붙인다.

이 셋을 분리하는 이유:

- model output 은 사용자에게 바로 보이는 surface 이다
- parser 는 그 output 을 deterministic 하게 읽는 최소 공통 문법이다
- request evidence 는 model output 이 아니라 shared surface / audit / activity chain 에 근거해야 한다

#### 2.2.2 Source identity levels

현재 trunk 는 source identity 를 세 단계로 다룬다.

| 수준 | 예시 | 안정성 | 현재 사용처 |
|---|---|---|---|
| L1 | `` `imports/quarterly-review` `` | 높음 | 기본 footnote identity |
| L2 | `` `incidents/fan-boot` §Symptom `` | 중간 | display hint, operator readability |
| L3 | `blob_id + page_number + chunk offset` | 가장 높음 | [P2C] request evidence / hover / deep link |

P2B answer body 는 L1/L2 까지만 책임진다. L3 는 evidence surface 가 책임진다.

#### 2.2.3 No-hit / weak-hit behavior

- `wiki.search` 결과가 비어 있으면 Penny 는 explicit no-hit disclosure 를 먼저 한다
- 결과가 있으나 관련성이 낮으면 `wiki.get` 으로 1회 추가 확인 후 답한다
- 불충분한 hit 를 citation 없이 사실처럼 말하는 것은 금지다
- relevance 가 불명확한데도 답변해야 한다면 "위키 기준으로는 확정할 수 없다" 고 적는다

#### 2.2.4 Imported RAW vs authored wiki page

- imported page 는 frontmatter / receipt 에서 imported provenance 를 확인할 수 있다
- authored wiki page 는 imported suffix 없이 page path 또는 path+section 만 노출한다
- UI 가 "원본 파일 보기" 나 blob deep link 를 제공하고 싶다면 chat markdown 이 아니라 request evidence API 를 사용한다

#### 2.2.5 PDF capability gating

현재 trunk 는 두 경로를 구분한다.

1. `KnowledgeGadgetProvider` 기본 경로
   - built-in markdown extractor 만 보장
   - `application/pdf` 는 unsupported 로 surface 될 수 있음
2. bundle-installed extractor 경로
   - `plugins/plugin-document-formats` 가 `PdfExtractor` 를 등록
   - `application/pdf` import, page-break hints, OCR confidence warning, max-pages truncation contract 활성화

따라서 operator-facing 문서와 runbook 은 "PDF import always works" 라고 쓰면 안 된다. 정확한 약속은 "PDF import 는 document-formats bundle 과 `pdf` feature 가 활성인 배포에서 지원된다" 이다.

#### 2.2.6 Parser consumption rules for Web / CLI

- initial render 는 parser 결과만으로도 가능해야 한다
- enrichment 는 optional second step 다
- renderer 는 footnote body 를 parse 했다고 해서 page 존재나 blob 접근 권한까지 자동 인정하면 안 된다
- hover card / click-through / source preview 는 actor-filtered gateway evidence 또는 `wiki.get`/future blob-view contract 로 확인한다

### 2.3 설정 스키마

이 문서는 새 top-level TOML key 를 추가하지 않는다. retrieved-answer / citation contract 는 **필수 동작** 이며 feature toggle 이 아니다.

```toml
[knowledge]
wiki_path = "./.gadgetron/wiki"
wiki_autocommit = true
wiki_max_page_bytes = 1048576

[knowledge.search]
max_results = 10
```

규칙:

- `wiki.search` 기본 `limit` 은 10 이고 prompt 도 이 값을 전제로 한다
- citation body formatting 자체는 config 로 끄지 않는다
- imported suffix 는 page provenance 존재 여부에 따라 결정되며 user config knob 가 아니다
- PDF support 는 runtime TOML 이 아니라 bundle install + Cargo feature(`pdf`) 로 결정된다
- build 가 `pdf` feature 없이 배포되면 `application/pdf` import 는 unsupported 경로를 탄다

Validation:

- `knowledge.search.max_results` 가 1 미만이면 invalid
- `wiki_max_page_bytes` 는 extraction 전후 write limit 으로 유지된다
- renderer 는 footnote body 를 config 기반 다른 포맷으로 재직렬화하지 않는다

### 2.4 에러 & 로깅

#### 2.4.1 에러 surface

현재 contract 에서 중요한 에러/경고 코드는 다음과 같다.

| Surface | 코드/형태 | 의미 |
|---|---|---|
| `wiki.search` | `knowledge_invalid_query`, `knowledge_backend_unavailable` | query invalid 또는 backend unavailable |
| import/gadget pre-dispatch | `invalid_request_error` 성격의 invalid args | bundle extractor 가 등록되지 않은 배포에서 `application/pdf` 같은 MIME 을 요청 |
| import/extractor | `extract_unsupported_content_type` | extractor 는 선택되었지만 MIME 미지원 |
| import/extractor | `extract_malformed_input` | malformed PDF/RAW bytes |
| import/extractor | `extract_timeout`, `extract_too_large` | extraction timeout/size limit |
| Penny runtime | `penny_tool_execution`, `penny_tool_invalid_args` | tool call 실패 또는 invalid args |
| workbench evidence | `403` / `404` | actor 불가 또는 request/resource 부재 |

User-facing policy:

- no-hit 는 error 가 아니라 explicit disclosure 다
- malformed PDF 는 "import failed" 로 숨기지 말고 content-type / malformed 여부를 남긴다
- unauthorized evidence/blob access 는 info leakage 를 막기 위해 existence 를 과하게 드러내지 않는다
- bundle extractor 미등록 상태의 PDF import 는 "PDF 가 고장났다" 가 아니라 "현재 배포 경로에 extractor 가 없다" 는 operator-facing 진단으로 surface 한다

#### 2.4.2 tracing / audit

- `KnowledgeService::write` -> span name `knowledge.write`
- `IngestPipeline::import` -> span name `knowledge.import`
- unreadable page during legacy wiki index build -> `target: "wiki_search"` warning
- `PdfExtractor::name()` 은 `pdf-extract-0.7` 로 고정되어 audit / source metadata 에 남는다
- parser 자체는 side-effect-free pure function 이며 audit event 를 만들지 않는다

#### 2.4.3 STRIDE threat model

| 자산 | 신뢰 경계 | 위협 | 완화 |
|---|---|---|---|
| wiki page path / section | `wiki.search` 결과 -> Penny prompt | Spoofing: model 이 없는 path 를 footnote 로 fabricate | search-first 규칙, no-hit disclosure, parser 는 footnote 추출만 하고 existence 를 보증하지 않음 |
| imported PDF bytes | extractor bundle -> ingest pipeline | Tampering: malformed PDF 가 parser/worker 를 흔듦 | size limit, timeout, `spawn_blocking`, `ExtractError::Malformed` |
| request evidence / citations | Penny markdown -> workbench renderer | Repudiation: UI 가 model markdown 만 믿고 provenance 를 과장 | parser 결과와 actor-verified evidence 를 분리, request evidence 로 재검증 |
| hidden page names | shared surface -> actor | Information Disclosure: 권한 없는 page path 누출 | actor-filtered search/evidence, unauthorized deep-link/path verification 차단 |
| extraction worker / search loop | user input bytes/query | DoS: huge PDF, degenerate query, OCR-poor scans | 50 MB limit, timeout, `max_pages`, warning-only OCR signal, capped search limit |
| evidence action path | chat/UI -> gateway | Elevation of Privilege: footnote body 로 unauthorized blob/page 접근 유도 | footnote body 자체는 authority 아님, click-through 는 gateway ACL / future blob-view ACL 통과 필요 |

Compliance mapping:

- **SOC2 CC6.1 / CC6.6**: actor-filtered search/evidence, default-deny deep-link access
- **GDPR Art 32**: provenance read path 에 least-privilege / unauthorized non-disclosure 적용
- **HIPAA §164.312**: direct medical claim 은 없지만 audit-preserving access mediation 원칙과 정합

### 2.5 의존성

- `regex`, `once_cell`
  - `gadgetron-penny::citation` parser 구현에 사용
  - 이미 trunk 에 존재, 추가 결정 필요 없음
- `pdf-extract = 0.7` (optional)
  - `plugins/plugin-document-formats` 안에만 머문다
  - D-12 leaf 원칙 유지: heavy extractor dependency 는 plugin 경계 밖으로 새지 않는다
- 새 crate 추가 없음

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 크레이트 책임 분해

| 크레이트 | 책임 |
|---|---|
| `gadgetron-knowledge` | `wiki.search` payload 생성, import pipeline, bundle extractor 경로 |
| `gadgetron-penny` | prompt contract, citation parser, returned markdown surface |
| `gadgetron-core` | `ExtractError`, `StructureHint::PageBreak`, warning taxonomy |
| `plugins/plugin-document-formats` | optional PDF extractor implementation |
| `gadgetron-gateway` | actor-filtered request evidence / future citation enrichment API |
| `gadgetron-web` | parser 결과를 표시하고 request evidence 로 enrich 하는 renderer |

### 3.2 데이터 흐름 다이어그램 (ASCII)

```text
User question
   |
   v
gadgetron-penny / PENNY_PERSONA
   | 1. wiki.search(query, limit=10)
   v
gadgetron-knowledge::call_wiki_search
   |
   +--> SearchHitPayload { page_name, section?, snippet? }
   |
   v
Penny prose assembly
   |
   +--> answer body with [^1], [^2]
   +--> footnote definitions using page_name / section / imported date
   |
   v
gadgetron-penny::citation::extract_citation_refs
   |
   +--> initial UI/CLI rendering
   |
   +--> gateway request_evidence enrichment (actor-filtered)
            |
            +--> shared provenance / audit / request correlation
```

### 3.3 타 모듈 인터페이스 계약

- `13-penny-shared-surface-loop.md`
  - this doc 은 shared surface awareness 위에 올라가는 retrieved-answer/citation wire contract 를 정의한다
- `gateway/workbench-projection-and-actions.md`
  - `request_evidence` 는 parser-extracted citation 을 richer provenance 로 승격하는 자리다
- `web/expert-knowledge-workbench.md`
  - evidence pane 는 raw markdown parsing 만으로 끝나지 않고 request-scoped evidence 를 붙여야 한다
- `11-raw-ingestion-and-rag.md`
  - import / chunk / dedup foundation 은 11번이 소유한다
  - retrieved-answer current surface 와 PDF page-break exact contract 는 본 문서가 더 구체적이다

### 3.4 D-12 크레이트 경계표 준수 여부

준수한다. 새 type placement 변경은 없다.

- `CitationRef` 는 `gadgetron-penny` 에 남는다
- `StructureHint::PageBreak` 는 `gadgetron-core` 에 남는다
- `PdfExtractor` 는 plugin leaf 에 남는다
- `SearchHitPayload` 는 `gadgetron-knowledge` compatibility shim 으로 유지된다

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

| 대상 | 검증할 invariant |
|---|---|
| `citation::extract_citation_refs` | line-start definitions 만 추출, source order 유지, named label 허용 |
| `citation::extract_referenced_labels` | inline refs 만 추출, definition line 제외 |
| `spawn.rs` prompt regression tests | search-first, fabrication 금지, footnote format 고정 |
| `PdfExtractor` | per-page extract, `PAGE_SEPARATOR`, `PageBreak`, metadata shape, max_pages, malformed bytes |
| `search_hit_payload` | `page_name` verbatim 보존, `section` best-effort mapping, empty snippet filtering |

### 4.2 테스트 하네스

- `citation.rs` 는 pure function unit tests 로 충분
- `spawn.rs` prompt tests 는 `include_str!` witness test 또는 constant string containment test 사용
- `PdfExtractor` 는 hand-crafted PDF byte fixtures 사용
- parser code-fence caveat 는 explicit witness test 로 유지한다
- property-based test 는 이번 seam 에 필수는 아니지만, label alphabet / duplicate ordering invariant 가 늘어나면 [P2C]에 도입한다

### 4.3 커버리지 목표

- `gadgetron-penny::citation` line >= 90%, branch >= 85%
- `plugins/plugin-document-formats::pdf` line >= 85%, branch >= 80%
- prompt regression (`spawn.rs` RAG extension lines) line >= 80%
- `search_hit_payload` compatibility branches 100% (branch 수가 작다)

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

- `gadgetron-knowledge` + `gadgetron-penny`
  - markdown import -> `wiki.search` -> footnote parser round-trip
- `plugins/plugin-document-formats`
  - PDF extractor output shape -> pipeline-ready page break hints
- `gadgetron-gateway` + `gadgetron-web` [P2C]
  - parsed citations -> request evidence enrichment -> actor-filtered source preview

핵심 e2e 시나리오:

1. markdown import 후 Penny-style cited response 를 조립하면 `page_name` 이 footnote body 로 verbatim round-trip 된다
2. 두 페이지를 인용한 응답에서 `[^1]`, `[^2]` 순서와 path 가 보존된다
3. PDF extractor 는 page count / warnings / page break hints 를 pipeline 이 소비 가능한 shape 로 돌려준다
4. PDF feature 가 빠진 배포에서 `application/pdf` import 는 unsupported 로 surface 된다
5. [P2C] request evidence 는 parser 결과와 shared provenance 를 같은 actor scope 로 정렬한다

### 5.2 테스트 환경

- current headless tests:
  - `tempfile::TempDir`
  - in-memory / keyword-only knowledge service
  - Postgres 필수 아님
- plugin PDF tests:
  - `cargo test -p plugin-document-formats --all-features`
  - no external OCR service
- future gateway/web evidence tests:
  - axum test server + mocked request evidence store
  - actor contexts 2종 (authorized / unauthorized)

### 5.3 회귀 방지

다음 변경은 반드시 테스트를 깨야 한다.

- Penny prompt 에서 `wiki.search` first discipline 이 빠지는 변경
- footnote body 가 backticked `page_name` 을 보존하지 않는 변경
- parser 가 source order 를 잃거나 inline ref / definition 을 혼동하는 변경
- PDF extractor 가 `PageBreak` 를 emit 하지 않는 변경
- unsupported PDF deployment 가 조용히 plain markdown path 로 fallback 하는 변경
- request evidence layer 가 parser 없이 독자 문법을 도입하는 변경

## 6. Phase 구분

- [P2B]
  - search-first prompt
  - plain markdown footnote body
  - shared parser (`citation.rs`)
  - PDF page-break anchor contract
  - explicit no-hit / no-fabrication rule
- [P2C]
  - request-evidence enrichment
  - verified blob/page/page-number deep links
  - chunk-level stable anchor promotion
  - optional parser pre-strip for fenced code blocks
- [P3]
  - richer citation renderer (hover cards, inline provenance chips, export transforms)
  - alternative knowledge backends with anchor parity guarantees

## 7. 오픈 이슈 / 의사결정 필요

| ID | 내용 | 옵션 | 추천 | 상태 |
|---|---|---|---|---|
| Q-1 | chunk-level stable anchor 를 언제 citation body 에 직접 승격할지 | A. 계속 page path only / B. request evidence 에만 노출 / C. footnote body 에 chunk id 직접 표기 | B | 🟢 [P2C] defer |
| Q-2 | code-fence aware parser 를 P2C 에 도입할지 | A. 현 parser 유지 / B. pre-strip helper 추가 / C. full markdown AST parser 도입 | B | 🟢 [P2C] defer |
| Q-3 | imported RAW footnote 에 원본 filename/page 를 언제 직접 노출할지 | A. chat markdown 그대로 / B. request evidence hover 로만 / C. 둘 다 | B | 🟢 [P2C] defer |

---

## 리뷰 로그 (append-only)

### Round 0 — 2026-04-18 — PM draft
**결론**: Draft v0. W3-KL-3 이후 retrieved-answer / citation / evidence seam 문서 초안.

**체크리스트**:
- [x] 5대 필수 섹션 존재
- [x] current trunk drift (`11 §8.3`, `11 §9.3`) 보정 범위 명시
- [ ] round별 reviewer feedback 미반영

**Action Items**:
- A1: current footnote body 가 hyperlink 가 아니라 plain markdown 임을 더 명시
- A2: parser 결과와 actor-verified evidence 의 경계를 본문에 고정

**다음 라운드 조건**: A1, A2 반영 후 Round 1 진행

### Round 1 — 2026-04-18 — @gateway-router-lead @ux-interface-lead
**결론**: Conditional Pass

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
- A1: current P2B footnote body 에 hyperlink 를 약속하지 않음을 본문과 examples 양쪽에 명시
- A2: EvidencePane / request_evidence 가 parser 결과를 enrich 하는 구조를 flow diagram 과 rules 에 모두 반영

**다음 라운드 조건**: A1, A2 반영 후 Round 1.5 진행

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
- [x] API 응답 shape
- [x] config 필드
- [x] defaults 안전성
- [x] runbook playbook
- [x] 하위 호환
- [x] i18n 준비

**Action Items**:
- A1: unauthorized evidence/blob access 를 existence-leaking success path 처럼 설명하지 않도록 error surface 문구 보정
- A2: PDF capability gating 을 operator-facing 계약으로 명시해 "항상 PDF import 가능" 오해를 차단

**다음 라운드 조건**: A1, A2 반영 후 Round 2 진행

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
- A1: PDF feature-missing unsupported case 를 integration plan 에 추가
- A2: parser code-fence caveat witness test 를 unit test plan 에 명시

**다음 라운드 조건**: A1, A2 반영 후 Round 3 진행

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
- A1: 새 core type 을 만들지 않고 existing crate boundaries 위에 contract 만 고정한다는 점을 D-12 섹션에 명시
- A2: parser 가 pure function 이며 side-effect / audit 생성자가 아님을 로깅 섹션에 명시

**다음 라운드 조건**: 없음

### 최종 승인 — 2026-04-18 — PM
**결론**: Approved. W3-KL-3 이후 current trunk 의 retrieved-answer / citation / parser / evidence seam 이 문서로 닫혔다. Round 1 / 1.5 / 2 / 3 액션 아이템 반영 완료.
