# Assistant Bootstrap UX via `gadgetron init`

> **담당**: @dx-product-lead
> **상태**: Draft
> **작성일**: 2026-04-15
> **최종 업데이트**: 2026-04-15
> **관련 크레이트**: `gadgetron-cli`, `gadgetron-core`, `gadgetron-knowledge`, `gadgetron-penny`
> **Phase**: [P2A] / [P2B]
> **관련 문서**: `docs/design/usability/sprint7-cli-init-nodb.md`, `docs/design/ops/agentic-cluster-collaboration.md`, `docs/design/phase2/00-overview.md`, `docs/design/phase2/04-mcp-tool-registry.md`

---

## 1. 철학 & 컨셉 (Why)

### 1.1 문제 한 문장

Gadgetron의 assistant plane 은 현재 수동 `gadgetron.toml` 작성에 의존하고 있어, 제품 비전은 협업 플랫폼으로 커졌는데 첫 bootstrap 경험은 아직 Phase 1 수준에 머물러 있다.

### 1.2 제품 비전과의 연결

`docs/design/ops/agentic-cluster-collaboration.md` 는 Gadgetron을 assistant / operations / execution 3-plane 협업 플랫폼으로 정의한다. 이 정의가 실제 제품이 되려면, 첫 진입점도 그 모델을 반영해야 한다.

현재의 문제는 두 가지다.

1. **canonical entry point 부재**: Penny/assistant plane 을 위한 정식 bootstrap 경로가 없다.
2. **문서-구현 간 manual gap**: 운영자는 `[agent]`, `[agent.brain]`, `[knowledge]` 를 직접 작성해야만 한다.

이 문서의 목표는 `gadgetron init` 을 **유일한 canonical bootstrap command** 로 승격하는 것이다. 별도 `gadgetron penny init` 을 부활시키지 않는다.

### 1.3 채택하지 않은 대안

| 대안 | 설명 | 채택하지 않은 이유 |
|------|------|--------------------|
| **A. 수동 TOML 유지** | 매뉴얼 예시만 제공하고 사용자가 직접 작성 | 제품 진입 장벽이 높고, Drift가 재발한다 |
| **B. `gadgetron penny init` 부활** | assistant plane 전용 서브커맨드 추가 | 현재 trunk에 없는 별도 진입점을 다시 늘리면 canonical surface가 분열된다 |
| **C. `gadgetron init` 확장** | profile 기반으로 gateway / assistant bootstrap 지원 | 기존 진입점을 유지하면서 새 비전을 가장 자연스럽게 수용한다 |

채택: **C. `gadgetron init` 확장**

### 1.4 핵심 원칙과 trade-off

1. **Single canonical init**: bootstrap 명령은 `gadgetron init` 하나만 둔다.
2. **Profile-based, not auto-detect**: operator 가 `gateway` vs `assistant` 를 명시적으로 선택한다.
3. **Generated config must boot**: 생성된 파일은 `gadgetron serve` 로 바로 기동 가능한 최소 설정이어야 한다.
4. **Bootstrap is setup, not orchestration**: `init` 이 클러스터를 조작하거나 외부 네트워크를 강하게 요구하지는 않는다.
5. **Backward-compatible default**: `gadgetron init --yes` 의 기존 Phase 1 의미를 깨지 않도록 기본 non-interactive profile 은 `gateway` 로 둔다.

Trade-off:

- Assistant profile 을 넣으면 `gadgetron init` 이 더 복잡해진다.
- 그러나 명령이 하나 더 늘어나는 것보다 profile 확장이 더 덜 혼란스럽고, 문서 drift 도 줄인다.

---

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API

`gadgetron-cli` 의 `Init` 서브커맨드를 확장한다.

```rust
#[derive(Clone, Debug, clap::ValueEnum)]
pub enum InitProfile {
    Gateway,
    Assistant,
}

#[derive(Clone, Debug, clap::ValueEnum)]
pub enum InitBrainMode {
    ClaudeMax,
    ExternalAnthropic,
    ExternalProxy,
}

#[derive(Subcommand)]
pub enum Commands {
    Init {
        #[arg(long, short = 'o', default_value = "gadgetron.toml")]
        output: std::path::PathBuf,

        #[arg(long, short = 'y')]
        yes: bool,

        #[arg(long, value_enum)]
        profile: Option<InitProfile>,

        #[arg(long)]
        bind: Option<String>,

        #[arg(long)]
        provider: Option<String>,

        #[arg(long, value_enum)]
        brain_mode: Option<InitBrainMode>,

        #[arg(long)]
        external_base_url: Option<String>,

        #[arg(long)]
        wiki_path: Option<std::path::PathBuf>,

        #[arg(long)]
        enable_web_search: bool,

        #[arg(long)]
        searxng_url: Option<String>,

        #[arg(long, default_value_t = true)]
        starter_wiki: bool,
    },
}
```

