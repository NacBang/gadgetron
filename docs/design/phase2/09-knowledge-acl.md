# 09 — Knowledge ACL: Scope, Read/Write Rules, Search Filtering (D3 / D4 / D6)

> **담당**: PM (Claude) — Round 1.5 리뷰 예정 (`@security-compliance-lead`, `@dx-product-lead`)
> **상태**: Draft v0 (2026-04-18)
> **Parent**: `docs/adr/ADR-P2A-08-multi-user-foundation.md`, `docs/process/04-decision-log.md` D-20260418-02
> **Sibling**: [`08-identity-and-users.md`](08-identity-and-users.md) (전제), [`10-penny-permission-inheritance.md`](10-penny-permission-inheritance.md) (이 ACL 을 소비)
> **Parent (semantic infra)**: `docs/adr/ADR-P2A-07-semantic-wiki-pgvector.md`, `docs/design/phase2/05-knowledge-semantic.md`
> **Drives**: P2B 구현 — wiki 페이지 visibility, read/write rule, 하이브리드 검색 ACL pre-filter
> **관련 크레이트**: `gadgetron-knowledge` (wiki + 검색), `gadgetron-core` (scope 타입), `gadgetron-xaas` (TeamCache)
> **Phase**: [P2B]
>
> **Canonical terminology note**: historical references in this doc to `plugin` mainly refer to seed/frontmatter compatibility fields and legacy bundle working identifiers. Product terminology remains Bundle / Plug / Gadget.

---

## Table of Contents

1. 철학 & 컨셉
2. 용어 & 전제
3. 스키마 — `wiki_pages` 확장
4. Scope 타입 및 frontmatter 시맨틱
5. Access check — read / write 함수
6. Penny 의 `wiki.write` 시맨틱
7. Plugin seed 페이지 + locked 룰
8. 하이브리드 검색 ACL pre-filter (SQL)
9. Team cache (moka)
10. UI/CLI 동선
11. 마이그레이션 — 기존 페이지 → scope 할당
12. Phase 분해
13. 오픈 이슈
14. Out of scope
15. 리뷰 로그

---

## 1. 철학 & 컨셉

### 1.1 해결하는 문제

지식층 (wiki) 은 다수의 user 가 **공유** 하면서도 **개인 영역** 을 유지하고, **팀 기밀** 을 분리할 수 있어야 한다. 동시에:

- Penny 가 검색할 때 **권한 밖 정보가 context 에 섞이지 않아야** 한다 (T3 정보 누설 방지)
- Plugin seed 페이지는 user 가 실수로 덮어쓰지 않되, 의도적 커스터마이즈는 허용되어야 한다
- 감사·롤백이 git 히스토리 + audit_log 로 자연 추적돼야 한다

### 1.2 D-20260418-02 와의 매핑

| 결정 | 이 문서가 구현 |
|:---:|---|
| D3 | §4 scope enum, frontmatter 스키마, `admins` virtual team |
| D4 | §5 access check 함수 (read = scope, write = scope-member + admin), `locked = true` 예외 |
| D6 | §8 하이브리드 검색 pre-filter SQL, §9 TeamCache |

### 1.3 핵심 설계 원칙

1. **Frontmatter 가 권위** — 경로·이름은 UX. `scope`, `owner_user_id`, `locked` 세 필드가 ACL 의 truth
2. **3-level 단순** — `private` / `team:<id>` / `org`. Unix-style rwx 는 P3+
3. **위키 협업 친화** — 같은 scope 멤버는 모두 편집 가능 (org = 오픈 편집). `locked` 는 예외로만
4. **정보 누설 금지** — 접근 불가 페이지의 **존재 자체** 를 노출하지 않음. 검색 결과에 "제한됨" 표시 금지
5. **Plugin 오너십 ⊥ 스코프** — `plugin = "<name>"` 은 lifecycle 용, `scope` 는 visibility 용. 두 필드 혼동 금지

### 1.4 고려한 대안 / 기각 사유

| 대안 | 기각 사유 |
|---|---|
| Unix-style per-page rwx (owner/group/others, r/w/x) | UI 과중. 위키식 협업 저하. P3+ |
| Scope = `{plugin:<name>, ...}` 4-level | `plugin` 은 lifecycle 필드라 visibility 와 직교. 혼동 위험 |
| Post-filter 검색 (top-K 후 ACL 제거) | accessible 결과 손실 위험. D6-a 에서 반려 |
| Per-user materialized accessible_ids cache | 규모 커지면 유용하나 P2B YAGNI. P2C 옵션 |
| `write_scope` 별도 필드 | D4-a 의 "scope member + admin" 단순 규칙으로 충분. `locked` 하나로 예외 처리 |

