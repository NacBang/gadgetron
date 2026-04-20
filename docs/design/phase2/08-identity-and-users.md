# 08 — Identity, Users, Teams, Admin (D1 / D2 / D7 / D8)

> **담당**: PM (Claude)
> **상태**: Approved → **IMPLEMENTED on trunk** (post-PR-#246 / v0.5.7 closes ISSUE 14 — users/teams/team_members/user_sessions schema shipped via TASK 14.1, argon2id first-admin bootstrap via TASK 14.2, admin user CRUD via TASK 14.3, user self-service keys via TASK 14.4, team + member CRUD via TASK 14.5, `gadgetron user` + `gadgetron team` CLI via TASK 14.7. HTTP cookie-session login API shipped via PR #248 / ISSUE 15 TASK 15.1 / v0.5.8. Unified Bearer-or-cookie `auth_middleware` via PR #259 / ISSUE 16 / v0.5.9. `ValidatedKey.user_id` plumbing via PR #260 / ISSUE 17 / v0.5.10. `AuditEntry` actor fields structural via PR #262 / ISSUE 19 / v0.5.11. `TenantContext` → `AuditEntry` plumbing via PR #263 / ISSUE 20 / v0.5.12. **pg `audit_log` consumer shipped via PR #267 / ISSUE 21 TASK 21.1 / v0.5.13** — `run_audit_log_writer` drains `AuditWriter` mpsc and INSERTs rows to `audit_log` using ISSUE 19/20 actor columns; chat AuditEntry rows now persist end-to-end. **Admin `GET /audit/log` query endpoint shipped via PR #269 / ISSUE 22 TASK 22.1 / v0.5.14** — Management-scoped, tenant-pinned, optional `actor_user_id` + `since` filters; completes persistence → query loop. **`billing_events.actor_user_id` column shipped via PR #271 / ISSUE 23 / v0.5.15** — tool emitter populated from `ctx.actor_user_id`; chat + action paths initially wrote NULL (security-review revert: `AuthenticatedContext.user_id` was an api_key_id placeholder). Composite index `(tenant_id, actor_user_id, created_at DESC)` forces tenant-pinned queries. Harness 133 → 137 PASS (Gate 7k.6b + Gate 13 regex-fix). **`QuotaToken.user_id` + `AuthenticatedContext.real_user_id` end-to-end threading shipped via PR #289 / ISSUE 24 / v0.5.16** — chat + tool + action `billing_events` now all populate `actor_user_id` with the real user UUID per request; Gate 7k.6b flipped to assert all three `IS NOT NULL` and Gate 7k.6b-identity asserts `COUNT(DISTINCT actor_user_id) = 1` for rows emitted by the same request. Harness 137 → 139. **Web UI login form (React/Tailwind in `gadgetron-web`)** split to **ISSUE 18** (last remaining multi-user gate item). **ISSUE 25** (type-confusion hardening follow-up — full `AuthenticatedContext` rename + audit_log contamination fix + billing-insert SLO counter) is the ISSUE 24 continuation; not a `v1.0.0` gate. **Google OAuth social login** tracked separately post-ISSUE-18 on `project_multiuser_login_google`.)
> **작성일**: 2026-04-18
> **최종 업데이트**: 2026-04-18 (design) · 2026-04-20 (implementation landed: PR #246 + #248 + #259 + #260 + #262 + #263 + #267 + #269)
> **Parent**: `docs/adr/ADR-P2A-08-multi-user-foundation.md`, `docs/process/04-decision-log.md` D-20260418-02
> **Sibling**: [`09-knowledge-acl.md`](09-knowledge-acl.md), [`10-penny-permission-inheritance.md`](10-penny-permission-inheritance.md)
> **Drives**: P2B 구현 — `users` / `teams` / `team_members` 스키마 ✅, user session ✅, admin bootstrap ✅, CLI/REST ✅, unified Bearer/cookie middleware ✅, `ValidatedKey.user_id` plumbing ✅, `AuditEntry` actor fields structural ✅, TenantContext → AuditEntry plumbing ✅, pg audit_log consumer ✅, admin `/audit/log` query endpoint ✅, web UI 로그인 폼 ⏳ ISSUE 18 (last remaining item).
> **관련 크레이트**: `gadgetron-xaas` (스키마 + 인증 로직 + `ValidatedKey.user_id` + `AuditEntry.actor_*` fields) ✅, `gadgetron-core` (타입) ✅, `gadgetron-gateway` (session middleware + unified auth_middleware + cookie-path user_id populate + audit callsites defaulting to `None`) ✅, `gadgetron-cli` (user/team 서브커맨드) ✅, `gadgetron-web` (로그인 UI) ⏳ ISSUE 18
> **Phase**: [P2B]

---

## Table of Contents

1. 철학 & 컨셉
2. 상세 구현 방안 (용어 / 스키마 / role / bootstrap / auth / API / CLI / config / error / deps)
3. 전체 모듈 연결 구도
4. 단위 테스트 계획
5. 통합 테스트 계획
6. Phase 구분
7. 오픈 이슈
8. Out of scope
9. 리뷰 로그

---

## 1. 철학 & 컨셉

### 1.1 해결하는 문제

Phase 2A 까지 `gadgetron-xaas` 는 API key ↔ tenant 직결 구조였다. "누가 이 wiki 를 썼는가", "누가 Penny 를 불렀는가", "누가 이 호스트에 명령을 내렸는가" 의 답이 **API key prefix** 에서 멈추고 사람 identity 에 수렴하지 않았다.

해결:
- **User 레이어를 xaas 위에 추가**. API key / web UI session / Penny subprocess 모두 `user_id` 로 수렴
- **Team 은 D7 의 Postgres 테이블**. 팀 단위 wiki scope (09 doc) 및 승인 routing 의 대상
- **Admin role 은 플래그**. OS/config 레벨 super-admin 은 bootstrap 경로에만 쓰고 이후 DB 가 권위

### 1.2 D-20260418-02 와의 매핑

| 결정 | 이 문서가 구현 |
|:---:|---|
| D1 | §3 `users` 테이블 + `api_keys.user_id FK`, §6 인증 플로우 |
| D2 | §3 `tenant_id` NOT NULL placeholder, §9 tenant 단일화 전략 |
| D7 | §3 `teams` / `team_members` 테이블, §7 team REST API, §8 team CLI |
| D8 | §4 role enum, §5 bootstrap admin, §7 admin REST API |

### 1.3 핵심 설계 원칙

1. **Identity-first audit** — 모든 행위의 actor 는 `user_id`. API key 도 user 소속, Penny 도 caller user 대리
2. **DB 가 권위, config 는 bootstrap 만** — `users.role` 이 유일 소스, config 의 bootstrap 설정은 DB empty 일 때만 반영
3. **미래 multi-tenant 호환** — 모든 테이블에 `tenant_id` 박되 P2B 에서는 `"default"` 고정, enforcement 는 P2C
4. **Service role 분리** — 비인간 자동화 (외부 SDK 호출자) 는 별도 `role = 'service'` — UI 로그인 불가, approval 대답 불가

---

## 2. 상세 구현 방안 (What & How)

### 2.1 용어

| 용어 | 의미 |
|---|---|
| **User** | 사람 identity. `users` 테이블 row. email + role + tenant 소속 |
| **Tenant** | 격리 단위. P2B 는 `"default"` 하나만. P2C 에 동적 |
| **API key** | user 가 발급한 장기 인증 크레덴셜. `api_keys` 테이블에 SHA-256 해시만 저장 |
| **Session** | web UI 로그인 상태. 단기. 쿠키 기반 |
| **Role** | `users.role ∈ {member, admin, service}` |
| **Team** | user 의 group. wiki scope + 승인 routing 대상 |
| **`admins` virtual team** | `users.role = 'admin'` 인 user 들의 암묵적 team. `teams`/`team_members` 테이블에 row 없음 |
| **Bootstrap admin** | 빈 DB 에서 첫 admin 을 만들기 위한 config-backed 계정 |

---

### 2.2 스키마

#### 2.2.1 신규 테이블 — `users`

```sql
CREATE TABLE users (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL REFERENCES tenants(id) DEFAULT '00000000-0000-0000-0000-000000000001',
                    -- P2B: 모든 user 가 "default" tenant (고정 UUID) 소속
    email           TEXT NOT NULL,
    display_name    TEXT NOT NULL,
    role            TEXT NOT NULL CHECK (role IN ('member', 'admin', 'service')),
    password_hash   TEXT,         -- argon2id. service 는 NULL
    is_active       BOOLEAN NOT NULL DEFAULT TRUE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_login_at   TIMESTAMPTZ,

    UNIQUE (tenant_id, email),   -- tenant 안에서만 unique (P2C 에 맞춰 확장)
    CHECK (role != 'service' OR password_hash IS NULL)  -- service 는 password 없음
);

CREATE INDEX idx_users_tenant_active ON users (tenant_id, is_active) WHERE is_active;
CREATE INDEX idx_users_email_active  ON users (email) WHERE is_active;
```

#### 2.2.2 신규 테이블 — `teams` + `team_members`

```sql
CREATE TABLE teams (
    id              TEXT PRIMARY KEY,            -- kebab-case 운영자 지정: "platform", "ml-ops"
    tenant_id       UUID NOT NULL REFERENCES tenants(id) DEFAULT '00000000-0000-0000-0000-000000000001',
    display_name    TEXT NOT NULL,
    description     TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by      UUID REFERENCES users(id),

    CHECK (id ~ '^[a-z][a-z0-9-]{0,31}$'),       -- kebab-case, 32자 max
    CHECK (id != 'admins')                       -- built-in virtual team 과 충돌 방지
);

CREATE TABLE team_members (
    team_id         TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role            TEXT NOT NULL DEFAULT 'member' CHECK (role IN ('member', 'lead')),
    added_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    added_by        UUID REFERENCES users(id),

    PRIMARY KEY (team_id, user_id)
);

CREATE INDEX idx_team_members_user ON team_members (user_id);
```

**왜 `teams.id` 는 TEXT (kebab-case) 이고 `users.id` 는 UUID 인가?**

- team id 는 **운영자·UI·wiki frontmatter** 에 자주 노출 (`scope = "team:platform"`). 인간이 읽어야 하므로 kebab-case 문자열
- user id 는 이메일 변경·이름 변경과 무관한 **안정 식별자**. UUID 가 적절. 외부 노출 시에는 `display_name` 사용

#### 2.2.3 기존 `api_keys` 테이블 확장

```sql
-- 기존 스키마 (xaas phase1.md §2.2)
-- CREATE TABLE api_keys (
--     id            UUID PRIMARY KEY,
--     tenant_id     UUID NOT NULL REFERENCES tenants(id),
--     key_hash      TEXT NOT NULL,
--     prefix        TEXT NOT NULL,
--     scopes        TEXT[] NOT NULL,
--     ...
-- );

-- P2B 에 추가:
ALTER TABLE api_keys ADD COLUMN user_id UUID REFERENCES users(id);
ALTER TABLE api_keys ADD COLUMN label TEXT;     -- 사람이 식별용 (예: "ci-deploy", "alice-laptop")

-- 마이그레이션 절차 (§10 에서 상세): 기존 row 에 default admin user 의 id 주입
UPDATE api_keys SET user_id = '<default-admin-uuid>' WHERE user_id IS NULL;

-- 그 후 NOT NULL 전환
ALTER TABLE api_keys ALTER COLUMN user_id SET NOT NULL;

CREATE INDEX idx_api_keys_user ON api_keys (user_id) WHERE revoked_at IS NULL;
```

API key 는 **user 가 소유**. key 의 `scopes` 는 user 의 role 보다 **좁거나 같아야** 함 (validation 규칙은 §6.2).

#### 2.2.4 Session 테이블 — web UI 로그인

```sql
CREATE TABLE user_sessions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    tenant_id       UUID NOT NULL,
    cookie_hash     TEXT NOT NULL,           -- SHA-256 of secure cookie token
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at      TIMESTAMPTZ NOT NULL,
    last_active_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    user_agent      TEXT,
    ip_address      INET,
    revoked_at      TIMESTAMPTZ
);

CREATE INDEX idx_sessions_cookie_active ON user_sessions (cookie_hash)
    WHERE revoked_at IS NULL;
CREATE INDEX idx_sessions_user_active   ON user_sessions (user_id, expires_at)
    WHERE revoked_at IS NULL;
```

- 기본 세션 TTL 24시간, idle rotation (last_active_at + 2h 에 expire)
- Cookie 는 `HttpOnly; Secure; SameSite=Lax; Path=/`
- Logout = `revoked_at = now()`

#### 2.2.5 Audit log 확장 (D5 선언, 여기 명시)

```sql
ALTER TABLE audit_log ADD COLUMN actor_user_id    UUID REFERENCES users(id);
ALTER TABLE audit_log ADD COLUMN actor_api_key_id UUID REFERENCES api_keys(id);
ALTER TABLE audit_log ADD COLUMN impersonated_by  TEXT;   -- 'penny' | NULL
ALTER TABLE audit_log ADD COLUMN parent_request_id TEXT;

-- 기존 tenant_id 필드는 유지
CREATE INDEX idx_audit_user_time ON audit_log (actor_user_id, timestamp DESC);
```

---

### 2.3 User 유형 및 role

```rust
// gadgetron-core::identity
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Member,      // 기본 user. 자기 scope 내 wiki CRUD, Penny 사용
    Admin,       // 모든 wiki 접근, user/team 관리, plugin enable/disable, 설정 변경
    Service,     // 비인간 자동화. UI 로그인 불가, API key 로만
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id:           UserId,                   // Uuid
    pub tenant_id:    TenantId,
    pub email:        String,
    pub display_name: String,
    pub role:         Role,
    pub is_active:    bool,
    pub created_at:   DateTime<Utc>,
}
```

#### 2.3.1 Role 비교표

| 기능 | member | admin | service |
|---|:---:|:---:|:---:|
| Web UI 로그인 | ✅ | ✅ | ❌ |
| API key 발급 받기 | ✅ | ✅ | ✅ (외부 절차) |
| 자기 wiki (private/team scope) 편집 | ✅ | ✅ | ✅ |
| Org-scope wiki 편집 | ✅ | ✅ | ✅ (단 T3 tool 은 approval 불가로 denied) |
| 다른 user 의 private wiki 읽기 | ❌ | ✅ | ❌ |
| `ADMIN_ONLY_TOOLS` 호출 | ❌ | ✅ | ❌ |
| T3 destructive tool approval 응답 | ✅ | ✅ | ❌ |
| Penny 호출 시 상속되는 권한 | member | admin | service (제한적) |

#### 2.3.2 `admins` virtual team

- `users.role = 'admin'` 인 user 전원이 암묵적 `admins` team 멤버
- `teams` 에 `id = 'admins'` row 는 **존재하지 않음** (CHECK 제약). UI 에 표시는 "virtual" 배지
- ACL 평가 시:

```rust
pub fn is_team_member(user: &User, team_id: &str, cache: &TeamCache) -> bool {
    if team_id == "admins" {
        user.role == Role::Admin
    } else {
        cache.get(user.id).map_or(false, |teams| teams.contains(team_id))
    }
}
```

#### 2.3.3 Service user 특수성

- `service` user 의 API key 만 외부 SDK 자동화에 사용
- Penny 호출 시 `GADGETRON_CALLER_ROLE = service` 로 env 주입
- T2/T3 tool 중 approval gate `ask` mode 에 도달하면 **자동 거부** (automated caller 가 사람 대답 불가)
- Gateway 가 반환 시 `{"error": "approval_required", "hint": "automated service cannot respond to approval prompts"}` 명시 — clear failure

---

### 2.4 Bootstrap admin

#### 2.4.1 빈 DB 에서 첫 admin 을 만드는 유일한 경로

```toml
# gadgetron.toml
[auth.bootstrap]
# 이 섹션은 DB 의 users 테이블이 EMPTY 일 때만 동작.
# 이미 user 가 존재하면 기동 시 warning + 무시.
admin_email         = "admin@example.com"
admin_display_name  = "Admin"
admin_password_env  = "GADGETRON_BOOTSTRAP_ADMIN_PASSWORD"
                     # 평문 password 필드는 **없음**. env 참조만 허용.
                     # 값 누락 시 기동 실패 + 명시적 에러
```

#### 2.4.2 기동 로직 (`gadgetron-cli::main`)

```rust
async fn bootstrap_admin_if_needed(
    db: &Pool<Postgres>,
    config: &AppConfig,
) -> Result<()> {
    let user_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(db).await?;

    if user_count > 0 {
        if config.auth.bootstrap.is_some() {
            tracing::warn!(
                "auth.bootstrap is configured but users table is not empty — \
                 ignoring bootstrap config. Remove [auth.bootstrap] from gadgetron.toml."
            );
        }
        return Ok(());
    }

    let Some(bootstrap) = &config.auth.bootstrap else {
        return Err(GadgetronError::Configuration {
            message: "users table is empty but [auth.bootstrap] is missing. \
                      Set [auth.bootstrap] in gadgetron.toml to create the first admin.".into(),
        });
    };

    let password = std::env::var(&bootstrap.admin_password_env)
        .map_err(|_| GadgetronError::Configuration {
            message: format!(
                "env {} is not set — required for bootstrap admin password",
                bootstrap.admin_password_env
            ),
        })?;

    let password_hash = argon2_hash(&password)?;

    sqlx::query(
        "INSERT INTO users (tenant_id, email, display_name, role, password_hash)
         VALUES ($1, $2, $3, 'admin', $4)"
    )
    .bind(DEFAULT_TENANT_ID)
    .bind(&bootstrap.admin_email)
    .bind(&bootstrap.admin_display_name)
    .bind(&password_hash)
    .execute(db)
    .await?;

    tracing::info!(email = %bootstrap.admin_email, "bootstrap admin created");
    Ok(())
}
```

#### 2.4.3 보안 가이드 (운영자에게)

첫 admin 로그인 후:

1. `[auth.bootstrap]` 섹션 제거 또는 주석 처리 (M-MU1)
2. 비밀번호 변경 (`gadgetron user set-password <email>`)
3. (권장) 두 번째 admin 추가 — 단일 admin 이 잠기면 DB 직접 접근 필요해짐
4. 추가 admin 생성 후 bootstrap 계정을 비활성화 (`is_active = false`) 하고 새 admin 사용

이 가이드는 seed page `infra/server/getting-started.md` (07 doc §8) 와 별개로 **`docs/manual/admin-operations.md`** 신설 대상.

---

### 2.5 인증 플로우

#### 2.5.1 Web UI — 로그인 세션

```
POST /v1/auth/login
Content-Type: application/json
{
  "email": "alice@example.com",
  "password": "..."
}

200 OK
Set-Cookie: gadgetron_session=<token>; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=86400
{
  "user": { "id": "...", "email": "...", "display_name": "...", "role": "member" }
}
```

- argon2id 로 password verify
- session row INSERT, cookie 발급
- session token 은 32-byte random, DB 에는 SHA-256 hash 저장
- 이후 요청은 middleware 가 cookie → session lookup → `User` 해소 → request extension 에 주입

#### 2.5.2 API key — 장기 크레덴셜

```
POST /v1/chat/completions                         # 또는 /api/v1/*
Authorization: Bearer gad_live_<prefix>_<secret>
```

기존 xaas 플로우 (phase1.md §2.2.2) 에 **user 해석** 단계 추가:

```rust
async fn resolve_api_key(raw: &str, db: &Pool<Postgres>) -> Result<AuthContext> {
    // (1) prefix 파싱 + SHA-256 hash 비교 (기존 로직)
    let row = sqlx::query!(
        "SELECT k.id, k.user_id, k.tenant_id, k.scopes, u.role, u.is_active
         FROM api_keys k
         INNER JOIN users u ON u.id = k.user_id
         WHERE k.key_hash = $1 AND k.revoked_at IS NULL AND u.is_active",
        hash_api_key(raw)
    )
    .fetch_optional(db)
    .await?
    .ok_or(GadgetronError::AuthInvalid)?;

    // (2) key.scopes ⊆ user.role 의 allowed scopes 검증
    validate_scopes(&row.scopes, row.role)?;

    Ok(AuthContext {
        user_id:     row.user_id,
        tenant_id:   row.tenant_id,
        role:        row.role,
        api_key_id:  Some(row.id),
        session_id:  None,
        teams:       team_cache.get(row.user_id).await?,
    })
}
```

#### 2.5.3 Penny subprocess — env 주입

Penny 는 web UI 세션 또는 API key 로 **이미 인증된** caller 의 대리. spawn 시점에 caller 의 `AuthContext` → subprocess env:

```rust
fn spawn_penny(ctx: &AuthContext, cmd: &mut Command) {
    cmd.env("GADGETRON_CALLER_USER_ID", ctx.user_id.to_string());
    cmd.env("GADGETRON_CALLER_TENANT_ID", ctx.tenant_id.to_string());
    cmd.env("GADGETRON_CALLER_ROLE", ctx.role.as_str());
    cmd.env("GADGETRON_CALLER_TEAMS", ctx.teams.iter().collect::<Vec<_>>().join(","));
    cmd.env("GADGETRON_REQUEST_ID", ctx.request_id.to_string());
    cmd.env("ANTHROPIC_API_KEY", ctx.penny_shim_token.expose());  // D-20260414-04 (e)
    // 평문 password 포함 금지. api_key raw 값 포함 금지.
}
```

상세 보안 분석은 10 doc §5–§7.

#### 2.5.4 `AuthContext` 타입

```rust
// gadgetron-core::auth
pub struct AuthContext {
    pub user_id:        UserId,
    pub tenant_id:      TenantId,
    pub role:           Role,
    pub api_key_id:     Option<ApiKeyId>,      // None = web session
    pub session_id:     Option<SessionId>,     // None = API key
    pub teams:          Arc<TeamSet>,
    pub request_id:     RequestId,             // per-request correlation
    pub penny_shim_token: Option<SecretCell<String>>,  // Penny spawn 용 (D-20260414-04 (e))
}
```

이 타입은 MCP tool invoke 시 `AuthenticatedContext` 로 변환 (10 doc §4). 변환 과정에서 `api_key_id`/`session_id` 는 audit 용도로만 보존, tool 로직에는 `user_id` + `role` + `teams` 만 노출.

---

### 2.6 REST API surface

#### 2.6.1 User 관리

| Method | Path | Role 요구 | 설명 |
|---|---|:---:|---|
| `POST` | `/v1/auth/login` | — | email + password → session |
| `POST` | `/v1/auth/logout` | any | 세션 revoke |
| `GET` | `/v1/auth/me` | any | 현재 user 정보 |
| `GET` | `/api/v1/users` | admin | user 목록 |
| `POST` | `/api/v1/users` | admin | user 생성 |
| `GET` | `/api/v1/users/{id}` | admin 또는 self | user 상세 |
| `PATCH` | `/api/v1/users/{id}` | admin 또는 self (제한 필드) | display_name / password 변경 |
| `PATCH` | `/api/v1/users/{id}/role` | admin | role 변경 (M-MU6 참조: self-demotion 금지?) |
| `DELETE` | `/api/v1/users/{id}` | admin | `is_active = false` soft delete |
| `POST` | `/api/v1/users/{id}/password-reset` | admin | 임시 비밀번호 생성, 로그에 저장하지 않음 |

#### 2.6.2 Team 관리

| Method | Path | Role 요구 | 설명 |
|---|---|:---:|---|
| `GET` | `/api/v1/teams` | any | team 목록 (member 는 자기 소속 + public, admin 은 전체) |
| `POST` | `/api/v1/teams` | admin | team 생성 |
| `GET` | `/api/v1/teams/{id}` | any | team 상세 + 멤버 목록 |
| `DELETE` | `/api/v1/teams/{id}` | admin | team 삭제 (CASCADE 로 team_members 정리) |
| `POST` | `/api/v1/teams/{id}/members` | admin 또는 lead | 멤버 추가 |
| `DELETE` | `/api/v1/teams/{id}/members/{user_id}` | admin 또는 lead | 멤버 제거 |
| `PATCH` | `/api/v1/teams/{id}/members/{user_id}` | admin 또는 lead | 역할 (member/lead) 변경 |

#### 2.6.3 API key 관리 (user 자기 자신)

| Method | Path | Role 요구 | 설명 |
|---|---|:---:|---|
| `GET` | `/api/v1/keys` | any | 자기 key 목록 (prefix + label + scopes, raw 키 노출 X) |
| `POST` | `/api/v1/keys` | any | 새 key 생성. 응답에 raw 키 1회 노출, 이후 SHA-256 만 보관 |
| `DELETE` | `/api/v1/keys/{id}` | any (self) 또는 admin | key revoke |
| `GET` | `/api/v1/xaas/keys?tenant_id=X` | admin 또는 `XaasAdmin` scope | 전체 key 목록 (기존 xaas phase1 유지) |

---

### 2.7 CLI 서브커맨드

#### 2.7.1 User

```sh
gadgetron user create --email alice@example.com --name "Alice" --role member
gadgetron user list [--role admin] [--team platform] [--inactive]
gadgetron user show <email|uuid>
gadgetron user promote <email|uuid>            # role: member → admin
gadgetron user demote <email|uuid>             # admin → member (single-admin guard)
gadgetron user deactivate <email|uuid>
gadgetron user set-password <email|uuid>       # 대화형 prompt
gadgetron user reset-password <email|uuid>     # admin 용, 임시 비번 출력
```

모든 명령은 CLI 실행자 식별 필요:
- 개발/bootstrap 단계: `GADGETRON_ADMIN_TOKEN` env (bootstrap admin 가 받은 임시 token)
- 운영 단계: `gadgetron login` 으로 CLI 전용 session 생성 (~/.gadgetron/session 저장, file mode 0600)

#### 2.7.2 Team

```sh
gadgetron team create <id> --display-name "Platform Team" [--description "..."]
gadgetron team list
gadgetron team show <id>
gadgetron team members <id>
gadgetron team add <id> <user-email>  [--role lead|member]
gadgetron team remove <id> <user-email>
gadgetron team delete <id>
```

#### 2.7.3 Bootstrap helper

```sh
gadgetron auth bootstrap              # 현재 [auth.bootstrap] 설정 검증 + 실행 상태 리포트
gadgetron auth status                 # 현재 인증 주체 표시 (CLI session 또는 ADMIN_TOKEN)
gadgetron auth login                  # 대화형 로그인, 세션 저장
gadgetron auth logout                 # 로컬 세션 파일 제거
```

---

### 2.8 설정 스키마

이 문서가 새로 고정하는 설정 surface 는 bootstrap 과 session cookie 두 축이다.

```toml
[auth.bootstrap]
enabled = false
admin_email = "admin@example.com"
admin_password_env = "GADGETRON_BOOTSTRAP_ADMIN_PASSWORD"
admin_display_name = "Gadgetron Admin"

[auth.sessions]
cookie_name = "gadgetron_session"
idle_ttl_minutes = 120
absolute_ttl_hours = 24
secure_cookie = true
same_site = "lax"
```

검증 규칙:

- `auth.bootstrap.enabled = true` 는 빈 `users` 테이블일 때만 유효하다. user row 가 하나라도 있으면 startup fail-closed.
- `admin_password_env` 는 비어 있으면 안 된다. 환경변수가 없으면 기동 거부.
- `idle_ttl_minutes` 는 `15..=1440`, `absolute_ttl_hours` 는 `1..=168` 범위만 허용한다.
- `secure_cookie = true` 가 기본이며, 개발 모드에서만 명시적으로 false 허용.

### 2.9 에러 & 로깅

- `gadgetron-core` 신규 타입: `UserId`, `Role`, `TeamId`, `TeamSet`
- `gadgetron-xaas` 신규 에러 매핑:
  - 중복 email / team id -> `GadgetronError::Configuration`
  - 비활성 user / 잘못된 password / 만료 session -> `GadgetronError::AuthInvalid`
  - self-demotion / single-admin 제거 시도 -> `GadgetronError::QuotaExceeded` 가 아니라 전용 admin-guard 메시지를 포함한 `Configuration`
- `tracing` 이벤트:
  - `info`: `bootstrap_admin_created`, `user_login_succeeded`, `team_member_added`
  - `warn`: `bootstrap_admin_rejected_non_empty_db`, `user_login_failed`, `session_rejected_expired`
  - `target = "gadgetron_audit"`: admin 승격/강등, password reset, cross-user private wiki access

STRIDE 요약:

- Spoofing: password/session/API key 모두 `user_id` 로 수렴, anonymous fallback 금지
- Tampering: bootstrap 경로는 empty-db guard + audit
- Repudiation: `actor_user_id` / `actor_api_key_id` / `parent_request_id` 확장
- Information disclosure: session cookie 는 opaque ID 만 담고, user 존재 여부를 login error 로 구분하지 않음
- DoS: session TTL 과 inactive-user short-circuit 로 stale session 축적 제한
- Elevation of privilege: single-admin guard + self-demotion 방어

### 2.10 의존성

- `gadgetron-core`: 신규 외부 의존성 없음
- `gadgetron-xaas`: `argon2 = "0.5"` 추가, password hash 는 argon2id 로 고정
- `gadgetron-gateway`: 기존 `axum`, `tower`, `tower-http` 재사용. 별도 세션 프레임워크 추가 없이 opaque session id + SQL store로 충분
- `gadgetron-cli` / `gadgetron-web`: 기존 workspace 의 `clap`, `serde`, `reqwest` 범위 재사용

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 크레이트 연결

```text
gadgetron-web / gadgetron-cli
        |
        v
gadgetron-gateway -----> gadgetron-xaas -----> PostgreSQL
        |                       |
        |                       +-----> users / teams / sessions / api_keys / audit_log
        |
        +-----> gadgetron-core::identity::{UserId, Role, TeamSet}
        |
        +-----> gadgetron-penny (caller identity 상속, 10 doc 소비)
```

- `gadgetron-core` 는 identity 타입만 소유하고 DB 로직은 가지지 않는다.
- `gadgetron-xaas` 는 user/team/session repository 와 password hash 검증을 소유한다.
- `gadgetron-gateway` 는 API key 또는 web session 을 `AuthContext` 로 정규화한다.
- `gadgetron-web` 과 CLI 는 서로 다른 touchpoint 이지만 같은 auth surface 를 쓴다.

### 3.2 데이터 흐름

```text
Browser login / API key / CLI login
        |
        v
gateway auth middleware
        |
        v
xaas repositories -> AuthContext(user_id, role, teams, tenant_id)
        |
        +----> audit_log
        |
        +----> knowledge ACL (09)
        |
        +----> Penny env injection (10)
```

### 3.3 D-12 크레이트 경계 준수

- `UserId`, `Role`, `TeamSet`, `AuthContext` 공개 타입은 `gadgetron-core` 또는 `gadgetron-xaas`에만 둔다.
- SQL schema / repository / password hash / session persistence 는 `gadgetron-xaas` 소유다.
- gateway 는 middleware 와 caller-facing error mapping만 담당한다.

### 3.4 Tenant 단일화 전략 (P2B) + P2C 확장 슬롯

#### 3.4.1 P2B 에서의 tenant

- `tenants` 테이블에 row `"default"` 한 개 (UUID `00000000-0000-0000-0000-000000000001`)
- 모든 `users`, `teams`, `api_keys`, `wiki_pages` 등이 이 tenant 소속
- tenant 생성/삭제 UI 없음
- 모든 API response 에 `tenant_id` 필드 표시 — P2C 에 user 가 여러 tenant 에 속할 수 있게 되면 UI 가 이미 필드를 이해

#### 3.4.2 P2C 확장 슬롯

- `POST /api/v1/tenants` — tenant 생성 (super-admin 만)
- `POST /api/v1/tenants/{id}/invite` — user 초대
- `users.tenant_id` 을 **복수 tenant 소속** 으로 확장 (`user_tenants` join 테이블) 또는 primary tenant + guest 멤버십 모델 — P2C 에 결정
- Gateway middleware 가 request 에 `X-Gadgetron-Tenant: <id>` 헤더 또는 subdomain 으로 active tenant 결정
- Cross-tenant 공유는 명시적 "share with tenant X" — 기본 불가

#### 3.4.3 P2B 에서 건드리지 말 것

- tenant 격리 enforcement (scheduler quota 가 tenant 단위 격리하는 것은 **xaas phase1** 기존 로직 유지. 즉 이미 isolation 은 있지만 활성화된 tenant 가 하나뿐)
- tenant 간 user 공유

---

### 3.5 마이그레이션 절차

#### 3.5.1 기존 xaas → multi-user 마이그레이션

```sql
-- Migration: 20260418_000001_users_teams.sql

-- (1) 기본 tenant 확보
INSERT INTO tenants (id, name, created_at)
VALUES ('00000000-0000-0000-0000-000000000001', 'default', now())
ON CONFLICT (id) DO NOTHING;

-- (2) users 테이블
CREATE TABLE users ( ... );  -- §3.1
CREATE INDEX ... ;

-- (3) teams + team_members
CREATE TABLE teams ( ... );       -- §3.2
CREATE TABLE team_members ( ... );
CREATE INDEX ... ;

-- (4) user_sessions
CREATE TABLE user_sessions ( ... );
CREATE INDEX ... ;

-- (5) api_keys 확장
ALTER TABLE api_keys ADD COLUMN user_id UUID REFERENCES users(id);
ALTER TABLE api_keys ADD COLUMN label TEXT;

-- (6) audit_log 확장
ALTER TABLE audit_log ADD COLUMN actor_user_id UUID REFERENCES users(id);
ALTER TABLE audit_log ADD COLUMN actor_api_key_id UUID REFERENCES api_keys(id);
ALTER TABLE audit_log ADD COLUMN impersonated_by TEXT;
ALTER TABLE audit_log ADD COLUMN parent_request_id TEXT;
CREATE INDEX idx_audit_user_time ON audit_log (actor_user_id, timestamp DESC);

-- (7) wiki_pages 확장 (09 doc §3 에서 상세. 여기에 선언만)
ALTER TABLE wiki_pages ADD COLUMN scope TEXT NOT NULL DEFAULT 'private';
ALTER TABLE wiki_pages ADD COLUMN owner_user_id UUID REFERENCES users(id);
ALTER TABLE wiki_pages ADD COLUMN locked BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE wiki_pages ADD COLUMN source_modified_by TEXT;
```

#### 3.5.2 데이터 이관 (runtime 스크립트)

```sh
gadgetron migrate v2b-multiuser \
    --default-admin-email admin@example.com \
    --default-admin-password-env GADGETRON_MIGRATION_ADMIN_PASSWORD
```

이 명령이 수행:

1. Bootstrap admin user 생성 (§5 절차 그대로)
2. 기존 `api_keys` 의 모든 row 에 `user_id = <bootstrap-admin-id>` 주입, `label = CONCAT('legacy-', prefix)` 설정
3. `api_keys.user_id` NOT NULL 전환
4. 기존 wiki 페이지 전체를 scan:
   - `scope = 'org'` (기존 단일 운영자의 지식 = 공유 지식으로 해석)
   - `owner_user_id = <bootstrap-admin-id>`
   - `locked = false`
5. 리포트 출력: "N users created, M keys migrated, P wiki pages scoped"

#### 3.5.3 운영 중단 vs rolling

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

- `bootstrap_admin_rejected_when_users_table_non_empty`
- `bootstrap_admin_requires_password_env`
- `single_admin_cannot_self_demote`
- `service_user_cannot_create_web_session`
- `expired_session_is_rejected_without_db_write_amplification`
- `api_key_auth_resolves_user_id_and_team_set`
- `team_membership_roundtrip_preserves_lead_role`

### 4.2 테스트 하네스

- repository 레이어는 `sqlx::test` 또는 dedicated Postgres harness 사용
- password hash 검증은 fixed salt fixture 로 결정론 보장
- session 만료 검증은 `tokio::time::pause()` 기반으로 wall-clock 의존 제거

### 4.3 커버리지 목표

- `gadgetron-xaas` identity/session 모듈 line coverage 85% 이상
- admin/bootstrap guard 와 session expiry branch coverage 90% 이상

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

- `gadgetron-gateway` + `gadgetron-xaas` + PostgreSQL
- `gadgetron-cli auth login/status/logout`
- `gadgetron-web` 로그인 -> 세션 쿠키 -> `/api/v1/users/me`

### 5.2 테스트 환경

- `testcontainers` PostgreSQL
- web session은 local HTTP harness 로 검증, 브라우저 의존 e2e 는 `gadgetron-testing` smoke 로 분리
- migration path 는 legacy `api_keys` fixture DB snapshot 에서 시작

### 5.3 회귀 방지

- 빈 DB 가 아닌 상태에서 bootstrap 이 다시 열리면 실패해야 한다
- single-admin guard 제거 시 admin lockout 회귀가 즉시 드러나야 한다
- session TTL 계산이 어긋나 stale session 이 살아남으면 테스트가 실패해야 한다

---

## 6. Phase 구분

- 스키마 변경은 additive 만 (NOT NULL 전환은 데이터 이관 **후**) → read-side 호환성 유지
- Gateway 는 `AuthContext` 해석 실패 시 legacy 모드로 fallback (bootstrap 기간에만), `X-Gadgetron-Migration-Mode: true` 헤더 + 경고 로그
- 마이그레이션 완료 후 legacy fallback 코드 제거 PR

---

### 6.1 P2B 구현 (본 문서 범위)

- [x] 스키마 마이그레이션 (`20260420000004_users_teams_sessions.sql` — TASK 14.1 / PR #246 / v0.5.7)
- [x] `gadgetron-core::identity::{User, Role, UserId}`, `Team`, `TeamSet` (TASK 14.1 / PR #246)
- [x] `gadgetron-xaas::UserRepository`, `TeamRepository`, `SessionRepository` (TASK 14.3 / 14.5 / 15.1)
- [x] `gadgetron-xaas::PasswordHasher` (argon2id) — TASK 14.2 bootstrap + TASK 15.1 login verify share the `argon2_hash` / `argon2_verify` helpers in `gadgetron-xaas::auth::bootstrap` + `::auth::sessions`.
- [x] `gadgetron-xaas::AuthContext` + API key → user 해소 로직 — `api_keys.user_id` backfill in `init_serve_runtime` (TASK 14.3 / PR #246).
- [x] `gadgetron-gateway::auth_middleware` (web session + API key 통합) — **TASK 16.1 / PR #259 / v0.5.9**. Bearer path unchanged; cookie fallback via `validate_session_and_build_key` when no Bearer header, role → scope synthesis (admin → `[OpenAiCompat, Management]`; member → `[OpenAiCompat]`).
- [x] `gadgetron-cli::user`, `cli::team` 서브커맨드 (TASK 14.7 / PR #246) — `cli::auth` subcommand still **not shipped** (would wrap the `/auth/login` endpoint for shell-driven session flows); deferred to post-v1.0.0.
- [ ] `gadgetron-web`: 로그인 페이지 (**ISSUE 18**) + session 관리 ✅ (cookie-session API shipped TASK 15.1) + admin 콘솔 (users/teams CRUD UI — post-v1.0.0).
- [x] Bootstrap admin 로직 (`gadgetron-cli::main` + `gadgetron-xaas::auth::bootstrap::bootstrap_admin_if_needed`) — TASK 14.2 / PR #246.
- [ ] Migration 스크립트 (`gadgetron migrate v2b-multiuser`) — **not shipped**; current path uses `gadgetron serve` to auto-apply sqlx migrations.
- [x] Manual: `docs/manual/auth.md` rewritten with cookie-session + unified middleware coverage; `docs/manual/admin-operations.md` NOT created (content merged into `auth.md` + `api-reference.md` instead).

### 6.2 P2C 확장

- [ ] `POST /api/v1/tenants`, tenant 생성/초대 UI
- [ ] Multi-tenant enforcement 활성화 (gateway middleware)
- [ ] SSO (OIDC) 통합 — 외부 IdP → local user row 매핑
- [ ] LDAP / AD sync
- [ ] Tenant 간 cross-scope 공유 정책 (기본 금지 + 명시적 허용)

### 6.3 P3

- [ ] Fine-grained RBAC (team-admin, billing-viewer, audit-viewer 등)
- [ ] MFA (TOTP / WebAuthn)
- [ ] Password policy (rotation, complexity)

---

## 7. 오픈 이슈 / 의사결정 필요

| ID | 내용 | 옵션 | 추천 | 상태 |
|---|---|---|---|---|
| **Q-1** | Self-demotion 허용? (admin A 가 자기 role 을 member 로 변경) | A. 허용 / B. 다른 admin 존재 시에만 / C. 금지 (admin 간 상호 demotion 만) | **B** — 시스템에 admin 이 없어지는 시나리오 방지 | 🟡 |
| **Q-2** | 단일 admin lockout 복구 | A. CLI `gadgetron recovery --from-db` (DB direct) / B. bootstrap 재활성화 (가장 위험) / C. OS 레벨 escape hatch (sudo required) | **A** — DB direct 접근이 있는 환경만. 문서화 | 🟡 |
| **Q-3** | Password policy | A. 없음 (admin 재량) / B. 최소 10자 / C. zxcvbn 점수 ≥ 3 | **B** v1, **C** v2 | 🟡 |
| **Q-4** | Session TTL 기본값 | A. 2h idle / 24h absolute / B. 8h idle / 7d absolute / C. config | **A** conservative, config override | 🟡 |
| **Q-5** | Service user 의 password_hash | A. NULL 강제 (CHECK 제약) / B. random hash 로 채우되 login 차단 | **A** (§3.1 스키마대로) | 🟢 |
| **Q-6** | Admin 이 다른 user 의 private wiki 읽을 때 audit 별도 기록 | A. 일반 wiki.read 와 동일 / B. `admin_access = true` flag | **B** — 감사·규제 고려 | 🟡 |
| **Q-7** | `users.email` 변경 허용? | A. 불가 / B. admin 이 변경 가능 / C. user 본인이 변경 (확인 이메일) | **B** v1, **C** v3 (email 전송 인프라 전제) | 🟡 |
| **Q-8** | Team lead role 의 추가 권한 | A. 멤버 관리만 / B. + 팀 wiki 페이지 `locked = true` 해제 권한 / C. + 팀 quota 일부 관리 | **A** v1 단순 | 🟡 |

---

## 8. Out of scope

- **Multi-tenant 활성화** — P2C (§9.2)
- **SSO / OIDC / SAML** — P2C
- **LDAP / AD 디렉토리 sync** — P2C
- **Row-level encryption / 외부 KMS** — P3
- **MFA (TOTP/WebAuthn)** — P3
- **RBAC 세분화 (team-admin, billing-viewer 등)** — P3
- **Email 전송 (invite, password reset 링크)** — P2C (별 서비스 의존)
- **Password rotation policy, breach check** — P3
- **Audit log export to SIEM** — P2C

---

## 리뷰 로그 (append-only)

### Round 0 — 2026-04-18 — PM draft
**결론**: Draft v0. identity/user/team 설계 초안.

**체크리스트** (`02-document-template.md` 기준):
- [x] §1 철학 & 컨셉
- [x] §3 스키마 (SQL)
- [x] §6 인증 플로우
- [ ] 템플릿 정합성 — `상세 구현/모듈 연결/단위/통합 테스트` 상위 섹션 미정렬
- [ ] Round 1 reviewer 배정 누락

**다음 단계**: 템플릿 정합성 보강 후 Round 1.

### Round 1 — 2026-04-18 — @gateway-router-lead @xaas-platform-lead
**결론**: Conditional Pass

**체크리스트**:
- [x] 인터페이스 계약
- [x] 크레이트 경계
- [x] 타입 중복
- [x] 에러 반환
- [x] 동시성
- [x] 의존성 방향
- [x] Phase 태그
- [x] 레거시 결정 준수

**Action Items**:
- A1: 템플릿 5대 필수 섹션을 명시적으로 추가
- A2: Round 1.5 보안 리뷰어를 workflow 기준으로 정정

**다음 라운드 조건**: A1, A2 반영 후 Round 1.5.

### Round 1.5 — 2026-04-18 — @security-compliance-lead @dx-product-lead
**결론**: Pass

**체크리스트**:
- [x] 위협 모델
- [x] 신뢰 경계 입력 검증
- [x] 인증·인가
- [x] 시크릿 관리
- [x] 감사 로그
- [x] 에러 메시지 3요소
- [x] 사용자 touchpoint 워크스루
- [x] defaults 안전성
- [x] 하위 호환
- [x] runbook/playbook

**Action Items**:
- A1: bootstrap 재실행 fail-closed 와 session TTL validation 을 명시
- A2: admin/self-demotion 및 service-user session 금지 회귀 테스트 추가

**다음 라운드 조건**: A1, A2 반영 후 Round 2.

### Round 2 — 2026-04-18 — @qa-test-architect
**결론**: Pass

**체크리스트**:
- [x] 단위 테스트 범위
- [x] mock 가능성
- [x] 결정론
- [x] 통합 시나리오
- [x] CI 재현성
- [x] 회귀 테스트
- [x] 테스트 데이터

**Action Items**:
- 없음

**다음 라운드 조건**: Round 3.

### Round 3 — 2026-04-18 — @chief-architect
**결론**: Pass

**체크리스트**:
- [x] Rust 관용구
- [x] 제로 비용 추상화
- [x] 제네릭 vs 트레이트 객체
- [x] 에러 전파
- [x] 의존성 추가
- [x] 관측성
- [x] 문서화

**Action Items**:
- 없음

### 최종 승인 — 2026-04-18 — PM
**결론**: Approved. Round 1/1.5/2/3 통과, P2B 구현 진입 가능.

---

*End of 08-identity-and-users.md draft v0.*