#### Contract

- `gadgetron init`
  - TTY + no `--profile`: prompt for `gateway` or `assistant`
  - non-TTY + no `--profile`: default to `gateway` for backward compatibility
- `gadgetron init --profile gateway`
  - current Phase 1 behavior 유지
- `gadgetron init --profile assistant`
  - writes a runnable assistant-plane config
  - creates the assistant workspace directory tree if needed
- `gadgetron init --profile assistant --yes`
  - no prompts, deterministic defaults

#### Non-goals

- `init` does **not** run `claude login`
- `init` does **not** connect to SearXNG
- `init` does **not** modify kube/slurm state
- `init` does **not** create API keys

### 2.2 내부 구조

`cmd_init` 는 giant static string 을 쓰는 대신 profile-aware plan builder 로 바꾼다.

```rust
pub struct InitOptions {
    pub output: PathBuf,
    pub yes: bool,
    pub profile: Option<InitProfile>,
    pub bind: Option<String>,
    pub provider: Option<String>,
    pub brain_mode: Option<InitBrainMode>,
    pub external_base_url: Option<String>,
    pub wiki_path: Option<PathBuf>,
    pub enable_web_search: bool,
    pub searxng_url: Option<String>,
    pub starter_wiki: bool,
}

pub struct InitPlan {
    pub profile: InitProfile,
    pub files: Vec<GeneratedFile>,
    pub notes: Vec<String>,
    pub warnings: Vec<String>,
}

pub struct GeneratedFile {
    pub path: PathBuf,
    pub contents: String,
}
```

Flow:

1. parse CLI flags into `InitOptions`
2. resolve profile
3. if interactive and profile is absent, prompt
4. build `InitPlan`
5. validate plan
6. write files
7. print next-step instructions

#### Rendering model

- `gateway` profile uses the current annotated config baseline
- `assistant` profile renders:
  - `[server]`
  - `[web]`
  - `[agent]`
  - `[agent.brain]`
  - `[knowledge]`
  - optional `[knowledge.search]`
- if `starter_wiki = true`, also write `wiki/README.md`

#### Workspace side effects

Assistant profile:

- create parent directory for `wiki_path`
- create `wiki_path` itself if missing
- optionally create starter page
- do **not** require `.git` to exist yet; knowledge runtime may initialize on first serve

### 2.3 설정 스키마

#### 2.3.1 `gateway` profile output

`gateway` profile is today’s `gadgetron init` output with no semantic change.

#### 2.3.2 `assistant` profile output

```toml
[server]
bind = "127.0.0.1:8080"

[web]
enabled = true
api_base_path = "/v1"

[agent]
binary = "claude"
claude_code_min_version = "2.1.104"
request_timeout_secs = 300
max_concurrent_subprocesses = 4

[agent.brain]
mode = "claude_max"

[knowledge]
wiki_path = "./.gadgetron/wiki"
wiki_autocommit = true
wiki_max_page_bytes = 1048576

# [knowledge.search]
# searxng_url = "http://127.0.0.1:8888"
# timeout_secs = 10
# max_results = 10
```

If `--enable-web-search --searxng-url <URL>` is passed, `[knowledge.search]` is emitted uncommented.

If `--brain-mode external_anthropic --external-base-url <URL>` is passed:

```toml
[agent.brain]
mode = "external_anthropic"
external_base_url = "http://127.0.0.1:4000"
```

Validation rules:

- `external_base_url` requires `brain_mode = external_anthropic|external_proxy`
- `enable_web_search` without `searxng_url` in non-interactive mode is an error
- `provider` is ignored for `assistant` profile in P2A and prints a warning
- `wiki_path` parent must be creatable

### 2.4 에러 & 로깅

No new `GadgetronError` variant is required. `gadgetron-cli` continues to use command-local `anyhow::Error` for write/prompt/render failures.

stdout contract:

- print chosen profile
- print written files
- print exact next steps

Example:

```text
Assistant profile selected.

Config written to gadgetron.toml
Workspace created at ./.gadgetron/wiki

  Next steps:
    1. Run: gadgetron key create
    2. Run: gadgetron serve --config gadgetron.toml --no-db
    3. Open: http://127.0.0.1:8080/web
```

