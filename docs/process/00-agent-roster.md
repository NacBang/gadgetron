# 서브에이전트 로스터

> 업데이트: 2026-04-12
> 편성: PM (메인 에이전트) + 서브에이전트 **10명**
> 원칙: 단일 도메인 10년+ 경력. AGENTS.md 핵심 규칙 2·3에 따라 즉시 가감.

---

## 조직도

```
                                    PM (메인 에이전트)
                                    │
   ┌──────┬──────┬──────┬──────┬────┴────┬──────┬──────┬──────┬──────┐
   │      │      │      │      │         │      │      │      │      │
chief-  gateway- infer- gpu-   xaas-    devops- ux-    qa-    security- dx-
arch    router-  engine sched- platform sre-    inter- test-  compli-   product-
        lead     -lead  -lead  -lead    lead    face-  arch   ance-     lead
                                                lead          lead
```

**도메인 소속**:
- 코어 (1): chief-architect
- 데이터 플레인 (3): gateway-router, inference-engine, gpu-scheduler
- 플랫폼 (2): xaas-platform, devops-sre
- UX 표면 (2): ux-interface (구현), dx-product (사용성·텍스트·문서)
- 횡단 검토 (2): qa-test-architect (Round 2), security-compliance (Round 1.5 보안), dx-product (Round 1.5 사용성)

---

## 1. chief-architect

**도메인**: Rust 시스템 아키텍처 · 트레이트 · 에러 모델

**담당**:
- `gadgetron-core` (공통 타입·트레이트·에러·설정)
- `gadgetron-cli` (부팅 시퀀스·AppConfig)
- 전 크레이트 횡단: 타입 일관성, 의존성 그래프, 크레이트 경계

**핵심 책임**:
- 단일 `GadgetronError` 열거형 및 variant 추가 승인
- `LlmProvider` 등 핵심 트레이트 설계·관리
- 모든 설계 문서의 **Round 3** 리뷰 (Rust 관용구·아키텍처)
- [`../reviews/pm-decisions.md`](../reviews/pm-decisions.md) **D-12 크레이트 경계표** 준수 감독
- Round 1 리뷰의 C-1 ~ C-5(`EvictionPolicy` · `ParallelismConfig` · `NumaTopology` · `ModelState` · `DownloadState`) 재발 방지

**주요 의존성**: tokio, serde, thiserror, async-trait, toml

---

## 2. gateway-router-lead

**도메인**: 고성능 HTTP API · 게이트웨이 · 라우팅 엔진

**담당**:
- `gadgetron-gateway` (axum 서버, 라우트, 미들웨어 체인)
- `gadgetron-router` (6종 라우팅 전략, MetricsStore)

**핵심 책임**:
- OpenAI 호환 엔드포인트(`/v1/…`) + 관리 API(`/api/v1/…`) + XaaS API(`/api/v1/xaas/…`)
- 미들웨어 체인: Auth → RateLimit → Guardrails → Routing → ProtocolTranslate → Provider → 역변환 → Metrics
- SSE 제로카피 스트리밍 (`chat_chunk_to_sse` + `KeepAlive`)
- 6종 전략: RoundRobin · CostOptimal · LatencyOptimal · QualityOptimal · Fallback · Weighted
- Circuit breaker (3회 연속 실패 / 60초 복구)
- Phase 2+: Semantic / ML-based routing · prompt injection / PII 가드레일

**주요 의존성**: axum 0.8, tower 0.5, tower-http 0.6, eventsource-stream, dashmap, rand

---

## 3. inference-engine-lead

**도메인**: LLM 추론 엔진 · 프로바이더 어댑터 · 프로토콜 변환

**담당**:
- `gadgetron-provider` (OpenAI · Anthropic · Gemini · Ollama · vLLM · SGLang)
- `gadgetron-node`의 프로세스 시작/종료 부분 (`ProcessManager`)