---

## 2. 용어 & 전제

### 2.1 용어

| 용어 | 의미 |
|---|---|
| **Scope** | 페이지의 read visibility. `"private"` / `"team:<id>"` / `"org"` |
| **Owner** | 페이지를 처음 생성한 user. `owner_user_id` 로 기록 |
| **Locked** | `true` 이면 write 가 owner + admin only (scope member 편집 불가) |
| **Source** | 페이지 생성 출처. `"user"` / `"conversation"` / `"plugin_seed"` / `"reindex"` |
| **Plugin ownership** | `plugin = "<name>"` frontmatter. Plugin disable 시 archive 필터. scope 와 직교 |
| **Source modified by** | Plugin seed 페이지를 user 가 수정한 경우 user id 기록. plugin 버전업 시 자동 덮어쓰기 방지 |

### 2.2 전제 (다른 문서에서 이미 확정)

- User/team/role — 08 doc 에서 확정. `User`, `Team`, `Role` 타입 존재
- Semantic search 기반 — ADR-P2A-07, 05 doc. pgvector + ts_rank 하이브리드
- Audit schema 확장 — 08 doc §3.5 (actor_user_id 등)
- Penny caller 상속 — 10 doc 에서 `AuthenticatedContext` 정의, wiki tool 이 이를 소비

---

## 3. 스키마 — `wiki_pages` 확장

### 3.1 기존 스키마 (05 doc 기준 가정)

```sql
-- ADR-P2A-07 / 05 doc 에서 이미 정의된 것:
-- CREATE TABLE wiki_pages (
--     id              UUID PRIMARY KEY,
--     tenant_id       UUID NOT NULL REFERENCES tenants(id),
--     path            TEXT NOT NULL,
--     title           TEXT,
--     content         TEXT NOT NULL,
--     frontmatter     JSONB NOT NULL DEFAULT '{}'::jsonb,
--     embedding       vector(1536),
--     tsv             tsvector,
--     git_sha         TEXT,
--     created_at      TIMESTAMPTZ,
--     updated_at      TIMESTAMPTZ,
--     UNIQUE (tenant_id, path)
-- );
```

### 3.2 P2B 추가 컬럼 (08 doc §3.5 에 선언, 여기서 상세)

```sql
ALTER TABLE wiki_pages ADD COLUMN scope TEXT NOT NULL DEFAULT 'private';
ALTER TABLE wiki_pages ADD COLUMN owner_user_id UUID REFERENCES users(id);
ALTER TABLE wiki_pages ADD COLUMN locked BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE wiki_pages ADD COLUMN source TEXT NOT NULL DEFAULT 'user';
ALTER TABLE wiki_pages ADD COLUMN source_modified_by UUID REFERENCES users(id);
ALTER TABLE wiki_pages ADD COLUMN plugin TEXT;     -- lifecycle 오너십, 06 doc §4

ALTER TABLE wiki_pages ADD CONSTRAINT scope_format CHECK (
    scope = 'private' OR
    scope = 'org' OR
    scope ~ '^team:[a-z][a-z0-9-]{0,31}$'
);

ALTER TABLE wiki_pages ADD CONSTRAINT source_values CHECK (
    source IN ('user', 'conversation', 'plugin_seed', 'reindex')
);

CREATE INDEX idx_wiki_scope_tenant ON wiki_pages (tenant_id, scope);
CREATE INDEX idx_wiki_owner        ON wiki_pages (owner_user_id);
CREATE INDEX idx_wiki_plugin       ON wiki_pages (plugin) WHERE plugin IS NOT NULL;
```

### 3.3 기존 `wiki_chunks` 는 건드리지 않음

청크 테이블은 `page_id` FK 만 있으면 ACL 은 `wiki_pages` 레벨에서 결정 → 검색 쿼리가 join 으로 해결 (§8).

---

## 4. Scope 타입 및 frontmatter 시맨틱

### 4.1 Rust 타입

```rust
// gadgetron-core::knowledge::scope
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Private,
    Team(TeamId),           // TeamId 는 String wrapper (kebab-case)
    Org,
}

impl Scope {
    pub fn parse(s: &str) -> Result<Self, ScopeParseError> {
        match s {
            "private" => Ok(Self::Private),
            "org"     => Ok(Self::Org),
            s if s.starts_with("team:") => {
                let team_id = &s[5..];
                TeamId::parse(team_id).map(Self::Team)
            }
            _ => Err(ScopeParseError::Unknown(s.into())),
        }
    }

    pub fn serialize(&self) -> String {
        match self {
            Self::Private       => "private".into(),
            Self::Org           => "org".into(),
            Self::Team(team)    => format!("team:{}", team.as_str()),
        }
    }
}
```