Warnings are printed to stderr:

- `claude` not found on PATH
- `git config user.name/email` missing
- `provider` ignored for assistant profile

#### 2.4.1 Security & Threat Model (STRIDE)

This section is required for Round 1.5 per `docs/process/03-review-rubric.md §1.5-A`.

**Assets**

| Asset | Sensitivity | Owner |
|------|-------------|-------|
| generated `gadgetron.toml` | Medium — contains local topology and assistant config | Operator |
| `wiki_path` directory and starter wiki pages | Medium — may later hold private notes | User / Operator |
| terminal output from `gadgetron init` | Low — should contain instructions only, not secrets | Operator |
| selected assistant brain mode and external base URL | Medium — can redirect assistant traffic if misconfigured | Operator |

**Trust boundaries**

| ID | Boundary | Crosses | Auth mechanism |
|----|----------|---------|----------------|
| B-I1 | operator shell → `gadgetron init` | CLI input | local OS user |
| B-I2 | `gadgetron init` → filesystem | config and wiki file writes | local OS permissions |
| B-I3 | generated config → `gadgetron serve` | offline handoff into runtime parse | same workspace / operator control |

**STRIDE table**

| Component | S | T | R | I | D | E | Highest unmitigated risk |
|-----------|---|---|---|---|---|---|--------------------------|
| CLI option parser | Low | Low | Low | Low | Low | Low | None — local-only entry |
| assistant config renderer | Low | Medium — invalid combinations can emit broken config | Low | Low | Medium — unusable bootstrap blocks first run | Low | emitted config drifts from runtime contract |
| workspace file writer | Low | Medium — wrong path may overwrite unintended local file | Low | Low | Medium — non-creatable path blocks setup | Low | operator-selected output path mistake |
| stdout/stderr guidance | Low | Low | Low | Medium — misleading output could leak path confusion or bad steps | Low | Low | docs/generator drift causing incorrect next steps |

**Mitigations**

| ID | Mitigation | Location |
|----|------------|----------|
| M-I1 | assistant renderer emits canonical `[agent]`, `[agent.brain]`, `[knowledge]` only, never legacy `[penny]` | §2.3 + unit tests in §4 |
| M-I2 | invalid `brain_mode` / `external_base_url` / search combinations fail before file write | §2.3 validation rules |
| M-I3 | bootstrap does not generate secrets, perform network calls, or mutate cluster state | §2.1 non-goals |
| M-I4 | generated config must parse and boot through `gadgetron serve --no-db` in integration tests | §5.1 |
| M-I5 | manual docs are updated in the same landing sequence so stdout guidance and manuals do not drift | §2.6 |

### 2.5 의존성

No new external crate is required.

Reuse:

- existing `clap` value enums
- existing `toml` rendering approach
- existing filesystem primitives

### 2.6 구현 순서

Implementation should land in narrow slices so the generated config, runtime parsing, and manual docs stay synchronized.

1. **CLI surface**
   - add `InitProfile`, `InitBrainMode`, and new `init` flags
   - preserve existing gateway behavior when no assistant profile is selected
2. **Plan/render path**
   - split `cmd_init` into option parse → plan build → validate → write
   - add assistant config renderer and starter wiki writer
3. **Boot verification**
   - add integration tests that parse generated config and boot `serve --no-db`
   - assert `/v1/models` contains `penny` for assistant profile
4. **Manual sync**
   - update `docs/manual/configuration.md`, `docs/manual/quickstart.md`, and `docs/manual/penny.md`
   - replace manual-only assistant bootstrap guidance with `init --profile assistant`

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 연결 구조

```text
operator
  -> gadgetron init
      -> init plan builder
      -> config renderer
      -> workspace file writer
      -> gadgetron.toml + starter wiki
  -> gadgetron serve
      -> AppConfig::load()
      -> register_penny_if_configured()
      -> /web assistant entry point
```

### 3.2 크레이트 경계

- `gadgetron-cli`
  - owns prompt flow, plan builder, file writing
- `gadgetron-core`
  - owns canonical config structs (`[agent]`, `[agent.brain]`)
- `gadgetron-knowledge`
  - owns `[knowledge]` runtime semantics, but not bootstrap prompts
- `gadgetron-penny`
  - consumes generated config, but does not participate in `init`

This preserves D-12 boundaries:

- no prompt logic in `gadgetron-core`
- no file-write bootstrap logic in `gadgetron-penny`
- no assistant orchestration logic in `gadgetron-knowledge`

### 3.3 타 문서와의 계약

