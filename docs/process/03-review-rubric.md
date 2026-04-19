# 크로스 리뷰 체크리스트

> 리뷰어는 각 항목을 ✅Pass / ❌Fail / N/A로 표시.
> Fail은 반드시 구체적 피드백과 Action Item 제시.

---

## §1. Round 1 — 도메인 정합성 리뷰

**리뷰어**: PM이 선정한 타 도메인 서브에이전트 **2명**
**목적**: 모듈 간 인터페이스 충돌 · 크레이트 경계 위반 · 타입 중복 방지

- [ ] **인터페이스 계약** — 타 모듈이 이 설계의 공개 API를 호출할 수 있는가?
- [ ] **크레이트 경계** — `docs/reviews/pm-decisions.md` D-12 규정대로 타입이 올바른 크레이트에 배치되었는가?
- [ ] **타입 중복** — 기존 `gadgetron-core` 타입과 중복 정의가 없는가? (Round 1 C-1 ~ C-5 재발 방지)
- [ ] **에러 반환** — `GadgetronError` variant 사용이 일관적인가? 신규 variant가 필요하면 제안했는가?
- [ ] **동시성** — Send/Sync 경계, Lock 전략 선택 근거, deadlock 가능성 검토 완료?
- [ ] **의존성 방향** — 크레이트 의존성 그래프에 역방향·순환 의존이 없는가?
- [ ] **그래프 검증 (graphify)** — §3 전체 모듈 연결 구도가 `graphify-out/GRAPH_REPORT.md` + `graphify query/explain/path` 결과를 인용하며 claimed dependency 와 실제 graph edges 가 일치하는가? Speculative dependency list 만으로는 Pass 아님. 리뷰어는 최소한 god node 하나를 `graphify explain <node>` 로 확인하고 설계 문서의 설명과 비교한다. (자세한 규칙은 `docs/process/05-development-process.md §6`)
- [ ] **Phase 태그** — 섹션/필드가 `[P1]/[P2]/[P3]`로 명확히 구분되었는가?
- [ ] **레거시 결정 준수** — `pm-decisions.md` D-1 ~ D-13과 충돌하지 않는가?

---

## §1.5. Round 1.5 — 보안 & 사용성 리뷰 (신규, 2026-04-12)

**리뷰어**: `security-compliance-lead` + `dx-product-lead` (병렬)
**목적**: Round 1 도메인 정합성 통과 후, 본격 테스트 가능성 검증 전에 보안 위협과 사용자 사용성 빈틈을 동시에 차단

### §1.5-A. 보안 체크리스트 (`security-compliance-lead`)

- [ ] **위협 모델 (필수)** — STRIDE 6 카테고리(Spoofing/Tampering/Repudiation/Info disclosure/DoS/EoP) 검토. 자산·신뢰 경계·위협·완화가 표로 명시되어 있는가?
- [ ] **신뢰 경계 입력 검증** — 외부 입력(HTTP body, 헤더, 파일, env)이 진입 시점에 검증되는가?
- [ ] **인증·인가** — 새 엔드포인트가 default-deny인가? scope/role 검사가 있는가? bypass 경로가 없는가?
- [ ] **시크릿 관리** — API 키·TLS 키·DB 자격증명이 logs/configs/error message/trace 필드에 노출되지 않는가? redaction layer 적용?
- [ ] **공급망** — 신규 의존성이 `cargo audit` 통과? 라이선스 호환? 변조 위험은?
- [ ] **암호화** — at-rest / in-transit 정책 명시? 자체 구현 crypto 없음? `rustls`/`ring`/`argon2` 사용?
- [ ] **감사 로그** — 보안 이벤트(인증 실패, 권한 거부, 키 회전)가 append-only로 기록되는가?
- [ ] **에러 정보 누출** — 에러 메시지가 attacker에게 internal 구조 (파일 경로·DB 스키마·스택)를 노출하지 않는가?
- [ ] **LLM 특이 위협** (해당 시) — prompt injection 방어, 출력 PII 필터, 모델 출처 검증?
- [ ] **컴플라이언스 매핑** — SOC2 CC6.x / GDPR Art 32 / HIPAA §164.312 중 해당 control 식별?

### §1.5-B. 사용성 체크리스트 (`dx-product-lead`)