### 4.2 Frontmatter 예시

**Private 개인 노트**

```markdown
---
scope = "private"
owner_user_id = "550e8400-..."
type = "note"
tags = ["learning"]
---

# Rust borrow checker 정리
```

**팀 런북**

```markdown
---
scope = "team:platform"
owner_user_id = "550e8400-..."
type = "runbook"
tags = ["oncall"]
locked = false                        # 팀원 누구나 편집 가능
---

# 배포 롤백 절차
```

**공용 지식 (기본 협업)**

```markdown
---
scope = "org"
owner_user_id = "550e8400-..."
type = "decision"
tags = ["architecture"]
---

# 플러그인 구조 결정 (D-20260418-01 요약)
```

**Plugin seed 페이지 (locked)**

```markdown
---
scope = "org"
source = "plugin_seed"
plugin = "server"
plugin_version = "0.1.0"
locked = true                         # user 실수로 덮어쓰지 않도록
type = "runbook"
tags = ["ai-infra", "disk"]
---

# 디스크 가득 차면 이렇게 하세요
```

**Admin-only 정책 문서** (built-in `admins` team 사용)

```markdown
---
scope = "team:admins"
owner_user_id = "550e8400-..."
type = "policy"
locked = true
---

# Gadgetron 보안 운영 정책
```

### 4.3 Parsing 규칙

- frontmatter 의 `scope` 필드 누락 → 기본 `"private"` (보수적)
- `owner_user_id` 누락 → 페이지 생성 user 의 id 자동 주입
- `locked` 누락 → `false`
- `source` 누락 → `"user"`
- 알 수 없는 필드는 warn 하되 reject 하지 않음 (ADR-P2A-07 "convention-over-schema" 원칙)

---

## 5. Access check — read / write 함수

### 5.1 핵심 함수

```rust
// gadgetron-knowledge::acl
pub struct AclContext<'a> {
    pub user:  &'a User,
    pub teams: &'a TeamSet,        // moka cache 로부터 주입
}

pub fn can_read(ctx: &AclContext, page: &WikiPage) -> bool {
    // Admin 은 모든 wiki read 가능
    if ctx.user.role == Role::Admin { return true; }

    match &page.scope {
        Scope::Private => page.owner_user_id == Some(ctx.user.id),
        Scope::Team(team_id) if team_id.as_str() == "admins" => {
            ctx.user.role == Role::Admin   // Admin 이 아니면 아예 거부
        }
        Scope::Team(team_id) => ctx.teams.contains(team_id),
        Scope::Org => true,                 // tenant 내 모든 user
    }
}

pub fn can_write(ctx: &AclContext, page: &WikiPage) -> bool {
    if ctx.user.role == Role::Admin { return true; }

    if page.locked {
        // locked 이면 owner 만 write (admin 은 위 조건에서 이미 통과)
        return page.owner_user_id == Some(ctx.user.id);
    }

    match &page.scope {
        Scope::Private => page.owner_user_id == Some(ctx.user.id),
        Scope::Team(team_id) if team_id.as_str() == "admins" => false,  // admin 만
        Scope::Team(team_id) => ctx.teams.contains(team_id),
        Scope::Org => true,     // 위키식 오픈 편집
    }
}
```

### 5.2 Test invariants (Round 2 QA 입력)

- **Private page visibility**: user A private → user B read = false, write = false. user B admin → read = true
- **Team page visibility**: team:platform page → platform 멤버만 read/write. admin 은 bypass
- **Org page editing**: 모든 user write 가능 (admin/member 공통). service user 도 write 가능하지만 T2 tool 이므로 approval gate 통과 필요
- **Locked org page**: owner + admin 만 write. 일반 member 는 locked=true 시 scope 가 org 여도 write 거부
- **`team:admins`**: admin 만 read/write. member 는 team_members 테이블에 불가
- **Role change 즉시 반영**: user role 을 admin 으로 promote → TeamCache invalidate → 다음 요청부터 admin 경로
- **자기 페이지 거부 규칙**: admin 이 아닌 이상 다른 user 의 private page 에 `can_read=false`

### 5.3 Path-based 방어 (defense in depth)

ACL 은 **DB 레벨** 에서 권위이지만, 파일시스템 저장 위치도 관습적 guard:

- `wiki/private/<user_id>/...` 은 `scope = "private"` 만 허용
- `wiki/team/<team_id>/...` 은 `scope = "team:<team_id>"` 만 허용
- `wiki/org/...` 은 `scope = "org"` 만 허용 (plugin seed 포함)
- Frontmatter scope 와 경로가 어긋나면 wiki.write 에서 **reject** (운영자 실수 방지)