- `docs/design/usability/sprint7-cli-init-nodb.md`
  - remains canonical for `gateway` bootstrap
- `docs/manual/configuration.md`
  - must be updated to say `init --profile assistant` is the recommended assistant entry path once implemented
- `docs/manual/penny.md`
  - manual TOML authoring becomes fallback, not primary guidance

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

| 대상 | 검증 invariant |
|------|----------------|
| profile resolver | TTY / non-TTY / `--yes` 조합에서 profile 선택이 deterministic 해야 함 |
| assistant renderer | output must contain `[agent]`, `[agent.brain]`, `[knowledge]`, no `[penny]` |
| gateway renderer | current Phase 1 template와 의미상 동일해야 함 |
| plan validator | invalid `brain_mode` / `external_base_url` 조합은 실패해야 함 |
| starter wiki writer | assistant profile writes starter page only when requested |

### 4.2 테스트 하네스

- pure unit tests for renderer + validator
- CLI parse tests with `clap`
- tempdir-based filesystem tests

Named tests:

- `init_noninteractive_defaults_to_gateway_profile`
- `init_assistant_profile_renders_agent_sections`
- `init_assistant_profile_never_renders_legacy_penny_section`
- `init_assistant_profile_creates_workspace_tree`
- `init_external_base_url_requires_external_brain_mode`
- `init_enable_web_search_requires_searxng_url_in_yes_mode`

### 4.3 커버리지 목표

- line coverage 85%+
- branch coverage 80%+
- renderer / validator paths 90%+

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

1. `gadgetron init --profile assistant --yes`
   - writes config + starter wiki
   - generated config parses via `AppConfig::load`

2. assistant profile → `gadgetron serve --no-db`
   - server boots
   - `penny` registers
   - `/v1/models` contains `penny`

3. assistant profile with search enabled
   - generated config contains valid `[knowledge.search]`
   - `gadgetron mcp serve` exposes `web.search`

### 5.2 테스트 환경

- temp directories
- no real Claude Code invocation required
- optional fake SearXNG URL string validation only

### 5.3 회귀 방지

These tests must fail if:

- assistant bootstrap regresses to manual-only config
- legacy `[penny]` becomes the emitted config again
- generated config cannot boot with `gadgetron serve`
- docs/examples and generated assistant config diverge

---

## 6. Phase 구분

| 항목 | Phase |
|------|-------|
| `gadgetron init --profile gateway` 유지 | [P1] |
| `gadgetron init --profile assistant` | [P2A] |
| starter wiki generation | [P2A] |
| `doctor` assistant-profile checks (`claude`, wiki path, search URL`) | [P2B] |
| collaboration/ops profile (`cluster`, `workload`) | [P3] |

---

## 7. 오픈 이슈 / 의사결정 필요

| ID  | 내용 | 옵션 | 추천 | 상태 |
|-----|------|------|------|------|
| Q-1 | `gadgetron init --yes` 기본 profile 을 무엇으로 둘 것인가 | A: gateway / B: assistant | A — Phase 1 backward compatibility 보존 | 🟡 PM 검토 요청 |
| Q-2 | assistant profile 에 direct provider quick-start(`--provider`)를 허용할 것인가 | A: 무시 / B: provider block 추가 | A — P2A는 assistant bootstrap을 먼저 닫고 혼합 프로필은 후속 | 🟡 PM 검토 요청 |
| Q-3 | init 시 `.git` repo까지 만들 것인가 | A: yes / B: serve-time init에 맡김 | B — bootstrap은 file tree만 보장하고 runtime init과 중복을 피함 | 🟡 PM 검토 요청 |

---

## 리뷰 로그 (append-only)

### Round 1 — 2026-04-15 — 예정
**결론**: 미실시

**체크리스트**: (`03-review-rubric.md §1` 기준)
- [ ] 인터페이스 계약
- [ ] 크레이트 경계
- [ ] 타입 중복
- [ ] 에러 반환
- [ ] 동시성
- [ ] 의존성 방향
- [ ] Phase 태그
- [ ] 레거시 결정 준수

**다음 라운드 조건**: Round 1 리뷰어(@chief-architect, @dx-product-lead) 검토 후

### Round 1.5 — 2026-04-15 — 예정
**결론**: 미실시
(`03-review-rubric.md §1.5` 기준)

### Round 2 — 2026-04-15 — 예정
**결론**: 미실시
(`03-review-rubric.md §2` 기준)

### Round 3 — 2026-04-15 — 예정
**결론**: 미실시
(`03-review-rubric.md §3` 기준)
