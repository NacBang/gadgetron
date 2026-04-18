# 문서 권위 체계와 정합성 정리 규칙

> **상태**: Active
> **승인**: 2026-04-18 사용자 승인
> **적용 범위**: `README.md`, `docs/`, seed docs, 구현 세션, 10분 자동 문서화 루프

---

## 1. 목적

Gadgetron 문서는 설계/ADR/결정 로그/매뉴얼/개요 문서가 함께 진화한다. 이 구조는 유연하지만, 권위 순서와 reconciliation 규칙이 없으면 아래와 같은 드리프트가 발생한다.

- ADR/결정 로그는 최신인데 `README.md`, `docs/00-overview.md`, `docs/architecture/glossary.md` 가 뒤처짐
- 설계 문서는 새 용어를 쓰는데 seed/manual/runbook 이 옛 용어를 유지
- 현재 구현, 목표 구조, 레거시 초안이 서로 섞여 독자가 같은 질문에 문서마다 다른 답을 읽게 됨

이 문서는 **어떤 문서가 더 권위적인지**, 그리고 **충돌을 어떻게 정리해야 하는지**를 고정한다.

---

## 2. 권위 순서

문서 충돌 시 아래 순서가 우선한다.

1. **사용자 직접 지시**
2. **ADR (`docs/adr/`)**
3. **결정 로그 (`docs/process/04-decision-log.md`)**
4. **활성 설계 문서 (`docs/design/`, status가 Draft/Round/Approved/Implemented 인 현재 문서)**
5. **제품 개요/용어 문서**
   - `README.md`
   - `docs/00-overview.md`
   - `docs/architecture/glossary.md`
6. **사용자/운영자/개발자 매뉴얼**
   - `docs/manual/`
   - `docs/modules/`
   - `docs/process/`
7. **seed 문서 / 예시 / 리뷰 로그 / archive**

하위 계층 문서는 상위 계층 문서와 충돌하면 **버그**다.

---

## 3. 문서 종류별 역할

### 3.1 ADR / Decision log

- 구조, 용어, 경계, lifecycle 같은 **정답**을 고정한다.
- 하위 문서가 따라와야 한다.
- 단독으로 방치하면 안 된다. downstream sweep 의 시작점이다.

### 3.2 설계 문서

- 특정 기능/영역의 canonical spec 이다.
- ADR/decision-log 를 구체화하되 반대로 덮어쓰지 않는다.
- 레거시 초안이면 status 와 superseded note 를 강하게 표기해야 한다.

### 3.3 Overview / Glossary / README

- 독자가 가장 먼저 읽는 entrypoint 다.
- 따라서 stale state 허용 범위가 가장 낮다.
- ADR/decision-log 변경이 생기면 가장 먼저 sweep 대상이 된다.

### 3.4 Manual / Runbook / Developer docs

- 실제 운영/사용/개발 경로를 설명한다.
- 문서가 설명하는 명령, 스크립트, URL, 상태 확인 경로는 실제로 동작해야 한다.

---

## 4. 정합성 정리 규칙

### 4.1 결정 변경 시 downstream sweep 의무

용어/경계/구조 결정이 바뀌면 같은 작업 흐름에서 최소 아래 문서를 함께 검토한다.

- `README.md`
- `docs/00-overview.md`
- `docs/architecture/glossary.md`
- 관련 설계 문서
- 관련 manual / runbook / process docs

### 4.2 “현재 구현”과 “목표 구조”를 분리 표기

둘이 다를 때는 문서에 반드시 둘 다 적는다. 다만 구현자가 헷갈리지 않도록 **canonical answer를 먼저** 적고, 현재 코드 위치는 보조 정보로만 적는다.

- **Canonical ownership / rule**
- **Current code location** (필요할 때만)

이 구분 없이 한 문장으로 섞어 쓰지 않는다. 특히 구현자가 읽는 문서(`README.md`, `docs/00-overview.md`, `docs/architecture/glossary.md`, active design docs)는 “현재 디렉토리에 있으니 core인가 보다” 같은 추정을 유도하면 안 된다.

### 4.3 legacy 문서는 강하게 표시

아직 재작성되지 않은 문서는 다음 중 하나를 반드시 가져야 한다.

- `legacy filename`
- `superseded by ADR/decision log`
- `not source of truth`

### 4.4 용어는 canonical vocabulary 사용

제품/아키텍처 용어는 다음만 canonical 이다.

- **Bundle**
- **Plug**
- **Gadget**

아래는 금지 또는 legacy-only 다.

- `plugin`
- `backend plugin`
- `tool provider`
- `MCP plugin`

예외:

- 외부 생태계 고유 명칭 (`device plugin`, npm plugins, Tailwind plugin)
- 역사적 인용
- 호환성 필드 (`plugin`, `plugin_version`) 의 migration note

### 4.5 실행 경로 문서는 runnable 이어야 한다

실행/배포/데모/운영 문서는 다음 중 하나를 설명해야 한다.

- `build`
- `start`
- `stop`
- `status`
- `logs`

또는 그와 동등한 자동화/운영 루프.

---

## 5. 10분 자동 문서화 루프 규칙

문서 정합성 backlog 가 남아 있는 동안 자동 루프는 **reconciliation-only** 모드로 동작한다.

- 새 설계 주제 확장보다 기존 충돌 해소를 우선
- audience rotation 은 유지하되, 대상은 “불일치 해소”가 우선
- 한 번의 실행은 하나의 conflict cluster 를 정리하고 PR 하나만 머지
- backlog 가 0 이라고 명시적으로 확인되기 전까지는 “새 문서 추가”보다 “기존 문서 정렬”을 우선

---

## 6. 완료 기준

다음 조건을 모두 만족해야 “문서 정합성 gate 통과”로 본다.

1. `README.md`, `docs/00-overview.md`, `docs/architecture/glossary.md` 가 최신 ADR/decision-log 와 충돌하지 않음
2. legacy 설계 문서가 있으면 superseded/legacy 표기가 명확함
3. 사용자/운영자/개발자 문서의 명령/스크립트/런북이 실제 운영 경로와 맞음
4. 같은 질문에 대해 서로 다른 상위 문서가 상반된 답을 주지 않음
5. active reconciliation tracker 의 open item 이 0 임

---

## 7. 관련 문서

- `docs/process/04-decision-log.md`
- `docs/reviews/document-consistency-sweep-2026-04-18.md`
- `docs/adr/ADR-P2A-10-bundle-plug-gadget-terminology.md`
