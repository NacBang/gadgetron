# gadgetron-xaas Phase 2 — tenant self-service implementation plan (ISSUE 14)

> **담당**: @xaas-platform-lead
> **상태**: ISSUE 14 ✅ CLOSED (PR #246 / v0.5.7). ISSUE 15 ✅ CLOSED (PR #248 / v0.5.8). ISSUE 16 ✅ CLOSED (PR #259 / v0.5.9). ISSUE 17 ✅ CLOSED (PR #260 / v0.5.10). ISSUE 19 ✅ CLOSED (PR #262 / v0.5.11). ISSUE 20 ✅ CLOSED (PR #263 / v0.5.12). ISSUE 21 TASK 21.1 (pg audit_log consumer) ✅ CLOSED (PR #267 / v0.5.13). ISSUE 22 TASK 22.1 (admin `/audit/log` query endpoint) ✅ CLOSED (PR #269 / v0.5.14). 남은 스코프: **ISSUE 18** web UI 로그인 폼 (하나만 남음).
> **작성일**: 2026-04-19
> **설계 출처**: `docs/design/phase2/08-identity-and-users.md` (approved 2026-04-18)
> **이 문서**: 상위 설계(08) → 구체적 마이그레이션/엔드포인트/TASK 분해로 브리징
> **Phase**: [P2] — ISSUE 14 of EPIC 4

---

## 1. 왜 브리징 문서?

`08-identity-and-users.md` 는 완전한 스키마 + REST + CLI 를 규정한 **정책**
문서다. 이 문서는 **구현 경로**만 다룬다: 어떤 파일에 어떤 TASK가 들어가는지,
TASK 순서는 무엇인지, ISSUE 14 close까지의 패치 버전 경로.

## 2. TASK 분해

| TASK | 결과물 | 상태 |
|------|-------|-----|
| **14.1** users/teams/sessions 마이그레이션 + api_keys 확장 + audit_log 확장 | `20260420000004_users_teams_sessions.sql` | ✅ PR #246 |
| **14.2** Bootstrap flow — `users` 비었을 때 `[auth.bootstrap]` config + `GADGETRON_BOOTSTRAP_ADMIN_PASSWORD` 환경변수로 첫 admin 생성 | `gadgetron-xaas/src/auth/bootstrap.rs` + CLI wiring | ✅ PR #246 |
| **14.3** Admin user CRUD 엔드포인트 — `GET/POST/DELETE /admin/users/*` | `crates/gadgetron-xaas/src/identity.rs` + `gadgetron-gateway/src/web/workbench.rs` | ✅ PR #246 |
| **14.4** User self-service API key 엔드포인트 — `GET/POST /keys`, `DELETE /keys/{id}` | `crates/gadgetron-xaas/src/identity_keys.rs` + gateway | ✅ PR #246 |
| **14.5** Teams + team_members CRUD — `GET/POST/DELETE /admin/teams/*` + `/members/*` | `crates/gadgetron-xaas/src/teams.rs` + gateway | ✅ PR #246 |
| **14.6** → **ISSUE 15** web UI 세션 로그인 — `/auth/login`, `/auth/logout`, `/auth/whoami` | `crates/gadgetron-xaas/src/sessions.rs` + `crates/gadgetron-gateway/src/auth_session.rs` | ✅ PR #248 (ISSUE 15 close) |
| **14.7** CLI subcommands — `gadgetron user {create,list,delete}`, `gadgetron team {create,list,delete}` | `gadgetron-cli` main.rs | ✅ PR #246 |

**CLOSED**: ISSUE 14 (PR #246 / v0.5.7) + ISSUE 15 (PR #248 / v0.5.8) + ISSUE 16 TASK 16.1 단일 middleware (PR #259 / v0.5.9) + ISSUE 17 TASK 17.1 `ValidatedKey.user_id` plumbing (PR #260 / v0.5.10) + ISSUE 19 TASK 19.1 `AuditEntry` actor fields structural (PR #262 / v0.5.11) + ISSUE 20 TASK 20.1 TenantContext → AuditEntry plumbing (PR #263 / v0.5.12) + ISSUE 21 TASK 21.1 pg `audit_log` consumer (PR #267 / v0.5.13, Gate 7v.7 +2) + ISSUE 22 TASK 22.1 admin `/audit/log` query endpoint (PR #269 / v0.5.14, Gate 7v.8 +2). Harness progression: 126 → 129 (PR #259 ISSUE 16 Gate 7v.6) → 131 (PR #267 ISSUE 21 Gate 7v.7) → 133 (PR #269 ISSUE 22 Gate 7v.8). ISSUE 17 + 19 + 20 은 behavior-preserving 변경으로 새 gate 없이 기존 가드에 의존.

남은 multi-user 스코프 (post-PR-#269):
- **ISSUE 18**: web UI 로그인 form (React/Tailwind in `gadgetron-web`) — 사용자가 `/web` 방문 시 cookie 없으면 로그인 폼으로 리다이렉트. Playwright E2E gate 7v.9 (7v.7 + 7v.8 이 ISSUE 21/22 에 선점되었으므로) 이 login → shell render → logout → back-to-form 루프를 검증 예정.
- Session rotation on cookie refresh — 현재 whoami 가 `last_active_at` 만 갱신; 주기적 session token rotation 은 post-v1.0.0 보안 강화 항목.
- Google OAuth 소셜 로그인 — `project_multiuser_login_google` 로 tracked; ISSUE 18 이후 stack up.
- Session rotation on cookie refresh — 현재 whoami 가 `last_active_at` 만 갱신; 주기적 session token rotation 은 post-v1.0.0 보안 강화 항목.
- Google OAuth 소셜 로그인 — `project_multiuser_login_google` 로 tracked; ISSUE 18 이후 stack up.

## 3. TASK 14.1 마이그레이션 (배포 상태)

`crates/gadgetron-xaas/migrations/20260420000004_users_teams_sessions.sql`:
- `users` (UUID PK, tenant_id default = P2B 고정 UUID, email unique per-tenant, role CHECK 3-values, argon2id hash nullable for service)
- `teams` (kebab-case TEXT PK, CHECK regex `^[a-z][a-z0-9-]{0,31}$`, 'admins' reserved)
- `team_members` (team_id + user_id composite PK, role enum)
- `user_sessions` (cookie_hash = SHA-256, expires_at + idle rotation)
- `ALTER TABLE api_keys` — `user_id UUID REFERENCES users(id)` nullable (14.3에서 backfill 후 NOT NULL 전환은 `20260420000005` reserved)
- `ALTER TABLE api_keys` — `label TEXT` (사용자가 붙이는 라벨: "ci-deploy", "alice-laptop")
- `ALTER TABLE audit_log` — `actor_user_id`, `actor_api_key_id`, `impersonated_by`, `parent_request_id`

**왜 user_id nullable?** 기존 키 rows에 user_id를 채워넣으려면 bootstrap admin user가 먼저 존재해야 한다. 14.2 이후 backfill → 14.3 에 NOT NULL 전환.

**왜 별도 마이그레이션으로 분리 안 했나?** 초기 스키마이므로 한 트랜잭션으로 깔아도 안전. idempotent CREATE TABLE IF NOT EXISTS + ADD COLUMN IF NOT EXISTS 를 썼기 때문에 재실행도 무해.

## 4. 테스트 전략 (Round 2)

- **Unit (in-module)**: argon2id 해시 검증 round-trip (14.2), session cookie 만료 로직 (14.6), team id regex CHECK (pg-driven)
- **Integration (PostgresFixture)**: user-team-key-session 체인 round-trip; RBAC (member cannot delete admin)
- **Harness gates** (per TASK):
  - 7v.1 — `POST /api/v1/users` admin creates user, member-key 403 (TASK 14.3)
  - 7v.2 — user self-service `POST /api/v1/keys` → 새 키 wire shape + 한 번만 노출 (TASK 14.4)
  - 7v.3 — `DELETE /api/v1/keys/{id}` revoke + 직후 인증 시도 401 (TASK 14.4)
  - 7v.4 — team CRUD roundtrip (TASK 14.5)
  - 7v.5 — bootstrap flow: 빈 users로 serve 시작 시 첫 admin 생성됨 (TASK 14.2)

## 5. 보안 고려 (08 §4 요약)

- **Password storage**: argon2id (libsodium/password-hash crate). bcrypt/PBKDF2 금지.
- **Key rotation**: revoke-then-create (새 키 생성 → 구 키 revoke, 둘을 동시에 노출하지 않음)
- **Single-admin guard**: 마지막 admin이 자신을 member로 강등하거나 삭제 시 400 error. 08 §7 Q-1.
- **Session cookie**: `HttpOnly; Secure; SameSite=Lax; Path=/`. 24h TTL, 2h idle rotation.
- **Bootstrap fail-loud**: empty users + no bootstrap config → serve 시작 거부. 중복 bootstrap → serve 시작 거부.

## 6. 참조

- `docs/design/phase2/08-identity-and-users.md` — 스펙 원전
- `docs/design/xaas/phase1.md` — 기반 레이어 (tenants + api_keys + audit_log)
- ROADMAP §EPIC 4 / ISSUE 14
