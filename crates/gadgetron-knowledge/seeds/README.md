---
tags = ["index", "getting-started"]
type = "index"
source = "seed"
plugin = "gadgetron-core"
plugin_version = "0.3.0"
---

# Gadgetron 위키

Gadgetron의 집단 기억입니다. 운영자, 사용자, Penny가 함께 쓰고 읽습니다.

## 시작 문서

- [`penny/usage.md`](penny/usage.md) — Penny와 대화하는 법, 도구 목록, 기록 정책
- [`penny/conventions.md`](penny/conventions.md) — 페이지 작명·계층 규칙
- [`operators/getting-started.md`](operators/getting-started.md) — 운영자 초기 설정
- [`operators/troubleshooting.md`](operators/troubleshooting.md) — 운영 트러블슈팅
- [`decisions/README.md`](decisions/README.md) — 팀 결정 기록 템플릿
- [`runbooks/README.md`](runbooks/README.md) — 반복 작업 절차 템플릿

## 폴더 구조 (컨벤션)

| 폴더 | 용도 | 주 저자 |
|---|---|---|
| `penny/` | Penny 자체에 대한 메타 문서, 컨벤션 | 코어 + 사용자 |
| `operators/` | 운영자 온보딩, 장애 대응, 운영 노하우 | 운영자 + Penny |
| `decisions/` | 팀 설계·운영 결정 기록 (ADR 스타일) | 사용자 + Penny |
| `runbooks/` | 반복되는 작업의 절차 | 운영자 + Penny |
| `users/<이름>/` | 사용자별 개인 노트·선호·맥락 | 개별 사용자 |

## 기본 원칙

1. **모든 변경은 git 커밋됩니다.** 실수는 `git log`로 추적 가능.
2. **파일시스템의 마크다운이 원본.** pgvector 인덱스는 파생물. DB 손실 시 `gadgetron reindex --full`로 복구 가능.
3. **Penny는 반복될 만한 지식을 적극 기록합니다.** 저장하지 않을 내용은 명시하세요.
4. **크레덴셜은 자동 차단.** PEM 키·AWS 키 등 명백한 시크릿이 포함된 쓰기는 거부됩니다.

## 수정·삭제

- 아무 파일이나 편집기로 직접 수정 가능. 변경은 다음 `gadgetron reindex` 또는 Penny 다음 호출 시 인덱스에 반영.
- Penny에게 말해서 수정/삭제도 가능: "이 페이지 지워줘", "이 내용 틀렸어, 고쳐줘".