단, **권위는 frontmatter**. 마이그레이션·이름변경으로 경로가 변해도 frontmatter 가 같으면 ACL 은 변하지 않음.

---

## 6. Penny 의 `wiki.write` 시맨틱

### 6.1 Tool 시그니처 (09 doc, 10 doc 와 공유)

```rust
pub struct WikiWriteArgs {
    pub path:        String,
    pub content:     String,          // 전체 본문 (frontmatter 포함 또는 자동 주입)
    pub scope:       Option<Scope>,   // 명시 시 존중, 미명시 시 §6.2 규칙
    pub locked:      Option<bool>,
}

pub struct WikiWriteResult {
    pub page_id:      PageId,
    pub path:         String,
    pub scope:        Scope,
    pub git_sha:      String,
    pub conflict:     Option<ConflictInfo>,  // 동일 path 의 기존 페이지가 있을 때
}
```

### 6.2 Penny 가 scope 를 결정하는 규칙

Penny 는 LLM 이라 "어떤 scope 로 저장할지" 를 판단해야 함. 시스템 프롬프트에 들어갈 정책:

1. **Caller user 가 명시적 지시** → 그대로 (`"이거 개인 노트에 저장해줘"` → private)
2. 인시던트 기록·운영 결정 → 기본 **`org`** (공유 가치 큼)
3. 개인 대화 요약·scratchpad → 기본 **`private`**
4. 팀 런북·meeting note → 컨텍스트상 팀이 명확하면 `team:<id>`, 불명확하면 caller 에게 질문

### 6.3 Owner 및 impersonated_by

- `owner_user_id = caller.user_id` (Penny 가 caller 를 **대리**)
- DB 레벨 write 는 `impersonated_by = 'penny'`, `parent_request_id = caller.request_id` 로 audit
- Git 커밋 메시지: `"wiki: [penny:<user_email>@<scope>] <path>"` — Penny 행위임을 명시
- 이 규칙은 07/10 doc 에서 반복 정의

### 6.4 Write 실패 케이스

- `can_write(caller, existing_page) == false` → 401 + error_code `permission_denied`
- 경로 conflict (동일 path, 다른 owner) 에 locked=true → reject + suggest `--overwrite-as-owner` flag (admin 만)
- `scope = "team:<id>"` 지정이지만 caller 가 해당 team 비멤버 → reject

### 6.5 Penny 가 바로 하지 못하는 동작 (`ADMIN_ONLY_TOOLS` 재강조)

- 다른 user 의 private 페이지 편집 (admin 만)
- Plugin seed 페이지의 `locked = false` 변환 (plugin 소유, admin 만 unlock 가능)
- `scope` 를 `"team:admins"` 로 설정 (admin 만)

Penny 가 시도하면 tool 호출 단계에서 거부 + `PermissionDenied` 결과 반환 → Penny 가 사용자에게 "admin 권한이 필요합니다" 알림.

---

## 7. Plugin seed 페이지 + locked 룰

### 7.1 Plugin seed 생성 플로우

```rust
// 06 doc §4 의 SeedPage 확장
pub struct SeedPage {
    pub path:         String,
    pub content:      String,            // frontmatter 포함
    pub overwrite:    bool,
    pub scope:        Option<Scope>,     // 미지정 시 "org"
    pub locked:       Option<bool>,      // 미지정 시 true (plugin seed 기본)
}
```

Plugin enable 첫 회:
1. SeedPage 컬렉션 수신
2. 각 페이지에 대해 기존 존재 확인:
   - 없음 → 신규 insert. `source = "plugin_seed"`, `plugin = "<name>"`, `plugin_version = <ver>`, `locked = true`
   - 존재 + `source_modified_by IS NULL` → plugin_version 비교 후 갱신 가능
   - 존재 + `source_modified_by IS NOT NULL` → **덮어쓰기 금지**. audit 로그에 `seed_skipped` 기록 + admin 에게 diff 알림
3. 각 페이지 git commit

### 7.2 User 가 seed 페이지 편집하려는 경우

1. Web UI 편집 버튼 클릭 → `locked = true` 때문에 "편집 불가" 메시지
2. User 가 "Unlock for editing" 버튼 (admin 또는 plugin scope 소유자만 노출)
3. Unlock 시 DB 업데이트:
   ```sql
   UPDATE wiki_pages
   SET locked = false,
       source_modified_by = $user_id
   WHERE id = $page_id;
   ```