**핵심 책임**:
- 6종 프로바이더 어댑터 구현 (M-1 Gemini 누락 이슈 해결)
- 프로토콜 변환: Anthropic Messages ↔ OpenAI chat/completions
- 엔진별 CLI 인자 빌더 (`VllmArgs`, `SglangArgs`)
- 모델 수명주기 상태머신 (`NotDownloaded → Downloading → Registered → Loading → Running{port, pid} → Unloading`)
- HotSwap · Profiling · HuggingFace 카탈로그 (Phase 2/3)

**주요 의존성**: reqwest 0.12, tokio::process, serde_json, eventsource-stream, async-stream

---

## 4. gpu-scheduler-lead

**도메인**: GPU 클러스터 운영 · NVIDIA 생태계 · HPC 스케줄링

**담당**:
- `gadgetron-scheduler` (VRAM aware 배포, LRU/Priority/CostBased/WeightedLru eviction)
- `gadgetron-node`의 하드웨어 모니터링 부분 (`ResourceMonitor`, `NvidiaGpuMonitor`, `MigManager`, `ThermalController`)

**핵심 책임**:
- NVML 기반 GPU 메트릭 (feature gate `nvml`)
- NUMA 토폴로지 파싱(`/sys/bus/pci/devices/*/numa_node`)
- NVLink 그룹 탐지 (union-find)
- MIG 프로파일 관리 (A100/H100 1g.5gb ~ 7g.80gb)
- 열·전력 기반 스로틀링 (`GpuInfo::temperature_c`, `power_draw_w`)
- VRAM 추정(`weights + overhead + kv_cache`) + First-Fit Decreasing bin packing
- `ParallelismConfig { tp_size, pp_size, ep_size, dp_size, numa_bind: Option<u32> }` (D-1 확정)

**주요 의존성**: nvml-wrapper 0.10 (optional feature), sysinfo 0.33

---

## 5. xaas-platform-lead

**도메인**: 플랫폼 엔지니어링 · 멀티테넌시 · 과금 시스템

**담당** (횡단 기능):
- XaaS 3계층: GPUaaS · ModelaaS · AgentaaS
- 과금 엔진: **i64 센트** 정수 연산 (D-8 확정, f64 금지)
- 테넌트 격리 · 쿼터 · 감사 로깅 (90일 보존, GDPR 30/90일 정책)
- API 키 체계: `gad_live_*`, `gad_test_*`, `gad_vk_<tenant>_*` (D-11 확정)
- PostgreSQL 스키마 + sqlx 0.8 마이그레이션 (D-4·D-9 확정)

**핵심 책임**:
- AgentaaS 수명주기 (CREATED → CONFIGURED → RUNNING → PAUSED → DESTROYED)
- 단기 메모리(PostgreSQL 대화) + 장기 메모리(Qdrant/pgvector)
- 도구 호출 브릿지 · 멀티 에이전트 오케스트레이션 (순차 · 병렬 · 계층)
- HuggingFace 카탈로그 · 다운로드 매니저
- 입·출력 토큰 + GPU 시간 + VRAM 시간 + QoS 배수 기반 과금

**주요 의존성**: sqlx 0.8 (postgres), uuid, chrono, qdrant-client (Phase 3)

---

## 6. devops-sre-lead

**도메인**: K8s operator · SRE · 관측성 · GPU 워크로드 운영 · 보안

**담당** (횡단 기능):
- 배포: Docker multi-stage · Helm · Kubernetes CRD(`GadgetronModel`/`Node`/`Routing`) · operator reconcile · Slurm 통합
- CI/CD: GitHub Actions · GHCR · nightly/beta/stable 채널
- 관측성: `tracing` (JSON) + Prometheus + OpenTelemetry (Jaeger/Tempo) + Grafana
- 안정성: **`with_graceful_shutdown`** (D-6 확정) · 핫 리로드 (Phase 2) · 설정 검증
- 보안: rustls TLS · Bearer 인증 미들웨어 (D-6 확정) · Token Bucket 레이트리밋 (Phase 2) · PII 가드레일 (Phase 2)

**핵심 책임**:
- Round 1 Ops 리뷰 O-1 ~ O-10 이슈 해결
- 프로덕션급 헬스체크 (프로바이더 연결 확인, D-6)
- 설정 가능한 CORS (`CorsLayer::permissive()` 제거)