- [ ] **사용자 touchpoint 워크스루** — 이 기능을 사용자가 어떻게 만나는가? CLI? API? config? TUI? 모든 경로 매핑.
- [ ] **에러 메시지 3요소** — 모든 새 에러가 (무엇이 일어났는지 / 왜 / 사용자가 어떻게 고칠지) 답하는가?
- [ ] **CLI flag** — GNU/POSIX 규약 (`--long`, `-s`)? `--help`가 스캔 가능? 예시 포함?
- [ ] **API 응답 shape** — OpenAI 호환 형식 유지? `{error: {message, type, code}}` 일관?
- [ ] **config 필드** — 모든 새 필드에 doc comment + default + env override 라인이 있는가?
- [ ] **defaults 안전성** — default 값이 안전(secure)하고 합리적(least surprise)인가?
- [ ] **문서 5분 경로** — quick-start 5분 안에 동작 확인 가능한가? copy-pasteable?
- [ ] **runbook playbook** — 새 알람/에러에 대해 oncall이 따라할 단계가 있는가?
- [ ] **하위 호환** — 기존 CLI flag/API 응답을 깨지 않는가? 깬다면 deprecation 경로?
- [ ] **i18n 준비** — 사용자 메시지가 string literal 하드코딩 아닌 분리 가능한 구조?

### Round 1.5 합격 기준

- 보안 체크리스트 10개 모두 ✅ 또는 N/A (단 위협 모델 항목은 N/A 불가)
- 사용성 체크리스트 10개 모두 ✅ 또는 N/A (단 사용자 touchpoint 워크스루는 N/A 불가)
- 두 리뷰어가 독립적으로 ✅ Pass 표시

---

## §2. Round 2 — 테스트 가능성 리뷰

**리뷰어**: `qa-test-architect`
**목적**: 구현 전에 "이 설계가 검증 가능한가"를 보증

- [ ] **단위 테스트 범위** — 공개 함수 전부에 단위 테스트 계획이 있는가?
- [ ] **mock 가능성** — 외부 의존성(HTTP, 파일, 프로세스, GPU)을 mock할 추상화가 있는가?
- [ ] **결정론** — 테스트가 race condition·시계·네트워크 타이밍에 취약하지 않은가?
- [ ] **통합 시나리오** — 1개 이상 e2e 흐름이 기술되어 있는가?
- [ ] **CI 재현성** — 로컬과 CI가 같은 결과를 내는 환경 설정이 있는가?
- [ ] **성능 검증** — P99 < 1ms 오버헤드 SLO를 검증할 경로가 있는가?
- [ ] **회귀 테스트** — 과거 버그가 되살아날 때 잡히는가?
- [ ] **테스트 데이터** — fixture/snapshot 파일 위치와 갱신 정책이 명확한가?

---

## §3. Round 3 — 아키텍처 & Rust 관용구 리뷰

**리뷰어**: `chief-architect`
**목적**: Rust 코드 품질 · 시스템 일관성 · 장기 유지보수성 보증

- [ ] **Rust 관용구** — `Result<T, GadgetronError>` 일관 사용 · `?` 연산자 · `async-trait` 필요성
- [ ] **제로 비용 추상화** — 불필요한 `Box` / `Arc` / 힙 할당이 없는가?
- [ ] **제네릭 vs 트레이트 객체** — 선택 근거가 명확한가?
- [ ] **에러 전파** — `From` 구현으로 변환 시 컨텍스트 손실이 없는가?
- [ ] **수명주기** — `'static` 제약이 필요한 곳에 명시되었는가?
- [ ] **의존성 추가** — 신규 crate 추가가 정당한가? 기존 것으로 대체 가능한가?
- [ ] **트레이트 설계** — `#[non_exhaustive]`, default method로 호환성 확보?
- [ ] **관측성** — tracing span/event가 운영에 충분한가?
- [ ] **hot path** — 자주 호출되는 경로에 allocation/clone/lock 비용이 없는가?
- [ ] **문서화** — 공개 API의 rustdoc `///` 주석이 있는가?

---

## 추가 라운드 (PM 재량)

Round 1~3 중 반복 실패가 있거나 큰 변경이 발생하면 PM이 추가 라운드를 소집할 수 있다. 3회는 최소치이지 상한이 아니다.

---

## 리뷰 결과 기록 포맷

각 리뷰어는 해당 설계 문서의 `## 리뷰 로그`에 다음 형식으로 append:

```markdown
### Round N — YYYY-MM-DD — @reviewer-name
**결론**: Pass / Fail / Conditional Pass

**체크리스트**:
- [x] 인터페이스 계약
- [ ] 크레이트 경계 — 문제: `HotSwapManager`가 scheduler에 있어야 하는데 core에 있음

**Action Items**:
- A1: HotSwapManager를 scheduler로 이동
- A2: D-12 준수 재확인

**Open Questions**:
- 없음 / Q-1 등록

**다음 라운드 조건**: A1, A2 반영 후 Round 1 retry
```
