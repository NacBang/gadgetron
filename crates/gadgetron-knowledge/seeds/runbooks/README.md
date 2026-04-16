---
tags = ["runbooks", "template"]
type = "index"
source = "seed"
plugin = "gadgetron-core"
plugin_version = "0.3.0"
---

# Runbooks

반복되는 작업·장애 대응 절차를 기록하는 폴더입니다. 코드화할 수 있다면 코드로, 그전까지는 사람이 따라할 수 있는 체크리스트로 남깁니다.

## 왜 기록하는가

한 번 해본 작업을 두 번째 할 때 처음부터 조사하지 않기 위해서입니다. 장애 대응 중에는 생각할 시간이 없으니 절차가 문서로 있어야 합니다.

## 작성 시점

- 30분 이상 걸려서 알아낸 절차
- 새벽 3시에 깨워서 대응했던 장애
- 월 1회 이상 반복되는 작업
- 손 탄 순서대로 해야 성공하는 작업

## 파일 이름

`runbooks/<주제>.md` 또는 `runbooks/<카테고리>/<주제>.md`

예시:
- `runbooks/h100-boot-failure.md`
- `runbooks/deployment/rolling-restart.md`

장애 기록은 `incidents/<YYYY-MM-DD>-<slug>.md`에 별도로.

## 템플릿

```markdown
---
tags = ["<시스템>", "<증상>"]
type = "runbook"
created = 2026-04-16T00:00:00Z
updated = 2026-04-16T00:00:00Z
source = "conversation"
confidence = "medium"
---

# <런북 제목>

## 언제 이걸 보는가

- 증상: ...
- 영향: ...
- 발동 조건: ...

## 사전 확인

```sh
# 먼저 이걸 체크
<command>
```

## 절차

1. ...
2. ...
3. 확인: `<command>` 출력이 `<expected>`여야 함
4. 실패 시: ...

## 원인 (알려진 경우)

...

## 예방

- ...
- 자동화 가능성: [[decisions/<관련결정>]] 참조

## 참고

- 최근 적용: YYYY-MM-DD (@이름)
- 관련 런북: [[runbooks/<other>]]
```

## 장애 기록 vs 런북

| 문서 타입 | 위치 | 내용 |
|---|---|---|
| Runbook | `runbooks/<주제>.md` | 재사용 가능한 절차 |
| Incident | `incidents/<날짜>-<슬러그>.md` | 특정 날짜의 장애 기록 (타임라인·원인·재발방지) |

두 번째 발생 시 incident는 그대로 두고, 공통 대응을 추출해서 runbook으로 승격합니다.

## 점검 주기

- 매 분기: stale runbook 확인 (`gadgetron wiki audit`, 90일 이상 미수정)
- 실제 장애 대응 후: 런북대로 동작했는지 확인, 틀린 부분 업데이트
- Kairos가 런북 따르는 대화를 보면 절차 검증 기회

## 관련

- [`decisions/README.md`](../decisions/README.md) — 결정 기록
- [`operators/troubleshooting.md`](../operators/troubleshooting.md) — 흔한 에러 모음 (런북보다 짧은 단위)
