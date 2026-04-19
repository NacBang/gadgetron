# 버전 관리 정책

> **최초 승인**: 2026-04-14 (D-20260414-01)
> **현재 개정**: 2026-04-18 — ROADMAP v2 (PR #186) EPIC/ISSUE/TASK 체계로 리프레임
> **적용 범위**: 워크스페이스 전 크레이트 (lockstep 버저닝)
> **관련 문서**: [`docs/ROADMAP.md`](../ROADMAP.md) — T1/T2/T3/T4 정의와 릴리스 태그 목록의 canonical source

---

## 1. 규칙 — EPIC/ISSUE 기반

PR #186 의 ROADMAP v2 에서 버전 라인은 Phase-N 가 아닌 EPIC/ISSUE 에 묶입니다. 요약:

| 작업 단위 | 규모 | 버전 영향 | Git tag |
|-----------|------|-----------|---------|
| **EPIC** (T1) | 1-3 개월 | **minor bump** (`0.N.x` → `0.(N+1).0`) | ✅ `vX.Y.0` (EPIC closure commit) |
| **ISSUE** (T2) | 3-10 일 | **patch bump** (`0.N.X` → `0.N.(X+1)`) | ❌ 없음 |
| **TASK / SUBTASK** (T3 / T4) | 반나절 ~ 하루 | 없음 | ❌ 없음 |

- ISSUE 머지 = 단일 PR · 단일 harness green run. 패치 버전만 올라가고 태그는 없음. `Cargo.toml` 의 `[workspace.package] version` 을 머지 커밋에서 bump.
- EPIC closure = 해당 EPIC 의 모든 ISSUE 가 머지된 뒤, "EPIC closure" 커밋에서 minor bump + `vX.Y.0` 태그.
- 0.x 구간 동안 **SemVer 호환성 약속은 없습니다.** 모든 크레이트는 pre-1.0 으로 간주되며, minor bump (=EPIC closure) 에서 breaking change 가 허용됩니다.
- `1.0.0` 은 **진짜 출시 시점** 에만 명시적으로 bump — ROADMAP v2 에서 **EPIC 4 (Multi-tenant / XaaS) closure** 에 연결되어 있습니다.

## 2. 크레이트 버저닝 — Lockstep

모든 워크스페이스 멤버는 `[workspace.package] version` 을 공유합니다 (`version.workspace = true`). 크레이트별 독립 버저닝은 `1.0.0` 이전에는 도입하지 않습니다.

**이유**: 크레이트 경계는 `docs/process/04-decision-log.md` D-12 에서 고정되어 있으나, 내부 타입 흐름이 밀접해 어느 한 크레이트의 breaking change 가 거의 항상 다른 크레이트에 파급됩니다. 독립 버저닝은 운영 이득 없이 릴리스 노트만 복잡하게 만듭니다.

## 3. Patch bump = ISSUE 머지

ISSUE 한 개가 PR 로 머지될 때 항상 patch 를 올립니다. 다음은 최근 머지된 ISSUE 들의 실례 (`docs/ROADMAP.md` §"Completed ISSUEs"):

| 버전 | ISSUE | 머지 PR |
|------|-------|---------|
| `0.2.0` | ISSUE 1 — usable OpenAI-compat gateway + workbench CRUD | #175/#176/#177/#179 (over-split pre-rule) |
| `0.2.1`-`0.2.4` | ISSUE 2 — workbench UX polish + workflow bootstrap | #180/#181/#182/#184 (over-split pre-rule) |
| `0.2.5` | ISSUE 2b — ROADMAP v2 recalibration | #186 |

ISSUE 범위가 너무 커서 하나의 PR 에 안 들어가면 ISSUE 자체를 분할하는 것이 원칙 — "over-split" 되어 여러 PR 로 머지된 ISSUE 1/2 는 ROADMAP v2 에서 사후적으로 묶었고, 이후에는 규칙을 따릅니다.

Phase 내부에서 **breaking change 가 필요하면** 해당 EPIC closure 를 기다리지 말고 별도 결정(decision log)으로 해결 방안을 문서화합니다. 기본은 "다음 EPIC 으로 미룬다".

## 4. Minor bump = EPIC closure

EPIC 의 마지막 ISSUE 가 머지되는 PR 에서 minor bump + git tag 를 진행합니다. 조건:

1. EPIC 에 속한 모든 ISSUE 가 "Completed ISSUEs" 로 이동되고 머지 PR 번호가 기록됨 (`docs/ROADMAP.md` 의 EPIC 섹션 참조).
2. `docs/ROADMAP.md` 의 해당 EPIC 섹션이 "CLOSED" 로 갱신되고 다음 EPIC 이 "ACTIVE" 로 승격됨.
3. `[workspace.package] version` 이 minor bump: 현재 `0.N.X` → `0.(N+1).0`.
4. git tag 를 EPIC closure 커밋에 찍음: `v0.(N+1).0`.
5. 태그 메시지는 해당 EPIC 의 이름 + 주요 deliverable 을 한 줄로 포함합니다.

EPIC 간 breaking change 는 허용됩니다. 각 크레이트의 CHANGELOG (존재 시) 또는 decision log 에 영향받는 공개 API 목록을 명시합니다.

## 5. 태그 네이밍

EPIC closure 태그만 공식 릴리스입니다. 모두 `vX.Y.Z` 형식 (suffix 없음):

| Tag | 조건 | ROADMAP 소스 |
|-----|------|------------|
| `v0.1.0-phase1` | 역사적 Phase 1 스냅샷 (ROADMAP v2 이전) | 기존 유지 |
| `v0.3.0` | EPIC 1 (Workbench MVP) closure | ROADMAP v2 §EPIC 1 |
| `v0.4.0` | EPIC 2 (Agent autonomy) closure | ROADMAP v2 §EPIC 2 |
| `v0.5.0` | EPIC 3 (Plugin platform) closure | ROADMAP v2 §EPIC 3 |
| `v1.0.0` | EPIC 4 (Multi-tenant / XaaS) closure — **first production release** | ROADMAP v2 §EPIC 4 |
| `v2.0.0` | EPIC 5 (Cluster platform) closure | ROADMAP v2 §EPIC 5 |

**Cargo.toml 의 `version` 은 suffix 없이 유지합니다** (crates.io 공개 전이므로 resolver 혼동이 없음). 사전 릴리스 identifier 가 필요한 경우 git tag 에서만 (`v0.3.0-rc.1` 등) 사용합니다.

## 6. CHANGELOG

현재는 `docs/process/04-decision-log.md` + `docs/ROADMAP.md` "Completed ISSUEs" 섹션이 사실상 CHANGELOG 역할을 겸합니다. 별도 `CHANGELOG.md` 는 **`v1.0.0` bump 와 동시에** 도입합니다 (keepachangelog.com 포맷).

그 전까지는:
- 릴리스별 SBOM (`docs/process/00-agent-roster.md`) 는 EPIC closure git tag 단위로 산출.
- 사용자 가시 변경은 decision log + ROADMAP "Completed ISSUEs" 에서 추적.

## 7. 현재 상태

- **Workspace version**: `0.2.5` (Phase 2A — EPIC 1 진행 중)
- **활성 EPIC**: EPIC 1 (Workbench MVP). 현재까지 shipped — ISSUE 1 (0.2.0), ISSUE 2 (0.2.1→0.2.4), ISSUE 2b (0.2.5). 남은 ISSUE — ISSUE 3 (production safety, NEXT), ISSUE 4 (operator observability).
- **다음 patch bump**: ISSUE 3 머지 시 `0.2.5` → `0.2.6`.
- **다음 minor bump**: EPIC 1 의 ISSUE 4 까지 머지된 뒤 `v0.3.0` 태그와 함께 `0.3.0` bump.
- **이전 tag**: `v0.1.0-phase1` (역사적).