4. 이후 user 편집은 자유롭게, 단 plugin 버전업 시 자동 덮어쓰기 **중단** (§7.1 3번 규칙)
5. Admin 이 "Reset to plugin default" 옵션으로 원복 가능 (audit 에 기록)

### 7.3 Lock 변경의 감사

모든 `locked` 토글은 audit 로그에 명시적 이벤트:

```json
{
  "event": "wiki_page_lock_changed",
  "page_id": "...",
  "old_locked": true,
  "new_locked": false,
  "old_source_modified_by": null,
  "new_source_modified_by": "550e8400-...",
  "actor_user_id": "550e8400-...",
  "timestamp": "2026-04-18T10:24:00Z"
}
```

---

## 8. 하이브리드 검색 ACL pre-filter

### 8.1 기본 쿼리 (D6-a 확정형)

```sql
-- Prepared statement parameters:
-- $1 = user_id (UUID)
-- $2 = tenant_id (UUID, P2B 에서는 'default' UUID)
-- $3 = query_embedding (vector(1536))
-- $4 = keyword_query (text)
-- $5 = result_limit (int, default 20)

WITH user_teams AS (
    SELECT team_id
    FROM team_members
    WHERE user_id = $1
),
user_role AS (
    SELECT role
    FROM users
    WHERE id = $1
),
accessible_pages AS (
    SELECT p.id, p.path, p.title, p.content, p.embedding, p.tsv,
           p.scope, p.updated_at
    FROM wiki_pages p
    CROSS JOIN user_role ur
    WHERE p.tenant_id = $2
      AND (
            ur.role = 'admin'
         OR p.scope = 'org'
         OR (p.scope = 'private' AND p.owner_user_id = $1)
         OR (p.scope = 'team:admins' AND ur.role = 'admin')
         OR (p.scope LIKE 'team:%'
             AND p.scope != 'team:admins'
             AND SUBSTRING(p.scope FROM 6) IN (SELECT team_id FROM user_teams))
      )
),
semantic AS (
    SELECT id, path, title, content,
           1.0 - (embedding <=> $3::vector) AS sem_score
    FROM accessible_pages
    WHERE embedding IS NOT NULL
    ORDER BY embedding <=> $3::vector
    LIMIT $5 * 3                      -- oversampling for RRF
),
keyword AS (
    SELECT id, path, title, content,
           ts_rank_cd(tsv, plainto_tsquery('simple', $4)) AS kw_score
    FROM accessible_pages
    WHERE tsv @@ plainto_tsquery('simple', $4)
    ORDER BY kw_score DESC
    LIMIT $5 * 3
),
fused AS (
    -- Reciprocal Rank Fusion
    SELECT
        COALESCE(s.id, k.id) AS id,
        COALESCE(s.path, k.path) AS path,
        COALESCE(s.title, k.title) AS title,
        COALESCE(s.content, k.content) AS content,
        COALESCE(s.sem_score, 0.0) * 0.6 + COALESCE(k.kw_score, 0.0) * 0.4 AS score
    FROM semantic s
    FULL OUTER JOIN keyword k ON s.id = k.id
)
SELECT id, path, title, content, score
FROM fused
ORDER BY score DESC
LIMIT $5;
```

### 8.2 주요 포인트

- `accessible_pages` CTE 가 **먼저** ACL 필터. 이후 semantic/keyword 는 pre-filtered 집합 위에서 작동
- `ur.role = 'admin'` 이 한 줄 bypass — admin 전권
- `team:admins` 는 별도 분기 (team_members 에 없는 virtual team)
- oversampling factor `$5 * 3` 으로 RRF fusion 시 후보 충분히 확보
- 최종 LIMIT 는 caller 지정 (기본 20)

### 8.3 성능 가이드

| 조건 | 예상 P95 latency | 인덱스 |
|---|---|---|
| 코퍼스 < 10K 페이지, accessible > 80% | < 50ms | HNSW + GIN tsvector + btree(scope) |
| 코퍼스 100K, accessible ~10% | < 200ms | 동일 + idx_wiki_scope_tenant |
| 코퍼스 1M+ | P2C 시 C 옵션 (materialized accessible_ids) 로 승격 |

### 8.4 Rust 바인딩

```rust
pub async fn hybrid_search(
    db: &Pool<Postgres>,
    embedder: &dyn EmbeddingProvider,
    user_id: UserId,
    tenant_id: TenantId,
    query: &str,
    limit: u32,
) -> Result<Vec<SearchHit>, KnowledgeError> {
    let embedding = embedder.embed(query).await?;

    sqlx::query_as!(
        SearchHit,
        r#"
        WITH user_teams AS (...),
        ...
        SELECT id as "id!: Uuid", path as "path!", title, content, score as "score!"
        FROM fused ORDER BY score DESC LIMIT $5
        "#,
        user_id.0,
        tenant_id.0,
        &embedding.as_slice() as &[f32],
        query,
        limit as i64,
    )
    .fetch_all(db)
    .await
    .map_err(Into::into)
}
```

