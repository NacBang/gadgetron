# 버전 정책 (Versioning Policy)

> **승인**: 2026-04-13 (초안, 사용자 승인 대기)
> **적용 대상**: 워크스페이스 루트 `Cargo.toml` `[workspace.package].version` + 모든 크레이트 + 릴리스 태그 + 매뉴얼
> **원칙**: 단일 바이너리 `gadgetron` 은 워크스페이스 전체가 하나의 릴리스 단위이다. 10개 크레이트 개별 배포 계획 없음.

---

## 1. Semantic Versioning (SemVer) + Phase marker

### 1.1 공식

```
MAJOR.MINOR.PATCH[-PHASE_MARKER][+BUILD]
```

| 컴포넌트 | 의미 | 증가 트리거 |
|---|---|---|
| **MAJOR** | 호환 불가 변경 | `gadgetron.toml` TOML 스키마 파괴적 변경, OpenAI 호환 API contract breaking, DB 마이그레이션 중 irreversible, `LlmProvider` trait breaking, CLI 서브커맨드 삭제/개명 |
| **MINOR** | 하위 호환 기능 추가 | 신규 provider, 신규 routing strategy, 신규 CLI 서브커맨드, 신규 `gadgetron.toml` 섹션 (기본값 유지), 신규 `GadgetronError` variant (`#[non_exhaustive]` 덕분에 breaking 아님), 신규 크레이트 추가 |
| **PATCH** | 버그 수정 / 순수 내부 리팩토링 | 성능 개선, 문서, 테스트 추가, 의존성 범위 내 업그레이드 |
| **PHASE_MARKER** | Phase 1/2 경계 명시 | `-phase1`, `-p2a`, `-p2b`, `-p2c`, `-p2d`, `-rc.N` (release candidate) |

### 1.2 Phase marker 규칙

- **필수 시점**: 각 Phase 의 첫 릴리스 + 마지막 릴리스.
- **부가적 시점**: Phase 중간 마일스톤을 태그로 남길 때.
- **SemVer 순서 보장**: pre-release 문자열은 SemVer 규약에 의해 `0.1.0 < 0.1.0-phase1 < 0.1.0` 이 되지 않는다. 따라서 `-phaseN` 은 항상 `X.Y.Z-phaseN`을 **사용하고 이후 아무 태그도 붙지 않은 `X.Y.Z`가 최종 릴리스**가 되는 패턴을 금지한다. 대신:
  - Phase baseline (구현 freeze 시점): `X.Y.0-phaseN` — pre-release 취급
  - Phase 정식 릴리스 (매뉴얼/CHANGELOG 완결): `X.(Y+1).0` 또는 `(X+1).0.0`
  - 예: `0.1.0-phase1` (baseline) → `0.2.0` (Phase 1 정식 + Phase 2A 준비 시작)

### 1.3 범위 판정 예시

| 변경 | 구분 |
|---|---|
| `gadgetron-kairos` 크레이트 신설 + `LlmProvider` 구현 등록 | MINOR |
| OpenAI 호환 `finish_reason` 값 새로 추가 | MINOR (클라이언트 spec 확장) |
| `gadgetron.toml` `[providers.*]` 에 `timeout_ms` 기본값 변경 | MAJOR 후보 — 동작 변경이 관측 가능하면 MAJOR |
| `api_keys` 테이블 컬럼 추가 (`ADD COLUMN ... NULL`) | MINOR (reversible) |
| `api_keys` 테이블 컬럼 DROP | MAJOR |
| Gemini provider bug fix | PATCH |
| `GadgetronError::Kairos` variant 추가 (이미 `#[non_exhaustive]`) | MINOR |
| `KairosErrorKind` enum 에 variant 추가 | MINOR (non_exhaustive 유지 시) |
| clippy warning cleanup | PATCH |

---

## 2. 릴리스 트레인 (Release Train)

### 2.1 현재 상태 (2026-04-13)

