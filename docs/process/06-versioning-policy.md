# 버전 관리 정책

> **승인**: 2026-04-14 사용자 직접 지시
> **관련 결정 로그**: `D-20260414-01`
> **적용 범위**: 워크스페이스 전 크레이트 (lockstep 버저닝)

---

## 1. 규칙

**Phase N 동안 워크스페이스 버전은 `0.N.X` 라인을 유지한다.**

| Phase | 버전 라인 | 상태 |
|-------|-----------|------|
| Phase 1 (Operations + Execution substrate) | `0.1.X` | 완료 (tag `v0.1.0-phase1`) |
| Phase 2 (Assistant plane + collaboration entry point) | `0.2.X` | **진행 중** |
| Phase 3 (Cluster ops expansion + richer automation) | `0.3.X` | 미정 |
| … | `0.N.X` | … |
| **정식 출시** | **`1.0.0`** | 공개 출시 시점에 명시적으로 bump |

- `1.0.0` 은 **진짜 출시(public launch) 직전**에만 명시적으로 bump 한다. Phase 번호와 무관하게 "제품을 출시한다"는 결정이 있어야 1.0.0 이 된다.
- 0.x 구간 동안 **SemVer 호환성 약속은 없다**. 모든 크레이트는 pre-1.0 으로 간주되며 minor bump(=phase 전환)에서 breaking change 가 허용된다.

## 2. 크레이트 버저닝 — Lockstep

모든 워크스페이스 멤버는 `[workspace.package] version` 을 공유한다 (`version.workspace = true`). 크레이트별 독립 버저닝은 1.0.0 이전에는 도입하지 않는다.

**이유**: 크레이트 경계는 `docs/process/04-decision-log.md` D-12 에서 고정되어 있으나, 내부 타입 흐름이 밀접해 어느 한 크레이트의 breaking change 가 거의 항상 다른 크레이트에 파급된다. 독립 버저닝은 운영 이득 없이 릴리스 노트만 복잡하게 만든다.

## 3. Patch bump (`0.N.X` → `0.N.(X+1)`)

Phase 내부에서 다음에 해당하면 patch 를 올린다:

- 버그 fix, 문서 개선, 성능 튜닝 (공개 API 불변)
- 기존 API 에 **추가만** 되는 변경 (새 variant, 새 field — `#[non_exhaustive]` 로 열려있는 경우)
- 설정 파일(`gadgetron.toml`) 후방호환 추가
- 의존성 patch bump, 보안 fix

Phase 내부에서 **breaking change 가 필요하면** 해당 phase 완료를 기다리지 말고 별도 결정(decision log)으로 해결 방안을 문서화한다. 기본은 "다음 phase 로 미룬다".

## 4. Minor bump (`0.N.X` → `0.(N+1).0`) = Phase 전환

다음 조건이 모두 충족되면 phase 전환 bump 를 한다:

1. 이전 phase 의 모든 Round 1.5 / 2 / 3 리뷰가 승인됨
2. `docs/00-overview.md` 의 phase 상태가 "완료"로 갱신됨
3. git tag `v0.N.0-phaseN` 로 이전 phase 의 최종 커밋을 태깅함
4. PM 승인 + 본 결정 로그에 "Phase N → N+1 전환" 엔트리 추가

Phase 간 breaking change 는 허용된다. 대신 각 crate 의 CHANGELOG(존재 시) 또는 decision log 에 영향받는 공개 API 목록을 명시한다.

## 5. 사전 릴리스 식별자 (선택적)

git tag 에서만 사전 릴리스 suffix 를 쓴다. `Cargo.toml` 의 workspace version 은 suffix 없이 유지한다 (crates.io 공개 전이므로 resolver 혼동이 없음).

| 상황 | git tag 예시 | Cargo.toml 값 |
|------|-------------|---------------|
| Phase 1 최종 | `v0.1.0-phase1` | `0.1.0` |
| Phase 2 진행 중 스냅샷 | `v0.2.0-rc.1` (선택) | `0.2.0` |
| Phase 2 완료 | `v0.2.0-phase2` | `0.2.0` |
| 정식 출시 | `v1.0.0` | `1.0.0` |

## 6. CHANGELOG

현재는 `docs/process/04-decision-log.md` 가 사실상 CHANGELOG 역할을 겸한다. 별도 `CHANGELOG.md` 는 **1.0.0 bump 와 동시에** 도입한다 (keepachangelog.com 포맷).

그 전까지는:
- 릴리스별 SBOM (`docs/process/00-agent-roster.md:211`) 는 git tag 단위로 산출
- 사용자 가시 변경은 decision log 에서 추적

## 7. 현재 상태 (2026-04-14)

- Workspace version: `0.2.0` (Phase 2 작업 대상 라인)
- 이전 phase tag: `v0.1.0-phase1`
- 다음 bump 조건: Phase 2 완료 승인 후 `0.3.0` 또는 정식 출시 결정 시 `1.0.0`
