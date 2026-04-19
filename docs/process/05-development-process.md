# 개발 프로세스

> **승인**: 2026-04-12 사용자 승인
> **적용**: 모든 스프린트에 적용. 예외 없음.

---

## 1. 스프린트 사이클

```
① 조사 (서브에이전트 병렬)
   └→ **graphify 먼저** (§6 참조): `graphify-out/GRAPH_REPORT.md`
       에서 관련 community hub + god node 파악 → 탐색을 해당 커뮤니티
       멤버로 좁힘. `graphify query/explain/path` 로 연결 관계 검증.
       조사 프롬프트를 서브에이전트에 위임할 때 같은 지시를 포함.
   └→ 기존 코드/문서 gap 분석
   └→ 구현에 필요한 정보 수집

② 문서화 (담당 서브에이전트가 작성)
   └→ 5 필수 섹션 (철학/구현/연결/단위테스트/통합테스트)
   └→ 모듈 테스트 방안 + 실제 동작 테스트 + 검증 방안
   └→ 구현 결정론 10가지 충족

③ 크로스 리뷰 (서브에이전트 적극 참여)
   └→ Round 1: 도메인 정합성 (2명 병렬)
   └→ Round 1.5: 보안 + 사용성 (2명 병렬)
   └→ Round 2: 테스트 가능성 (qa-test-architect)
   └→ Round 3: Rust 관용구 + 구현 결정론 (chief-architect)
   └→ 각 Round에서 발견된 gap → fix → re-review

④ 조사 (리뷰 결과 반영 확인)
   └→ 완결성 미충족 시 ② 로 복귀
   └→ 충족 시 → Approved

⑤ TDD 구현 (서브에이전트가 수행)
   └→ Red: 실패 테스트 먼저
   └→ Green: 최소 구현
   └→ Refactor
   └→ 서비스 경로를 바꿨다면 `build/start/stop/status/logs` 스크립트 또는 동등 자동화까지 같이 구현

⑥ 리팩토링 (스프린트 종료 후)
   └→ 코드: clippy 0 warnings, fmt, dead code 삭제
   └→ 문서: 코드-문서 불일치 0건
   └→ "더 이상 할 것 없음" 판단까지 반복

⑦ 검증 완료 (구현 후 필수)
   └→ 코드/스크립트 변경 직후 `./verify_cycle.sh changed`
   └→ PR 직전 `./verify_cycle.sh ci`
   └→ 필요한 도메인 smoke/eval 은 추가로 수행
   └→ 최종 보고에 "무엇을 실행했고 무엇을 생략했는지" 명시

⑧ 매뉴얼 업데이트 (push 전 필수)
   └→ docs/manual/ 에 구현된 내용 반영
   └→ 새 endpoint, CLI, config, 에러 코드 모두 매뉴얼에 포함
   └→ 서비스 완전 기동 경로와 운영 루프(build/start/stop/status/logs)도 매뉴얼에 포함
   └→ 매뉴얼에 없는 기능 = push 금지

⑨ PR + Merge
   └→ git commit → gh pr create → merge
   └→ clean state에서 다음 Sprint 시작
```

## 2. 절대 규칙

| # | 규칙 |
|---|---|
| 1 | **문서 없이 구현 없음** — 설계 문서 Approved 후에만 코드 작성 |
| 2 | **3회 이상 크로스 리뷰** — Round 1 → 1.5 → 2 → 3. 서브에이전트가 수행 |
| 3 | **TDD** — Red → Green → Refactor |
| 4 | **구현 결정론** — 누가 와도 같은 결과. TBD/모호함 금지. 10가지 필수 명시 |
| 5 | **서브에이전트 최대한 활용** — PM은 조율. 실행은 서브에이전트 |
| 6 | **스프린트 종료 후 리팩토링** — 더 할 것 없을 때까지 반복 |
| 7 | **PM 권한** — 실행 결정은 PM 자율. 전략적 결정만 사용자 escalation |
| 8 | **테스트 방안 필수** — 모듈 테스트 + 실제 동작 테스트 + 검증 방안 |
| 9 | **도구 설치 자유** — 구현/테스트/검증에 필요한 툴·플러그인·MCP 사전 승인 |
| 10 | **서비스 제공 완결성** — 실행 경로를 바꾸는 작업은 서비스를 끝까지 띄우는 스크립트/자동화와 운영 문서까지 함께 제출해야 완료 |

## 5. 구현 후 검증 계약

1. **구현이 끝나면 바로 `./verify_cycle.sh changed`** 를 실행한다. 이 스크립트는 현재 변경 파일을 기준으로 touched crate를 계산하고, 필요 시 workspace-wide 검증으로 승격한다.
2. **PR 직전에는 `./verify_cycle.sh ci`** 를 실행한다. 로컬에서 재현 가능한 범위의 CI 게이트(`check` / `fmt` / `clippy` / `test`)를 그대로 따라간다. `cargo-deny`가 설치되어 있으면 보안 검사까지 포함한다.
3. **공용/파급 큰 crate 변경은 자동으로 더 넓게 검증한다.** `gadgetron-core`, `provider`, `router`, `gateway`, `xaas`, 루트 manifest, CI workflow 변경은 `changed` 모드에서도 workspace 검증으로 올린다.
4. **Penny/Knowledge/Web 같은 결합 영역은 fan-out 검증을 포함한다.** 예: `gadgetron-penny`/`gadgetron-knowledge` 변경 시 `gadgetron-cli`도 함께 검증하고, `gadgetron-web` 변경 시 `gadgetron-gateway`도 함께 검증한다.
5. **스크립트가 끝이 아니다.** 실제 동작 smoke, eval harness, 수동 UX 확인이 필요한 작업은 해당 검증을 추가로 실행하고, 최종 보고에 명시한다.
6. **실행 경로 변경은 운영 루프까지 검증한다.** 새로 도입/수정한 `build/start/stop/status/logs` 경로 또는 동등 자동화로 서비스가 실제로 올라와야 한다.