| 릴리스 | 스코프 | 태그 존재 여부 | 비고 |
|---|---|:---:|---|
| `0.1.0-phase1` | Phase 1 baseline (Sprint 1–9 + hotfix #7–10 + Phase 2 설계 land) | ❌ 미생성 | 현재 드리프트 — 본 문서 승인 후 별도 릴리스 PR로 생성 |
| `0.2.0-p2a` | Phase 2A Kairos MVP baseline | ❌ | 구현 후 |
| `0.2.0-p2b` | Phase 2B Rich Knowledge baseline | ❌ | |
| `0.2.0-p2c` | Phase 2C Multi + Storage baseline | ❌ | |
| `0.2.0-p2d` | Phase 2D Media + Polish baseline | ❌ | |
| `0.3.0` | Phase 2 정식 — 매뉴얼 · CHANGELOG · 배포 가이드 완결 | ❌ | |
| `1.0.0` | Phase 3 프로덕션 — K8s/Slurm/멀티 리전/SLA | ❌ | |

### 2.2 릴리스 PR 체크리스트 (9 항목, 전 항목 충족해야 태그)

1. [ ] `Cargo.toml` `[workspace.package].version` 업데이트
2. [ ] `Cargo.lock` 재생성 (`cargo update -p gadgetron-core` 류 없이 clean 재생성)
3. [ ] `cargo fmt --all -- --check` pass
4. [ ] `cargo clippy --workspace --all-targets -- -D warnings` pass
5. [ ] `cargo test --workspace` 전부 pass (실행된 테스트 수 CHANGELOG 에 기록)
6. [ ] `cargo audit` + `cargo deny check licenses bans advisories` pass
7. [ ] `CHANGELOG.md` 에 해당 릴리스 섹션 추가 (Keep-a-Changelog 포맷)
8. [ ] `docs/manual/` 업데이트 (발전 중인 CLI/config/endpoint/error code)
9. [ ] `docs/00-overview.md` 최신화 (에러 variant, 크레이트 수, Phase 상태)

### 2.3 태그 생성 원칙

- **Annotated tag only** — `git tag -a vX.Y.Z[-phase] -m "..."`. lightweight tag 금지.
- 태그는 **main 브랜치의 머지 커밋**에만 부여.
- Signed tag (`-s`) 권장, 필수는 아님.
- 태그 push 는 별도 명령으로 — `git push origin vX.Y.Z-phase`.
- 태그 이후 즉시 다음 개발 버전 마커로 `Cargo.toml` 을 올린다 — 예: `0.1.0-phase1` 태그 직후 `Cargo.toml` 을 `0.2.0-dev` 로. 태그 생성 이력과 dev 순환을 구분.

### 2.4 태그 철회(retract) 정책

- 태그가 공개 리모트에 push 된 이후에는 **삭제하지 않는다**. 대체 태그를 새로 발행하고 이전 태그는 CHANGELOG 에 "retracted: <사유>" 로만 표기.
- main 에 병합된 커밋에 붙인 태그는 CHANGELOG 와 매뉴얼에서 참조되는 **immutable 아카이브**.

---

## 3. 크레이트 버전 정책

### 3.1 Workspace 버전 단일 관리

- 모든 10개 크레이트가 `workspace.package.version` 을 상속하여 단일 버전을 공유.
- **개별 크레이트 독립 배포 금지** (crates.io 퍼블리시는 Phase 3 `1.0.0` 이후 재검토).
- 예외: 외부 의존성 문제로 1개 크레이트만 긴급 PATCH 가 필요한 경우 — 전체 워크스페이스 버전을 올려 함께 릴리스.

### 3.2 크레이트 간 의존성 명시

```toml
# Cargo.toml — workspace dependencies
gadgetron-core = { path = "crates/gadgetron-core" }
```

- `path` 기반만 허용 (`version = "..."` 병기 금지, crates.io 퍼블리시 전까지).
- `1.0.0` 이후 crates.io 퍼블리시 고려 시점에 `version` 필드 추가 + `path` 병기.

### 3.3 MSRV (Minimum Supported Rust Version)

- 현재 `rust-version = "1.80"`.
- MSRV bump 은 **MINOR** 이상으로 릴리스 (`cargo install` 하는 operator 에게 영향 있음).
- `rust-toolchain.toml` 은 추가하지 않음 — operator 의 시스템 toolchain 존중.

---

## 4. API / 스키마 호환성 계약

### 4.1 OpenAI 호환 API (Phase 1 에서 frozen)

다음 엔드포인트는 **breaking change 금지** (MAJOR 없이는 형태 변경 불가):

- `POST /v1/chat/completions` — request body + response + SSE chunk 형태
- `GET  /v1/models`
- `GET  /health`, `GET /ready`
- 에러 응답 shape: `{ "error": { "message": "...", "type": "...", "code": "..." } }`

### 4.2 관리 API (`/api/v1/*`)

- Phase 1 에서는 **MINOR-level** 호환성만 보장 (경로 불변, 응답 필드 추가 가능).
- `1.0.0` 이후 MAJOR 호환 수준으로 승격.

### 4.3 `gadgetron.toml` 스키마

- 필드 추가: MINOR (기본값 제공 의무)
- 필드 삭제 / 이름 변경: MAJOR
- 기본값 semantic 변경: MAJOR
- `${ENV}` placeholder 규약 변경: MAJOR

### 4.4 PostgreSQL 스키마 (sqlx-cli)

- 마이그레이션은 append-only. 기존 마이그레이션 수정 금지.
- reversible migration (UP/DOWN 모두 작성) 권장. 불가능한 경우 (예: DROP COLUMN) MAJOR 로 분류하고 backup/rollback runbook 을 매뉴얼에 추가.
- 마이그레이션 파일명은 `YYYYMMDDHHMMSS_<snake_case_desc>.sql`.

### 4.5 CLI 서브커맨드

- 추가: MINOR
- 인자 추가 (optional, 기본값 존재): MINOR
- 인자 추가 (required): MAJOR
- 서브커맨드 삭제/개명: MAJOR
- 출력 포맷 (human-readable): MINOR (파싱 의존 금지 명시)
- 출력 포맷 (`--json` 등 machine-readable): MAJOR 호환성 보장

---

## 5. CHANGELOG

### 5.1 형식

[Keep a Changelog 1.1](https://keepachangelog.com/) 준수.

```markdown
## [0.2.0-p2a] — 2026-05-XX
### Added
- `gadgetron-knowledge` crate (wiki + SearXNG + MCP stdio server)
- `gadgetron mcp serve` / `gadgetron kairos init` 서브커맨드

### Changed
- `GadgetronError::error_code()` return type `&'static str` → `String`

### Fixed
- ...

### Security
- M1 tempfile atomic 0600, M2 stderr redaction
```

### 5.2 작성 책임

- Sprint 종료 시 @dx-product-lead 가 CHANGELOG 섹션 초안 작성
- Release PR 에서 PM 이 최종 확인

---

## 6. 현재 상태(2026-04-13)에 대한 조치

Phase 1 이 구현상 완료되었지만 태그가 존재하지 않아 드리프트가 있다. 본 정책 승인 후 다음 순서로 처리:

1. 별도 릴리스 PR 생성 — "chore(release): baseline 0.1.0-phase1"
2. `Cargo.toml` 버전을 `0.1.0` → `0.1.0-phase1` 로 일시 설정 (또는 `0.1.0` 을 그대로 `0.1.0-phase1` 로 태그)
3. `CHANGELOG.md` 신규 파일 생성 — Sprint 1–9 + hotfix #7–10 을 Phase 1 섹션으로 통합
4. §2.2 체크리스트 9 항목 pass 확인
5. `git tag -a v0.1.0-phase1 -m "Phase 1 baseline — 10 crates, 6 providers, 6 routing strategies, XaaS Phase 1, TUI"`
6. `git push origin v0.1.0-phase1`
7. 다음 개발 버전으로 `Cargo.toml` 을 `0.2.0-dev` 로 설정 (Phase 2A 구현 시작)

**이 릴리스 PR 의 오너**: @devops-sre-lead (CI/CD 담당). PM 이 합의 후 발주.

---

## 7. 참조

- [SemVer 2.0.0](https://semver.org/spec/v2.0.0.html)
- [Keep a Changelog 1.1](https://keepachangelog.com/en/1.1.0/)
- `docs/process/05-development-process.md` §1 ⑧ — PR + Merge 단계에서 이 정책을 호출
- `docs/process/04-decision-log.md` — D-1~D-13 + D-20260411-* + D-20260412-* 의 호환성 결정 기록부
