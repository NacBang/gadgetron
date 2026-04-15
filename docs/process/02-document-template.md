# 설계 문서 필수 템플릿

> 이 템플릿을 따르지 않는 문서는 리뷰 대기열에 올릴 수 없다.
> 모든 섹션은 **필수**. 해당 없음은 "N/A + 이유" 명시.

---

## 저장 경로

```
docs/design/<domain>/<task>.md
```

`<domain>` 예시: `core/`, `gateway/`, `router/`, `provider/`, `scheduler/`, `node/`, `tui/`, `xaas/`, `ops/`, `web/`

---

## 템플릿 (복사해서 사용)

```markdown
# <제목>

> **담당**: @<서브에이전트 이름>
> **상태**: Draft | Round 1 | Round 1.5 | Round 2 | Round 3 | Approved | Implemented
> **작성일**: YYYY-MM-DD
> **최종 업데이트**: YYYY-MM-DD
> **관련 크레이트**: `gadgetron-<crate>`, …
> **Phase**: [P1] / [P2] / [P3]

---

## 1. 철학 & 컨셉 (Why)

- 이 기능이 해결하는 문제 (한 문장)
- 제품 비전(`docs/00-overview.md §1`)과의 연결
- 고려한 대안과 채택하지 않은 이유
- 핵심 설계 원칙과 trade-off

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API

Rust 타입·트레이트·함수 시그니처를 코드 블록으로 명시.

\`\`\`rust
pub trait ... { ... }
pub struct ... { ... }
\`\`\`

### 2.2 내부 구조
- 데이터 구조, 동시성 모델 (`RwLock` / `DashMap` / `tokio::sync::mpsc` 등 선택 이유)
- 상태머신 (해당 시)
- 주요 알고리즘

### 2.3 설정 스키마
TOML 섹션 예시 + 기본값 + 검증 규칙.

### 2.4 에러 & 로깅
- `GadgetronError` variant 사용 (신규 variant 필요 시 명시)
- `tracing` span 이름·레벨·필드
- STRIDE threat model 요약 (자산 / 신뢰 경계 / 위협 / 완화)

### 2.5 의존성
- 추가할 crate 목록 + 버전 + 정당화

## 3. 전체 모듈 연결 구도 (Where)

- 상위 의존 크레이트 → 이 모듈 → 하위 의존 크레이트
- 데이터 흐름 다이어그램 (ASCII)
- 타 서브에이전트 도메인과의 인터페이스 계약
- `docs/reviews/pm-decisions.md` **D-12 크레이트 경계표** 준수 여부

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위
테스트 대상 함수/타입 목록과 각각이 검증할 invariant.

### 4.2 테스트 하네스
- mock · stub · fixture 전략
- property-based test 필요 여부

### 4.3 커버리지 목표
- Line/branch 수치 목표

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위
- 함께 테스트할 크레이트
- e2e 시나리오 (API 호출 → 응답 검증)

### 5.2 테스트 환경
- 필요한 외부 의존성 (Postgres, mock LLM, 가짜 GPU 노드)
- docker-compose / testcontainers 설정

### 5.3 회귀 방지
- 어떤 변경이 이 테스트를 실패시켜야 하는가

## 6. Phase 구분

섹션 또는 필드 단위로 `[P1]`/`[P2]`/`[P3]` 태그 부여.

## 7. 오픈 이슈 / 의사결정 필요

| ID  | 내용 | 옵션 | 추천 | 상태 |
|-----|------|------|------|------|
| Q-1 | …    | A / B / C | A | 🟡 사용자 승인 대기 |

---

## 리뷰 로그 (append-only)

### Round 1 — YYYY-MM-DD — @reviewer1 @reviewer2
**결론**: Pass / Fail / Conditional Pass

**체크리스트**: (`03-review-rubric.md §1` 기준)
- [x] 인터페이스 계약
- [ ] 크레이트 경계 — 문제: …

**Action Items**:
- A1: …

**다음 라운드 조건**: …

### Round 1.5 — YYYY-MM-DD — @security-compliance-lead @dx-product-lead
**결론**: …
(`03-review-rubric.md §1.5` 기준)

### Round 2 — YYYY-MM-DD — @qa-test-architect
**결론**: …
(`03-review-rubric.md §2` 기준)

### Round 3 — YYYY-MM-DD — @chief-architect
**결론**: …
(`03-review-rubric.md §3` 기준)

### 최종 승인 — YYYY-MM-DD — PM
```

---

## 주의 사항

- 5대 섹션(철학/구현/연결/단위테스트/통합테스트) 중 하나라도 비어있으면 리뷰 진입 불가.
- "오픈 이슈" 표는 비어있어도 해당 섹션 자체는 유지.
- 리뷰 로그는 절대 덮어쓰지 않는다 (append-only).