### 8.5 MCP tool 통합 (`wiki.search`)

```rust
impl McpToolProvider for WikiToolProvider {
    async fn wiki_search(
        &self,
        args: WikiSearchArgs,
        ctx: &AuthenticatedContext,      // 10 doc 에서 정의
    ) -> Result<WikiSearchResult> {
        let hits = hybrid_search(
            &self.db,
            &*self.embedder,
            ctx.user_id,
            ctx.tenant_id,
            &args.query,
            args.limit.unwrap_or(20),
        ).await?;

        // 결과에 "제한됨" 같은 메타 삽입 금지 (info leakage)
        Ok(WikiSearchResult { hits })
    }
}
```

---

## 9. Team cache (moka)

### 9.1 왜 cache

하이브리드 검색 쿼리가 모든 요청마다 `team_members` lookup 을 수행 — user 당 2–20 row. 검색 빈도가 높으면 DB round-trip 누적. moka LRU 로 per-user team set 캐시.

### 9.2 설계

```rust
// gadgetron-xaas::cache::team_cache
use moka::future::Cache;
use std::sync::Arc;
use std::time::Duration;

pub struct TeamCache {
    inner: Cache<UserId, Arc<TeamSet>>,
}

impl TeamCache {
    pub fn new(capacity: u64) -> Self {
        Self {
            inner: Cache::builder()
                .max_capacity(capacity)
                .time_to_live(Duration::from_secs(60))    // 1분 TTL
                .build(),
        }
    }

    pub async fn get(&self, db: &Pool<Postgres>, user_id: UserId) -> Result<Arc<TeamSet>> {
        if let Some(cached) = self.inner.get(&user_id).await {
            return Ok(cached);
        }

        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT team_id FROM team_members WHERE user_id = $1"
        )
        .bind(user_id.0)
        .fetch_all(db)
        .await?;

        let set = Arc::new(TeamSet::from(rows));
        self.inner.insert(user_id, set.clone()).await;
        Ok(set)
    }

    pub async fn invalidate(&self, user_id: UserId) {
        self.inner.invalidate(&user_id).await;
    }

    pub async fn invalidate_team(&self, team_id: &str, db: &Pool<Postgres>) {
        // team 삭제/변경 시 그 team 모든 멤버 invalidate
        let members: Vec<UserId> = sqlx::query_scalar(
            "SELECT user_id FROM team_members WHERE team_id = $1"
        )
        .bind(team_id)
        .fetch_all(db)
        .await
        .unwrap_or_default();

        for uid in members {
            self.inner.invalidate(&uid).await;
        }
    }
}
```

### 9.3 Invalidation 호출 지점

| 이벤트 | Invalidation |
|---|---|
| `POST /api/v1/teams/{id}/members` | `invalidate(new_member_id)` |
| `DELETE /api/v1/teams/{id}/members/{user_id}` | `invalidate(removed_user_id)` |
| `DELETE /api/v1/teams/{id}` | `invalidate_team(team_id, db)` |
| `PATCH /api/v1/users/{id}/role` (admin 승격) | `invalidate(user_id)` — `admins` virtual team 반영 |

### 9.4 Stale window 수용

- TTL 1분 + API write 경로의 즉시 invalidate 조합
- 이벤트 누락으로 stale 가능 최대 1분 → 해당 구간에서 과거 멤버가 팀 페이지 읽기 가능
- 감사 로그에는 **실시간 권한 스냅샷** 이 아니라 행위 발생 시점의 상태 기록 → 정확한 사후 추적 가능
- 보안 민감 시나리오 (해고 등) → admin 이 `gadgetron session revoke --user <id>` 명령으로 session 강제 종료 + cache 전량 invalidate (P3 기능)

---

## 10. UI/CLI 동선

### 10.1 Web UI — 페이지 편집 화면

```
┌───────────────────────────────────────────────┐
│ 배포 롤백 절차                            [⋮] │
├───────────────────────────────────────────────┤
│ Scope: [ ▾ Team: Platform       ] [ 🔒 Lock ]│
│ Owner: Alice Kim                              │
│ Source: user  ·  Last edited 2h ago by Bob   │
├───────────────────────────────────────────────┤
│                                               │
│ # 배포 롤백 절차                              │
│                                               │
│ 1. Gateway rollback 버튼 ...                  │
│                                               │
└───────────────────────────────────────────────┘
```

