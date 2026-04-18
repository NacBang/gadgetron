# 개발 프로세스

> **승인**: 2026-04-12 사용자 승인
> **적용**: 모든 스프린트에 적용. 예외 없음.

---

## 1. 스프린트 사이클

```
① 조사 (서브에이전트 병렬)
   └→ 기존 코드/문서 gap 분석
   └→ 구현에 필요한 정보 수집

② 문서화 (담당 서브에이전트가 작성)
   └→ 5 필수 섹션 (철학/구현/연결/단위테스트/통합테스트)
   └→ 모듈 테스트 방안 + 실제 동작 테스트 + 검증 방안
   └→ 구현 결정론 10가지 충족

③ 크로스 리뷰 (서브에이전트 적극 참여)
   └→ Round 1: 도메인 정합성 (2명 병렬)
   └→ Round 1.5: 보안 + 사용성 (2명 병렬)
   └→ Round 2: 테스트 가능성 (qa-test-architect)
   └→ Round 3: Rust 관용구 + 구현 결정론 (chief-architect)
   └→ 각 Round에서 발견된 gap → fix → re-review

④ 조사 (리뷰 결과 반영 확인)
   └→ 완결성 미충족 시 ② 로 복귀
   └→ 충족 시 → Approved

⑤ TDD 구현 (서브에이전트가 수행)
   └→ Red: 실패 테스트 먼저
   └→ Green: 최소 구현
   └→ Refactor
   └→ 서비스 경로를 바꿨다면 `build/start/stop/status/logs` 스크립트 또는 동등 자동화까지 같이 구현

⑥ 리팩토링 (스프린트 종료 후)
   └→ 코드: clippy 0 warnings, fmt, dead code 삭제
   └→ 문서: 코드-문서 불일치 0건
   └→ "더 이상 할 것 없음" 판단까지 반복

⑦ 검증 완료 (구현 후 필수)
   └→ 코드/스크립트 변경 직후 `./verify_cycle.sh changed`
   └→ PR 직전 `./verify_cycle.sh ci`
   └→ 필요한 도메인 smoke/eval 은 추가로 수행
   └→ 최종 보고에 "무엇을 실행했고 무엇을 생략했는지" 명시

⑧ 매뉴얼 업데이트 (push 전 필수)
   └→ docs/manual/ 에 구현된 내용 반영
   └→ 새 endpoint, CLI, config, 에러 코드 모두 매뉴얼에 포함
   └→ 서비스 완전 기동 경로와 운영 루프(build/start/stop/status/logs)도 매뉴얼에 포함
   └→ 매뉴얼에 없는 기능 = push 금지

⑨ PR + Merge
   └→ git commit → gh pr create → merge
   └→ clean state에서 다음 Sprint 시작
```

## 2. 절대 규칙

| # | 규칙 |
|---|---|
| 1 | **문서 없이 구현 없음** — 설계 문서 Approved 후에만 코드 작성 |
| 2 | **3회 이상 크로스 리뷰** — Round 1 → 1.5 → 2 → 3. 서브에이전트가 수행 |
| 3 | **TDD** — Red → Green → Refactor |
| 4 | **구현 결정론** — 누가 와도 같은 결과. TBD/모호함 금지. 10가지 필수 명시 |
| 5 | **서브에이전트 최대한 활용** — PM은 조율. 실행은 서브에이전트 |
| 6 | **스프린트 종료 후 리팩토링** — 더 할 것 없을 때까지 반복 |
| 7 | **PM 권한** — 실행 결정은 PM 자율. 전략적 결정만 사용자 escalation |
| 8 | **테스트 방안 필수** — 모듈 테스트 + 실제 동작 테스트 + 검증 방안 |
| 9 | **도구 설치 자유** — 구현/테스트/검증에 필요한 툴·플러그인·MCP 사전 승인 |
| 10 | **서비스 제공 완결성** — 실행 경로를 바꾸는 작업은 서비스를 끝까지 띄우는 스크립트/자동화와 운영 문서까지 함께 제출해야 완료 |

## 5. 구현 후 검증 계약

1. **구현이 끝나면 바로 `./verify_cycle.sh changed`** 를 실행한다. 이 스크립트는 현재 변경 파일을 기준으로 touched crate를 계산하고, 필요 시 workspace-wide 검증으로 승격한다.
2. **PR 직전에는 `./verify_cycle.sh ci`** 를 실행한다. 로컬에서 재현 가능한 범위의 CI 게이트(`check` / `fmt` / `clippy` / `test`)를 그대로 따라간다. `cargo-deny`가 설치되어 있으면 보안 검사까지 포함한다.
3. **공용/파급 큰 crate 변경은 자동으로 더 넓게 검증한다.** `gadgetron-core`, `provider`, `router`, `gateway`, `xaas`, 루트 manifest, CI workflow 변경은 `changed` 모드에서도 workspace 검증으로 올린다.
4. **Penny/Knowledge/Web 같은 결합 영역은 fan-out 검증을 포함한다.** 예: `gadgetron-penny`/`gadgetron-knowledge` 변경 시 `gadgetron-cli`도 함께 검증하고, `gadgetron-web` 변경 시 `gadgetron-gateway`도 함께 검증한다.
5. **스크립트가 끝이 아니다.** 실제 동작 smoke, eval harness, 수동 UX 확인이 필요한 작업은 해당 검증을 추가로 실행하고, 최종 보고에 명시한다.
6. **실행 경로 변경은 운영 루프까지 검증한다.** 새로 도입/수정한 `build/start/stop/status/logs` 경로 또는 동등 자동화로 서비스가 실제로 올라와야 한다.

## 3. 서브에이전트 역할 분담

| 역할 | 참여 시점 |
|---|---|
| chief-architect | Round 1 + Round 3 (final gate) + 리팩토링 |
| gateway-router-lead | 문서 작성 (gateway) + Round 1 + 구현 |
| inference-engine-lead | Round 1 (provider) + 구현 |
| gpu-scheduler-lead | Round 1 (GPU/VRAM) + 구현 |
| xaas-platform-lead | 문서 작성 (xaas) + Round 1 + 구현 |
| devops-sre-lead | CI/CD + 배포 문서 + Round 1 |
| ux-interface-lead | TUI/Web 구현 + Round 1 |
| qa-test-architect | Round 2 (테스트 가능성) + 테스트 하네스 |
| security-compliance-lead | Round 1.5 (보안) + STRIDE |
| dx-product-lead | Round 1.5 (사용성) + CLI UX + 에러 메시지 |

## 4. 산출물 체인

```
조사 보고서 → 설계 문서 (Approved) → TDD 코드 → 테스트 결과 → 리팩토링 → PR → Merge
```
