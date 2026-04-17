---
tags = ["decisions", "adr", "template"]
type = "index"
source = "seed"
plugin = "gadgetron-core"
plugin_version = "0.3.0"
---

# Decisions

설계·운영 결정을 기록하는 폴더입니다. 왜 이 선택을 했는지, 무엇을 고려하고 버렸는지, 누가 참여했는지를 남깁니다.

## 왜 기록하는가

결정은 일회성이 아닙니다. 6개월 뒤 "이걸 왜 이렇게 했지?" 물음에 답해야 하고, 후임이 같은 트레이드오프를 다시 계산하지 않도록 해야 합니다.

## 작성 시점

- 여러 대안 중 고른 결정
- 관례를 깨고 내린 결정
- 스크립트·코드에 영향을 주는 설정·정책 선택
- 외부 벤더·라이브러리 선택
- 한 줄 이상의 논쟁이 있었던 판단

사소한 코드 스타일 결정은 남기지 마세요. 오래 남을 결정만.

## 파일 이름

`decisions/<YYYY-MM-DD>-<slug>.md`

예시:
- `decisions/2026-04-14-assistant-ui-over-openwebui.md`
- `decisions/2026-04-16-plugin-architecture.md`

## 템플릿

```markdown
---
tags = ["<토픽1>", "<토픽2>"]
type = "decision"
created = 2026-04-16T00:00:00Z
updated = 2026-04-16T00:00:00Z
source = "user"
confidence = "high"
---

# <결정 제목>

**상태**: proposed | accepted | superseded | deprecated
**결정일**: YYYY-MM-DD
**참여자**: @이름 또는 @역할

## 맥락 (Context)

왜 이 결정이 필요했는가. 무엇이 제약이었는가.

## 고려한 대안

| 대안 | 장점 | 단점 |
|---|---|---|
| A | ... | ... |
| B | ... | ... |
| C | ... | ... |

## 결정

A를 선택했다. 이유는...

## 결과 (Consequences)

- 긍정: ...
- 부정: ...
- 미정: ... (나중에 재평가 필요)

## 관련 문서

- [[runbooks/관련런북]]
- [[decisions/이전-관련-결정]]
```

## Penny와의 협업

- 대화에서 결정이 내려지면 Penny가 자동으로 이 폴더에 기록할 수 있습니다
- 기록 시 `source = "conversation"` + `confidence = "medium"` 표시
- 사용자가 "이 결정은 확정이야"라고 하면 `confidence = "high"`로 업데이트

## 상태 표기

- **proposed** — 논의 단계, 확정 전
- **accepted** — 확정됐고 구현됨 (또는 구현 중)
- **superseded** — 다른 결정으로 대체됨. `Supersedes: decisions/old-decision` 링크 달기
- **deprecated** — 더 이상 유효하지 않음

## 관련

- [`runbooks/README.md`](../runbooks/README.md) — 반복 작업 절차
- [`penny/conventions.md`](../penny/conventions.md) — 작명 규칙