- **Scope 드롭다운**: `Private` / `Team: <내가 속한 팀들>` / `Org` — caller 가 설정 가능한 옵션만 표시
- **Lock 토글**: owner 또는 admin 에게만 노출
- **Owner 변경**: admin 만 가능 (별 메뉴)
- **scope 승격 플로우**: Private → Team 변경 시 "이 페이지를 platform 팀 전체에 공유하시겠어요?" 컨펌 모달

### 10.2 Web UI — 검색 결과

- accessible 결과만 표시
- 각 hit 에 scope badge (🔒 Private / 👥 Platform / 🌐 Org) + owner 이름
- "제한됨" 배지 없음 (info leakage 방지)
- admin 이 검색 시 자기 private 만 아닌 전체에서 매치 — badge 로 차이 인식

### 10.3 CLI

```sh
# 페이지 생성 (scope 기본 private)
gadgetron wiki create my-note.md --scope private

# Scope 변경
gadgetron wiki share servers/srv-7/index.md --team platform
gadgetron wiki share servers/srv-7/index.md --org
gadgetron wiki unshare servers/srv-7/index.md      # → private

# Lock 토글 (owner / admin 만)
gadgetron wiki lock   infra/server/runbooks/disk-full.md
gadgetron wiki unlock infra/server/runbooks/disk-full.md

# Plugin seed 원복 (admin)
gadgetron wiki reset-seed infra/server/runbooks/disk-full.md

# 검색 (caller user 권한으로)
gadgetron wiki search "디스크 가득 찼을 때"
```

### 10.4 Admin 콘솔

- `/admin/wiki/overview`: tenant 전체 페이지 수 + scope 별 분포 + source 별 분포
- `/admin/wiki/audit`: 최근 lock 변경 / scope 변경 / plugin seed 덮어쓰기 이벤트

---

## 11. 마이그레이션 — 기존 페이지 → scope 할당

### 11.1 마이그레이션 가정

08 doc §10 의 `gadgetron migrate v2b-multiuser` 가 실행되며 내부적으로 wiki scope 도 처리.

### 11.2 기본 규칙 (P2A → P2B)

P2A 는 단일 운영자. 모든 기존 wiki 페이지는:

```sql
UPDATE wiki_pages
SET scope = 'org',
    owner_user_id = :bootstrap_admin_id,
    locked = FALSE,
    source = COALESCE(frontmatter->>'source', 'user')::text
WHERE scope IS DISTINCT FROM 'org';  -- idempotent
```

- 이유: 기존 단일 운영자의 노트 = 사실상 조직 공유 지식. member 승격 후에도 계속 접근 가능해야 함
- owner = bootstrap admin (나중에 개별 전환은 admin 이 수동)

### 11.3 예외: frontmatter 에 이미 `scope` 가 있는 페이지

- plugin seed 페이지 처럼 frontmatter 에 `scope = "org"` + `source = "plugin_seed"` + `plugin = "..."` 가 이미 들어 있는 경우 유지
- frontmatter → DB 컬럼 우선 순위: **DB 가 권위** (마이그레이션 후). 단 마이그레이션 초기엔 frontmatter 읽어 DB 주입 → 이후 DB 가 truth

### 11.4 `git commit` 포함

- scope 할당은 frontmatter 수정 = 파일 변경 = git commit 필요
- 마이그레이션 스크립트가 `wiki: migrate to multi-user scope (default org)` 커밋 생성
- 대량 커밋 피하려면 batch 커밋 (예: 50 페이지씩)

---

## 12. Phase 분해

### 12.1 P2B 구현 (본 문서 범위)

- [ ] 스키마 확장 (`20260418_000002_wiki_acl.sql`)
- [ ] `gadgetron-core::knowledge::Scope` enum + 파서
- [ ] `gadgetron-knowledge::acl::{can_read, can_write}` 함수
- [ ] `gadgetron-knowledge::wiki_write` 의 scope/locked 검증 및 기록
- [ ] `gadgetron-knowledge::hybrid_search` 쿼리 리팩터 (user_id/tenant_id 파라미터화)
- [ ] `gadgetron-xaas::TeamCache` + invalidate 훅
- [ ] Plugin seed 페이지 로직 확장 (`SeedPage.scope`, `.locked`, `source_modified_by` 보호)
- [ ] MCP tool `wiki.write/read/list/search` 전부 `AuthenticatedContext` 소비
- [ ] Web UI: scope 드롭다운, lock 토글, 페이지 badge, admin 콘솔
- [ ] CLI: `gadgetron wiki share/unshare/lock/unlock/reset-seed/search`
- [ ] Migration: 기존 페이지 → scope='org' + owner=bootstrap-admin
- [ ] Manual: `docs/manual/knowledge.md` 에 scope 섹션 추가

