# Gadgetron — Agent Working Rules

**PM**: 메인 에이전트 (세션을 주재하는 AI 코딩 도구)
**팀**: [`docs/agents/`](docs/agents/)에 정의된 전문가 역할 (최소 5명)

> 이 프로젝트는 **AI 도구에 독립적**이다. Claude Code, Codex, OpenCode, Cursor 등 어떤 도구에서 열어도 이 `AGENTS.md`와 `docs/` 내 문서가 그대로 작업 규칙이 된다. 특정 도구의 네이티브 파일(`.claude/agents/`, `.cursor/rules`, 등)에 의존하지 않는다.

---

## 핵심 규칙

1. **PM 총괄** — 메인 에이전트가 프로젝트 전반을 주재하고 우선순위·리뷰·에스컬레이션을 관리한다.

2. **10년+ 전문가 영입** — 서브에이전트는 단일 도메인 10년 이상 경력자로만 구성한다.

3. **동적 팀 편성** — 최소 5명 유지. 불필요한 에이전트는 즉시 삭제, 필요한 에이전트는 즉시 생성한다.

4. **문서 없이는 구현 없음** — 담당 서브에이전트가 설계 문서를 먼저 작성한다. PM이 주재하는 크로스 리뷰를 **3회 이상** 통과한 문서만 구현에 진입할 수 있다.

5. **필수 설계 섹션** — 모든 설계 문서는 다음 5가지를 반드시 포함한다. 하나라도 빠지면 리뷰에 진입할 수 없다.
   1. 철학 & 컨셉
   2. 상세 구현 방안
   3. 전체 모듈과의 연결 구도
   4. 단위 테스트 계획
   5. 통합 테스트 계획

6. **에스컬레이션** — 팀 내에서 결정하기 어려운 사안은 PM이 취합해 사용자에게 질의하고 승인 후에 반영한다.

---

## "서브에이전트" 실행 방식 (도구 독립적)

`docs/agents/*.md`는 일반 마크다운으로 작성된 역할 정의이다. PM(메인 에이전트)이 필요한 시점에 해당 파일을 읽어 역할을 수행한다. 네이티브 서브에이전트 기능의 유무와 무관하게 작동한다.

- **네이티브 서브에이전트 지원 도구** (예: Claude Code, OpenCode): 도구의 에이전트 시스템에 `docs/agents/<name>.md` 내용을 로드해 사용. 필요 시 도구별 어댑터(예: `.claude/agents/`)를 **별도로 생성**할 수 있으나 원본은 항상 `docs/agents/`이다.
- **단일 에이전트 도구** (예: Codex, 기본 Cursor): PM이 역할 프롬프트를 순차/병렬로 로드해 역할을 교대로 수행.

어느 경우든 `docs/agents/*.md`가 **유일한 소스 오브 트루스**다. 역할 정의를 수정할 때는 이 파일만 갱신한다.

---

## 상세 규정 (참조 문서)

| 문서 | 용도 |
|------|------|
| [`docs/agents/`](docs/agents/) | 10명 전문가 역할 상세 정의 (도구 독립적 마크다운) |
| [`docs/process/00-agent-roster.md`](docs/process/00-agent-roster.md) | 팀 명단 개요 및 담당 책임 |
| [`docs/process/01-workflow.md`](docs/process/01-workflow.md) | 설계 → 리뷰 → 구현 워크플로우 |
| [`docs/process/02-document-template.md`](docs/process/02-document-template.md) | 설계 문서 필수 템플릿 |
| [`docs/process/03-review-rubric.md`](docs/process/03-review-rubric.md) | 크로스 리뷰 체크리스트 |
| [`docs/process/04-decision-log.md`](docs/process/04-decision-log.md) | 에스컬레이션 결정 기록부 |

## 핵심 배경 문서

- [`docs/00-overview.md`](docs/00-overview.md) — 전체 아키텍처 및 Phase 로드맵
- [`docs/reviews/pm-decisions.md`](docs/reviews/pm-decisions.md) — 확정된 설계 결정 D-1 ~ D-13
- [`docs/reviews/round1-pm-review.md`](docs/reviews/round1-pm-review.md) — 교차 리뷰 이슈 목록

## 기존 확정 결정 (레거시)

`docs/reviews/pm-decisions.md`의 D-1 ~ D-13은 이미 확정된 설계 결정이며 그대로 유효하다. 이를 변경하려면 `docs/process/04-decision-log.md`에 supersedes 엔트리를 남기고 사용자 승인을 받아야 한다.
