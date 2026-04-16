---
tags = ["kairos", "conventions", "wiki-structure"]
type = "runbook"
source = "seed"
plugin = "gadgetron-core"
plugin_version = "0.3.0"
---

# 위키 작명·계층 규칙

Kairos와 사용자가 함께 지키는 컨벤션입니다. 강제는 아니지만 지키면 검색·탐색이 쉬워집니다.

## 계층 규칙

슬래시(`/`)로 계층을 만듭니다. 한 단계 이상 깊이를 권장합니다.

```
runbooks/h100-boot-failure
decisions/2026-04-adr-p2a-07-semantic-wiki
users/junghopark/preferences
```

루트(`/`)에는 `README.md`만 둡니다. 주제 페이지는 반드시 어느 폴더 아래에.

## 이름 규칙

- **소문자 + 하이픈**. 공백·언더스코어·한글 파일명은 피합니다 (검색·링크 편의)
- **확장자 `.md`는 자동**. `wiki.write` 호출 시 이름에 `.md` 붙이지 않습니다
- **날짜는 `YYYY-MM-DD`** 접두사로: `decisions/2026-04-16-plugin-architecture`
- **카테고리 prefix는 넣지 않음** — 폴더 구조로 카테고리 표현

## 표준 폴더

| 폴더 | 담는 내용 | 템플릿 |
|---|---|---|
| `kairos/` | Kairos 자체 메타, 이 문서 같은 것 | 자유 |
| `operators/` | 운영자 온보딩·운영 가이드 | [`operators/getting-started.md`](../operators/getting-started.md) |
| `decisions/` | 설계·운영 결정 기록 (ADR 스타일) | [`decisions/README.md`](../decisions/README.md) |
| `runbooks/` | 반복 작업 절차 | [`runbooks/README.md`](../runbooks/README.md) |
| `incidents/<YYYY-MM-DD>-<slug>` | 발생한 장애 기록 | 런북 템플릿 재활용 |
| `users/<name>/` | 개인 선호·맥락 (다른 사람은 수정 금지 컨벤션) | 자유 |
| `projects/<project-name>/` | 프로젝트별 지식 | 자유 |
| `_archived/<orig-path>` | 소프트 삭제된 페이지 | `wiki.delete` 자동 |

## TOML 프론트매터

모든 페이지 첫 줄이 `---`이면 TOML 프론트매터로 인식합니다. 권장 필드:

```markdown
---
tags = ["h100", "boot-failure", "bios"]
type = "incident"
created = 2026-04-16T10:30:00Z
updated = 2026-04-16T11:00:00Z
source = "conversation"
confidence = "high"
---
```

| 필드 | 값 | 비고 |
|---|---|---|
| `tags` | 자유 | 태그 인덱스에 사용 |
| `type` | `"incident"` \| `"runbook"` \| `"decision"` \| `"note"` \| `"index"` \| 자유 | 닫힌 enum 권장 |
| `source` | `"user"` \| `"conversation"` \| `"reindex"` \| `"seed"` | |
| `confidence` | `"high"` \| `"medium"` \| `"low"` | AI 저장은 medium |
| `created` / `updated` | RFC 3339 | 파서가 자동 설정 |
| `plugin` | 예: `"gadgetron-core"` | seed 페이지의 소유자 |

프론트매터 없어도 저장은 가능합니다. 단, ADR-P2A-07 이후 의미 검색에서 타입/태그 필터링 혜택을 못 받습니다.

## 링크

Obsidian 스타일 `[[페이지이름]]` 링크를 쓰세요. Kairos와 파서가 백링크를 추적합니다 (백링크 인덱스는 P2B).

```markdown
이 인시던트는 [[runbooks/h100-boot-failure]]의 절차를 따랐다.
```

## 상충 시 우선순위

- 사람(운영자·사용자)이 명시한 규칙 > 이 문서의 컨벤션
- 이 문서의 컨벤션 > Kairos의 자동 판단
- 의심스러우면 사용자에게 확인

## 관련

- [`kairos/usage.md`](../kairos/usage.md) — Kairos 사용법
- [`README.md`](../README.md) — 위키 루트
