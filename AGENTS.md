# Behavioral Guidelines (general LLM coding hygiene)

These four principles apply across any task in this repo, regardless of which IDE / agent surface is loaded. They sit alongside the project-specific rules below in `# Gadgetron — Agent Working Rules`; when something feels underspecified by either set, prefer the stricter one.

**Tradeoff:** These guidelines bias toward caution over speed. For trivial tasks, use judgment.

## 1. Think Before Coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

Before implementing:
- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them - don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.

## 2. Simplicity First

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single-use code.
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.

## 3. Surgical Changes

**Touch only what you must. Clean up only your own mess.**

When editing existing code:
- Don't "improve" adjacent code, comments, or formatting.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently.
- If you notice unrelated dead code, mention it - don't delete it.

When your changes create orphans:
- Remove imports/variables/functions that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

The test: Every changed line should trace directly to the user's request.

## 4. Goal-Driven Execution

**Define success criteria. Loop until verified.**

Transform tasks into verifiable goals:
- "Add validation" → "Write tests for invalid inputs, then make them pass"
- "Fix the bug" → "Write a test that reproduces it, then make it pass"
- "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:
```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

---

**These guidelines are working if:** fewer unnecessary changes in diffs, fewer rewrites due to overcomplication, and clarifying questions come before implementation rather than after mistakes.


# Gadgetron — Agent Working Rules

**PM**: 메인 에이전트 (세션을 주재하는 AI 코딩 도구)
**팀**: [`docs/agents/`](docs/agents/)에 정의된 전문가 역할 (최소 5명)

> 이 프로젝트는 **AI 도구에 독립적**이다. Claude Code, Codex, OpenCode, Cursor 등 어떤 도구에서 열어도 이 `AGENTS.md`와 `docs/` 내 문서가 그대로 작업 규칙이 된다. 특정 도구의 네이티브 파일(`.claude/agents/`, `.cursor/rules`, 등)에 의존하지 않는다.

## Penny vs. 이 파일

Gadgetron 에는 이름이 붙은 구성요소 **Penny** 와, 이 repo 를 개발하는 AI 코딩 세션 (= 이 파일의 대상) 이 따로 있다. 단어 "에이전트" 는 두 맥락에서 모두 쓰일 수 있어 한 번만 구분을 박아둔다:

| 층위 | 무엇 | 정의 위치 | 런타임 |
|---|---|---|---|
| **제품** | **Penny** (Penny Brown) — 출시 바이너리에서 동작하며 사용자의 요청을 해결 | [`docs/design/phase2/02-penny-agent.md`](docs/design/phase2/02-penny-agent.md), [`docs/00-overview.md`](docs/00-overview.md) §1.2 | `gadgetron serve` 실행 시 내부 subprocess |
| **개발 협업** | PM(메인 AI 코딩 세션) + `docs/agents/` 전문가 역할 (chief-architect, gateway-router-lead, qa-test-architect 등) | 이 `AGENTS.md` + `docs/agents/*.md` | 개발 세션 중 Claude Code / Codex / Cursor 등 IDE 상에서 |

**이 문서(AGENTS.md) 가 지배하는 대상은 두 번째 (개발 협업) 다.** Penny 의 런타임 동작·권한·보안 정책은 `docs/design/phase2/` 와 ADR-P2A-* 에서 관리한다.

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

6. **완전 기동 책임** — 서비스 실행 경로를 추가·변경하는 작업은 문서만으로 끝나지 않는다. 구현 세션은 서비스를 실제로 완전히 띄워서 제공하기 위한 스크립트/자동화와 운영 루프(`build/start/stop/status/logs` 또는 동등 수단)까지 함께 제작·검증해야 한다.

7. **에스컬레이션** — 팀 내에서 결정하기 어려운 사안은 PM이 취합해 사용자에게 질의하고 승인 후에 반영한다.

---

## "서브에이전트" 실행 방식 (도구 독립적)

`docs/agents/*.md`는 일반 마크다운으로 작성된 역할 정의이다. PM(메인 에이전트)이 필요한 시점에 해당 파일을 읽어 역할을 수행한다. 네이티브 서브에이전트 기능의 유무와 무관하게 작동한다.

- **네이티브 서브에이전트 지원 도구** (예: Claude Code, OpenCode): 도구의 에이전트 시스템에 `docs/agents/<name>.md` 내용을 로드해 사용.
- **단일 에이전트 도구** (예: Codex, 기본 Cursor): PM이 역할 프롬프트를 순차/병렬로 로드해 역할을 교대로 수행.

**도구별 어댑터(`.claude/agents/`, `.cursor/rules/` 등)는 레포에 커밋하지 않는다.** 두 벌 관리로 인한 드리프트를 방지하기 위함. 개인적으로 Claude Code 네이티브 dispatch 가 필요하면 user-level `~/.claude/agents/` 에 사본을 두어 사용하고, 원본 수정은 항상 `docs/agents/` 에만 반영한다. `.gitignore` 가 `/.claude/` 전체를 제외한다.

어느 경우든 `docs/agents/*.md`가 **유일한 소스 오브 트루스**다. 역할 정의를 수정할 때는 이 파일만 갱신한다.

---

## 상세 규정 (참조 문서)

| 문서 | 용도 |
|------|------|
| [`docs/agents/`](docs/agents/) | 10명 전문가 + 1명 총괄자문(Codex) 역할 상세 정의 (도구 독립적 마크다운) |
| [`docs/process/00-agent-roster.md`](docs/process/00-agent-roster.md) | 팀 명단 개요 및 담당 책임 |
| [`docs/process/01-workflow.md`](docs/process/01-workflow.md) | 설계 → 리뷰 → 구현 워크플로우 |
| [`docs/process/02-document-template.md`](docs/process/02-document-template.md) | 설계 문서 필수 템플릿 |
| [`docs/process/03-review-rubric.md`](docs/process/03-review-rubric.md) | 크로스 리뷰 체크리스트 |
| [`docs/process/04-decision-log.md`](docs/process/04-decision-log.md) | 에스컬레이션 결정 기록부 |
| [`docs/process/07-document-authority-and-reconciliation.md`](docs/process/07-document-authority-and-reconciliation.md) | 문서 권위 체계 및 정합성 정리 규칙 |

## 핵심 배경 문서

- [`docs/00-overview.md`](docs/00-overview.md) — 전체 아키텍처 및 Phase 로드맵
- [`docs/reviews/pm-decisions.md`](docs/reviews/pm-decisions.md) — 확정된 설계 결정 D-1 ~ D-13
- [`docs/reviews/round1-pm-review.md`](docs/reviews/round1-pm-review.md) — 교차 리뷰 이슈 목록

## 기존 확정 결정 (레거시)

`docs/reviews/pm-decisions.md`의 D-1 ~ D-13은 이미 확정된 설계 결정이며 그대로 유효하다. 이를 변경하려면 `docs/process/04-decision-log.md`에 supersedes 엔트리를 남기고 사용자 승인을 받아야 한다.