### 12.2 P2C

- [ ] Materialized per-user accessible_ids cache (D6 C 옵션 승격)
- [ ] Cross-tenant 공유 (기본 금지 + 명시적 허용 API)
- [ ] Scope granularity 확장 — `team:<id>/read-only` 같은 variant?

### 12.3 P3

- [ ] Unix-style per-page rwx
- [ ] Row-level encryption (content at rest by scope)
- [ ] Attribute-based ACL (frontmatter tag 기반 정책)

---

## 13. 오픈 이슈

| ID | 내용 | 옵션 | 추천 | 상태 |
|---|---|---|---|---|
| **Q-1** | Scope 변경 시 과거 버전의 ACL | A. 현재 scope 만 적용 (git history 는 DB 의 현 ACL 로 필터) / B. 버전별 scope 저장 (snapshot) | **A** — 단순. admin 이 민감 히스토리 purge 는 git 직접 조작 (운영 매뉴얼) | 🟡 |
| **Q-2** | Plugin seed 버전업 시 user 편집 처리 | A. user 편집 존중 → 건너뜀 / B. 덮어쓰되 diff 보존 / C. admin 에게 merge 선택 제시 | **A** v1 — §7.1 규칙. **C** v2+ | 🟡 |
| **Q-3** | Service user 가 org 페이지 편집 허용? | A. 허용 (§5.1 기본) / B. 금지 (bot 이 org 지식을 임의 수정 위험) | **A** — approval gate 가 destructive 만 잡음. 일반 write 는 audit 로 추적 | 🟡 |
| **Q-4** | TeamCache TTL | A. 1분 (기본) / B. 30초 (민감) / C. 5분 (성능) | **A** — invalidate 훅 있으므로 TTL 은 safety net | 🟡 |
| **Q-5** | `wiki.list` tool 의 visibility | A. accessible 만 / B. accessible + scope badge 표시 (admin 에겐 차이 인식 용) | **A** (일반 user), admin 은 web UI 에서 badge | 🟢 |
| **Q-6** | Scope 변경 때 Penny 가 자동 제안 | A. 수동 UI / B. Penny 가 컨텍스트상 판단해 제안 ("이 페이지는 팀 공유 적합해보입니다") | **A** v1, **B** v2 (품질 확인 후) | 🟡 |
| **Q-7** | `path` 와 `scope` 의 convention (예: `private/<user>/...`) 강제? | A. 강제 (reject 불일치) / B. warn 만 / C. 완전 자유 | **B** — 운영 실수 방지 + 이전 컨벤션 유연성 | 🟡 |
| **Q-8** | Search 결과 snippet 에 접근 불가 근처 페이지 인용 링크 | A. 링크 제거 (rewrite) / B. 링크 유지 (클릭 시 403) / C. 링크 자체를 scope 체크 후 제거 | **C** — info leakage 방지 | 🟡 |

---

## 14. Out of scope

- **Unix-style per-page rwx** (P3)
- **Row-level encryption** (P3)
- **Attribute-based AC** (tag 기반 정책) (P3)
- **Cross-tenant 공유** (P2C)
- **Materialized accessible_ids cache** (P2C — 규모 시)
- **Scope 변경 승인 워크플로우** (예: private → org 는 admin 승인 필요) — 기본 자유, P3 에 정책 옵션
- **Wiki 페이지 버전별 ACL snapshot** (Q-1 이 A 로 확정 시 범위 외)
- **자동 분류 / 자동 scope 할당** (Penny LLM 기반) — v2+

---

## 15. 리뷰 로그 (append-only)

### Round 0 — 2026-04-18 — PM draft
**결론**: Draft v0. 8 open question. Round 1.5 준비.

**체크리스트**:
- [x] §3 스키마 (SQL)
- [x] §5 access check (Rust)
- [x] §8 검색 pre-filter (SQL)
- [x] §11 마이그레이션 절차
- [ ] §4 단위 테스트 계획 — 구현 시 상세화

**다음 단계**: Round 1.5 (`@security-compliance-lead`, `@dx-product-lead`) → draft v1.

### Round 1.5 — YYYY-MM-DD — @security-compliance-lead @dx-product-lead
_(pending)_

### Round 2 — YYYY-MM-DD — @qa-test-architect
_(pending)_

### Round 3 — YYYY-MM-DD — @chief-architect
_(pending)_

### 최종 승인 — YYYY-MM-DD — PM
_(pending)_

---

*End of 09-knowledge-acl.md draft v0.*