## 3. 서브에이전트 역할 분담

| 역할 | 참여 시점 |
|---|---|
| chief-architect | Round 1 + Round 3 (final gate) + 리팩토링 |
| gateway-router-lead | 문서 작성 (gateway) + Round 1 + 구현 |
| inference-engine-lead | Round 1 (provider) + 구현 |
| gpu-scheduler-lead | Round 1 (GPU/VRAM) + 구현 |
| xaas-platform-lead | 문서 작성 (xaas) + Round 1 + 구현 |
| devops-sre-lead | CI/CD + 배포 문서 + Round 1 |
| ux-interface-lead | TUI/Web 구현 + Round 1 |
| qa-test-architect | Round 2 (테스트 가능성) + 테스트 하네스 |
| security-compliance-lead | Round 1.5 (보안) + STRIDE |
| dx-product-lead | Round 1.5 (사용성) + CLI UX + 에러 메시지 |

## 4. 산출물 체인

```
조사 보고서 → 설계 문서 (Approved) → TDD 코드 → 테스트 결과 → 리팩토링 → PR → Merge
```

---

## 6. 그래프 기반 탐색 (graphify) 규칙

본 리포지토리는 `graphify` 로 생성된 지식 그래프 (`graphify-out/`, 281+ 파일에 대한 community detection + god nodes) 를 **탐색의 1차 진입점**으로 사용한다. 이 규모의 코드베이스에서 `GRAPH_REPORT.md` 를 먼저 읽는 것이 세 번의 speculative grep 보다 빠르다.

### 6.1 강제 규칙 — 메인 에이전트 + 모든 서브에이전트 (Agent-tool spawns 포함)

1. **파일을 찾거나 심볼을 참조하기 전에** `graphify-out/GRAPH_REPORT.md` 를 먼저 연다. 관련 *community hub* (예: "Auth & Server Core", "Knowledge Curation") 와 *god node* (해당 community 안의 high-degree symbol) 를 식별한다. 파일 검색을 community member list 로 좁히고 repo-wide grep 을 피한다.

2. **`Agent` 도구로 서브에이전트를 dispatch 할 때** 프롬프트에 다음을 포함한다: *"먼저 `graphify-out/GRAPH_REPORT.md` 를 읽어 관련 community + god node 를 찾고, 그 뒤에 구체 파일을 읽어라."* 탐색 범위를 scoped 로 유지해 corpus 재읽기를 방지한다.

3. **GRAPH_REPORT 읽기만으로 부족하면** `graphify query "<question>" [--dfs]`, `graphify path "<A>" "<B>"`, `graphify explain "<node>"` 로 연결 관계 / call site / decoupling 을 직접 검증한다. 설계 문서의 §Connections 섹션 작성 시 필수.

4. **Rust 코드 수정 후** `graphify update .` 를 실행한다 (AST fast path 로 LLM 비용 없음). 문서/markdown 변경은 `/graphify --update` 가 필요 (LLM 비용 발생).

5. **`graphify-out/wiki/index.md` 가 존재하면** raw 파일이 아니라 이 위키를 탐색 entry 로 사용한다 — curated agent-crawlable surface 다.

### 6.2 Hook discipline (자동 갱신)

Git hook 이 그래프를 자동 새로고침해 `GRAPH_REPORT.md` 가 fresh 하게 유지된다.

- `post-commit` — 커밋마다 AST refresh (`graphify hook install` 이 설치)
- `post-checkout` — branch 전환 시 refresh
- `post-merge` — `git pull` / merge 시 refresh (저장소 내 `.githooks/post-merge`)

신규 clone 후 `./scripts/install-git-hooks.sh` 한 번 실행. Idempotent — 재실행 안전.

### 6.3 Fallback

`graphify` CLI 미설치 시 (`pipx install graphifyy` 또는 `pip install --user graphifyy`), hook + `graphify update` 명령은 silent no-op — 커밋·머지·pull 을 절대 block 하지 않는다. `GRAPH_REPORT.md` 는 plain markdown 이라 도구 없어도 (stale 하더라도) 읽을 수 있다.

### 6.4 스프린트 사이클 내 위치

| 사이클 단계 | graphify 사용법 |
|---|---|
| ① 조사 | GRAPH_REPORT 로 관련 community + god node 파악 → member list 로 읽기 범위 축소. 서브에이전트 프롬프트에도 같은 지시 포함. |
| ② 문서화 | 설계 문서 §Connections 섹션에 `graphify query/explain/path` 결과 인용. "이 모듈이 무엇에 의존하는가 / 무엇이 이 모듈에 의존하는가" 를 그래프로 검증. |
| ③ 크로스 리뷰 | Reviewer 가 "이 설계는 실제 코드 경계와 맞는가" 를 `graphify path` 로 검증. 설계 문서의 claimed dependency 와 실제 graph edges 불일치 시 flag. |
| ⑤ TDD 구현 | 구현 도중 `graphify explain <node>` 로 변경 영향 범위 확인. 매 commit 후 hook 이 AST refresh. |
| ⑥ 리팩토링 | `graphify query "<refactor target>"` 으로 참조 call site 완전 열거. dead code 식별. |
