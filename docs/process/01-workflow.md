# 작업 워크플로우

> PM 주재: 메인 에이전트
> 원칙: 문서 없이는 구현 없다. Round 1 / 1.5 / 2 / 3 리뷰 없이는 머지 없다.

---

## 단계 0 — 태스크 발주 (PM)

PM이 작업을 식별하고 담당 서브에이전트를 지명한다.

- `TaskCreate`로 태스크 등록
- `owner` 메타데이터에 담당 서브에이전트 이름 기재
- 착수 전 scope, 관련 기존 문서(`docs/reviews/`, `docs/modules/`) 전달

---

## 단계 1 — 설계 문서 작성 (담당 서브에이전트)

담당자가 `docs/design/<domain>/<task>.md`를 작성한다.

- 템플릿: [`02-document-template.md`](02-document-template.md)
- 5대 필수 섹션이 없는 문서는 리뷰 대기열에 올릴 수 없음
- 타 모듈 인터페이스에 의존하면 PM에게 협업 요청
- 결정이 필요한 trade-off는 문서의 "오픈 이슈" 표에 등록

---

## 단계 2 — 크로스 리뷰 (Round 1 / 1.5 / 2 / 3)

PM이 주재하는 Round 1 · 1.5 · 2 · 3 리뷰를 순차 진행한다.

| 라운드 | 리뷰어 | 관점 | 기준 |
|--------|--------|------|------|
| **Round 1** | PM이 선정한 타 도메인 서브에이전트 **2명** | 인터페이스 정합성 · 크레이트 경계 · 타입 일관성 | [`03-review-rubric.md`](03-review-rubric.md) §1 |
| **Round 1.5** | `security-compliance-lead` + `dx-product-lead` | 위협 모델 · 승인 경계 · touchpoint · runbook usability | 동 문서 §1.5 |
| **Round 2** | `qa-test-architect` | 테스트 가능성 · 단위/통합 시나리오 | 동 문서 §2 |
| **Round 3** | `chief-architect` | Rust 관용구 · 에러 모델 · 의존성 흐름 | 동 문서 §3 |

- 리뷰 결과는 해당 설계 문서의 `## 리뷰 로그` 섹션에 append-only로 기록
- 한 라운드에서 Fail/Conditional Pass가 나오면 반영 후 그 라운드를 **재수행**
- 큰 변경이 생기면 PM이 이전 라운드도 재소집 가능
- Round 1.5는 보안·사용성 리스크가 없는 순수 내부 리팩터가 아니라면 기본 포함한다
- 4개 라운드는 **최소 기본선**이다. 필요하면 더 진행

---

## 단계 3 — 결정 수렴 & 에스컬레이션

- 팀 내 합의 가능한 피드백 → 문서 수정
- 합의 불가 또는 사용자 판단 필요 → PM이 [`04-decision-log.md`](04-decision-log.md)에 등록
- 사용자 승인 후 반영 → 해당 라운드 재수행

---

## 단계 4 — 구현 착수

문서가 Round 1/1.5/2/3 전부 통과하고 **"Approved"** 상태가 되어야 구현 가능.

- 구현은 담당 서브에이전트가 수행
- 커밋 메시지에 `design: docs/design/<path>.md` 참조
- 단위 + 통합 테스트 없이 제출된 코드는 미완성으로 반려

---

## 단계 5 — 검증 (qa-test-architect + PM)

`qa-test-architect`가 실제 테스트 실행 결과를 검토한다.

- 단위 테스트 통과
- 통합 테스트 통과
- 설계 문서의 "테스트 계획"과 실제 테스트 일치 여부 확인

최종 PM sign-off 후 문서 상태를 **"Implemented"**로 변경.

---

## 예외: 긴급 수정 (Hotfix)

기존 코드의 버그에 한정:

- 설계 문서 없이 Round 1 한 번만으로 패치 가능
- 사후에 `docs/design/hotfixes/<date>.md`로 기록
- **신규 기능에는 적용 불가**