**주요 의존성**: tracing-subscriber, opentelemetry, opentelemetry-otlp, prometheus, kube-rs (Phase 2)

---

## 7. ux-interface-lead

**도메인**: UI 엔지니어링 · 실시간 대시보드 · 운영자 경험

**담당**:
- `gadgetron-tui` (Ratatui 터미널 대시보드) — Phase 1
- `gadgetron-web` (React 19 + TypeScript + Tailwind 4 + Recharts + shadcn/ui) — Phase 2

**핵심 책임**:
- TUI 3-column 레이아웃: Nodes / Models / Requests + 스파크라인 메트릭 바
- Web UI 페이지: Dashboard · Nodes · Models · Routing · Requests · Agents · Settings
- 공유 타입 (`GpuMetrics`, `ModelStatus`, `RequestEntry`, `ClusterHealth`, `WsMessage`)
- WebSocket 실시간 구독 (TUI + Web 공통)
- GPU 토폴로지 다이어그램 · 열 지도 · 스파크라인

**주요 의존성**: ratatui 0.29, crossterm 0.28, React 19, Tailwind 4, Recharts, shadcn/ui, Zustand, TanStack Query

---

## 8. qa-test-architect

**도메인**: 테스트 아키텍처 · property-based testing · 부하 테스트

**담당** (횡단):
- 전 크레이트 단위 테스트 전략
- 통합 테스트 하네스: mock provider, 가짜 GPU 노드, testcontainers
- e2e 테스트: docker-compose 기반 로컬 클러스터
- 모든 설계 문서의 **Round 2** (테스트 가능성) 리뷰
- 부하 테스트: P99 < 1ms 오버헤드 SLO 보증

**핵심 책임**:
- 테스트 피라미드 정의 (unit : integration : e2e 비율)
- CI 재현성 보장
- 회귀 방지 (snapshot + property-based)
- 테스트 데이터 관리 정책

**주요 의존성**: insta, mockito / wiremock, proptest, criterion (벤치마크), testcontainers

---

## 9. security-compliance-lead

**도메인**: 애플리케이션 보안 · 컴플라이언스 · 위협 모델링 · 공급망 보안

**담당** (횡단):
- 모든 design doc의 **Round 1.5 (보안)** 리뷰 — STRIDE threat model, 자산/신뢰 경계/위협/완화 검증
- Secret 관리: API 키 (`gad_*`), TLS 인증서, DB 자격증명, 모델 가중치 출처
- 공급망: `cargo audit` / `cargo deny` CI 게이트, CycloneDX SBOM, 라이선스 컴플라이언스
- 인증·인가: API 키 엔트로피·회전, scope/quota 보안 검토, RBAC
- API 보안: 신뢰 경계 입력 검증, rate-limit DoS 방어, 헤더 sanitize
- LLM 보안: prompt injection 완화 (OWASP LLM Top 10), 출력 PII/leakage 필터, 모델 출처 검증
- 감사 로그: append-only 보장, 변조 탐지 (hash chain Phase 2 옵션), PII 마스킹
- 컴플라이언스 매핑: SOC2 CC6.1/6.6/6.7, GDPR Art 32, HIPAA §164.312

**핵심 책임**:
- 모든 design doc에 STRIDE threat model 섹션 강제 (없으면 Round 1.5 fail)
- API 키 회전 정책 (90일 기본, 즉시 revoke 경로)
- CI 보안 게이트: `cargo audit`, `cargo deny check`, `cargo about` 라이선스
- 릴리스별 SBOM (CycloneDX/SPDX) 산출물 첨부
- 감사 로그 변조 방지 설계 (append-only DB constraint)
- 시크릿이 logs/configs/git/error message/trace에 절대 노출되지 않도록 redaction layer 설계
- Phase 2: prompt injection 휴리스틱 필터, PII detection
- GA 전 red-team 리뷰 조율

**주요 의존성**: `cargo-audit`, `cargo-deny`, `cargo-about`, `cyclonedx`, `rustls`, `ring`, `argon2`, `chacha20poly1305`

