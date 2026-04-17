# `docs/agents/` — 개발 협업 역할 정의

이 디렉터리는 **Gadgetron을 개발하는 AI 코딩 세션** 이 수행하는 전문가 역할의 정의 파일을 모은다.

**주의**: 여기 모인 역할은 **Penny** (출시 바이너리에서 동작하는 제품 런타임) 와는 다른 층위다. 두 층위의 구분은 [`AGENTS.md`](../../AGENTS.md) §`Penny vs. 이 파일` 에 있다.

| 층위 | 대상 | SSOT |
|---|---|---|
| 제품 런타임 | **Penny** | [`docs/design/phase2/02-penny-agent.md`](../design/phase2/02-penny-agent.md) |
| 개발 협업 | PM + 아래 전문가 역할 | 이 디렉터리 (`docs/agents/*.md`) |

---

## 역할 목록

PM-주재 · 최소 5명 동적 편성 원칙은 [`AGENTS.md`](../../AGENTS.md)와 [`../process/00-agent-roster.md`](../process/00-agent-roster.md)에 규정되어 있다.

| 역할 | 파일 | 주요 담당 |
|---|---|---|
| Chief Architect | [`chief-architect.md`](chief-architect.md) | 핵심 타입·트레이트·에러, 크로스 크레이트 일관성 (D-12 seam) |
| Gateway / Router Lead | [`gateway-router-lead.md`](gateway-router-lead.md) | axum HTTP 서버, 6 routing strategy, SSE |
| Inference Engine Lead | [`inference-engine-lead.md`](inference-engine-lead.md) | 6 provider 어댑터, 프로토콜 변환 |
| GPU Scheduler Lead | [`gpu-scheduler-lead.md`](gpu-scheduler-lead.md) | VRAM bin-packing, NVML, MIG, eviction |
| XaaS Platform Lead | [`xaas-platform-lead.md`](xaas-platform-lead.md) | Multi-tenancy, auth, quota, audit, billing |
| DevOps / SRE Lead | [`devops-sre-lead.md`](devops-sre-lead.md) | 배포, CI/CD, observability |
| UX / Interface Lead | [`ux-interface-lead.md`](ux-interface-lead.md) | TUI (ratatui), Web UI (assistant-ui) |
| QA / Test Architect | [`qa-test-architect.md`](qa-test-architect.md) | 테스트 전략, mock/fake, 성능 벤치 |
| Security / Compliance Lead | [`security-compliance-lead.md`](security-compliance-lead.md) | STRIDE threat model, OWASP, 컴플라이언스 |
| DX / Product Lead | [`dx-product-lead.md`](dx-product-lead.md) | CLI UX, 에러 메시지, 문서 |
| Codex Chief Advisor | [`codex-chief-advisor.md`](codex-chief-advisor.md) | 외부 두 번째 의견 (OpenAI Codex 기반) |

---

## 이 파일들의 사용 방식

- 각 `<role>.md` 는 **일반 마크다운** 으로 작성된 role prompt 이다. 특정 AI 도구에 얽매이지 않는다.
- 네이티브 subagent 기능이 있는 도구 (Claude Code, OpenCode 등) 는 이 내용을 로드해서 에이전트로 등록해 사용할 수 있다. 도구별 어댑터(`.claude/agents/` 등) 는 로컬 user-level 에서만 관리하고 레포에는 커밋하지 않는다 (드리프트 방지; `AGENTS.md §"서브에이전트" 실행 방식` 참조).
- 단일 에이전트 도구 (Codex 등) 는 PM 이 역할을 순차적으로 로드해 교대로 수행한다.

역할 정의를 수정할 때는 **이 파일만** 갱신한다. 도구별 어댑터는 사본이다.
