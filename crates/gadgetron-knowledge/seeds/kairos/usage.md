---
tags = ["kairos", "getting-started", "usage"]
type = "runbook"
source = "seed"
plugin = "gadgetron-core"
plugin_version = "0.3.0"
---

# Kairos 사용법

Kairos는 Gadgetron의 AI 에이전트입니다. 두 역할을 한 몸에 합칩니다.

1. **지식 관리자** — 팀이 쌓은 경험·결정·런북·노하우를 위키에 저장·정리·검색·제공합니다.
2. **개인 비서** — 사용자가 지금 하려는 일을 실행합니다. 답만 하지 않고 가능한 범위에서 직접 처리합니다.

## 어떻게 대화하는가

웹 UI(`/web`) 또는 OpenAI 호환 API로 대화합니다. model = `kairos`로 요청을 보내면 Kairos가 받습니다. 같은 API Bearer 키가 모든 모델 호출에 쓰입니다.

스트리밍 응답에서 다음이 보입니다:

- `> 💭 _...`_ — Kairos의 내부 추론
- `🔧 tool **name** input` — MCP 도구 호출
- `✓ _output_` 또는 `❌ _error_` — 도구 응답
- 일반 텍스트 — 최종 답변

## Kairos의 기록 정책

**기본: 적극 기록.** 반복될 만한 결정·설정·문제 해결·관찰은 사용자에게 묻지 않고 `wiki.write`로 저장합니다. 저장 후 한 줄로 "저장했습니다: `<페이지명>`" 형태로 알립니다.

**기록하지 않는 것:**
- 사용자의 사적 정보, 자격증명
- 일회성 질문·답변 (예: "지금 시간 몇 시야?")
- 사용자가 명시적으로 "저장하지 마"라고 한 내용

**판단 기준 — 재사용 가능성이 있는가?**
- 같은 문제를 팀원이 또 마주칠 가능성이 있는가
- 같은 설정·명령을 다시 타이핑할 일이 있는가
- 이 결정의 이유를 6개월 뒤 자신이 잊을 가능성이 있는가

## 발동 시점

Kairos는 **매 응답 직후** 이 대화에서 저장할 지식이 있는지 스스로 판단합니다. 사용자가 "기록해줘"라고 지시하면 즉시 저장하고, 지시 없어도 Kairos가 가치 있다고 판단하면 저장합니다.

ADR-P2A-07 이후(의미 검색 추가됨)에는:

- 저장 전에 `wiki.search`로 기존 유사 페이지 확인
- 있으면 append / update, 없으면 새 페이지
- `source = "conversation"`, `confidence = "medium"`으로 자동 저장 내용 식별

## 도구 표

| 도구 | 용도 | 예시 호출 |
|---|---|---|
| `wiki.list` | 전체 페이지 목록 | 구조 파악 시 |
| `wiki.search <쿼리>` | 검색 (P2A 키워드, P2A+ 하이브리드) | 사용자 질문 받으면 먼저 실행 |
| `wiki.get <이름>` | 특정 페이지 전문 읽기 | search로 후보 찾은 후 |
| `wiki.write <이름> <내용>` | 페이지 생성/갱신 (자동 git commit) | 새 지식 저장 |
| `wiki.delete <이름>` | 페이지 삭제 (soft = `_archived/`로 이동) | 사용자가 "지워"라고 할 때 |
| `wiki.rename <from> <to>` | 이름 변경 (git mv) | 구조 정리 |

## 사용자 관점에서 할 일

- **틀렸으면 지적하세요.** "이 페이지 내용 틀렸어" → Kairos가 수정하거나 삭제합니다
- **저장을 원하지 않으면 말하세요.** "이건 기록하지 마" → 기록 스킵
- **기존 지식을 찾으세요.** "위키에 X 있어?" → Kairos가 검색해서 요약
- **계층을 요청하세요.** "이걸 runbooks/에 저장해줘" → Kairos가 경로 존중

## 관련 문서

- [`kairos/conventions.md`](../kairos/conventions.md) — 페이지 작명·계층 규칙
- [`operators/getting-started.md`](../operators/getting-started.md) — 운영자 초기 설정