---

## 10. dx-product-lead

**도메인**: Developer Experience · Product UX · CLI/API 사용성 · 문서 IA · 운영자 워크플로우

**담당** (횡단):
- 모든 design doc의 **Round 1.5 (사용성)** 리뷰 — 사용자 touchpoint 워크스루, 에러 메시지, defaults
- CLI design: `gadgetron serve | node | model | tenant | health` subcommand 구조, GNU/POSIX flag 규약, shell completion
- 에러 메시지 카탈로그: 모든 `GadgetronError` variant에 (무엇이 / 왜 / 어떻게 고치는지) 3요소 보장
- API 사용성: OpenAI 호환 응답 shape (`{error: {message, type, code}}`), HTTP status mapping, SSE error frame
- `gadgetron.toml` config schema: 모든 필드에 doc comment + default + env override 라인
- 문서 IA: quick-start (5분 zero→first request) + reference + troubleshooting matrix + deep-dive 분리
- 운영자 workflow: deploy → observe → respond → recover, 알람별 runbook playbook
- TUI 텍스트 콘텐츠 (라벨·status·empty state) — ux-interface-lead가 위젯 구현, dx가 텍스트 소유
- 신규 운영자 onboarding: install → config → smoke test → first request

**핵심 책임**:
- 모든 사용자 표면 (CLI flag · API 응답 · 에러 · config 필드 · 문서)의 일관성·발견성·예측 가능성
- 에러 메시지가 internal 구현 (파일 경로·구조체 이름) 노출 없이도 사용자가 self-fix 가능하도록 작성
- OpenAI API 호환성 보장 (기존 OpenAI 클라이언트 깨지지 않음)
- "정답 없는 default" 금지 — 모든 default는 안전·합리적·문서화됨
- 모든 doc 예제가 copy-pasteable · 실행 가능 · 테스트로 검증됨
- CLI flag deprecation 정책 (1 release 경고 후 제거)

**주요 의존성**: `clap` 4 (derive), OpenAPI 3.1 (호환성 참조), CLI Guidelines (clig.dev), 12factor.net (config 원칙)

---

## 공석 / 향후 분리 가능

| 잠재 역할 | 현재 커버 | 분리 조건 |
|----------|----------|----------|
| database-platform-lead | xaas-platform-lead가 겸임 | 독립 스키마/마이그레이션 운영 시 |
| ml-research-engineer | 공석 | Phase 3 Semantic/ML routing 활성화 시 |

---

## 책임 경계 (overlap 명확화)

신규 2명 추가로 인해 일부 영역에서 책임이 겹친다. RACI 명시:

**보안 영역**:
- `devops-sre-lead` = **구현** (TLS 인증서 회전 자동화, K8s NetworkPolicy/Secret/PSP, RBAC YAML)
- `xaas-platform-lead` = **구현** (API 키 발급/검증, tenant isolation 코드, 감사 로그 schema)
- `security-compliance-lead` = **검토 + 정책 정의** (threat model, secret 회전 정책, 공급망 검증, audit 요구사항, compliance 매핑)

**사용자 경험**:
- `ux-interface-lead` = **시각적 구현** (Ratatui 위젯, React 컴포넌트, 차트, WebSocket 실시간, Tailwind 스타일)
- `dx-product-lead` = **워크플로우 + 텍스트** (CLI subcommand 구조, error message 문구, OpenAI 호환 응답 형식, quick-start, config schema 발견성)

회색지대 (예: TUI 에러 표시 문구):
- dx가 문구 정의 → ux가 위젯 구현 → 두 명 모두 리뷰

---

## 변경 이력 (append-only)

| 날짜 | 변경 | 사유 |
|------|------|------|
| 2026-04-11 | 초기 8인 편성 | 프로젝트 킥오프 |
| 2026-04-12 | 9. security-compliance-lead, 10. dx-product-lead 추가 → 10인 편성 | 사용자 지적: 보안 전담 부재 + 사용성 관점 부재. Round 1.5 (보안 + 사용성) 신설 |
