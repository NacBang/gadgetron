# Configuration

Gadgetron is configured through three mechanisms: CLI flags, environment variables, and `gadgetron.toml`. For the current canonical local operator loop, use [quickstart.md](quickstart.md): `./demo.sh build|start|status|logs|stop` with a pgvector-enabled PostgreSQL. This page is the field reference and precedence guide.

When the same setting is supplied by more than one mechanism, the order of precedence from highest to lowest is:

1. **CLI flags** — always win
2. **Environment variables** — override the config file
3. **`gadgetron.toml`** — baseline configuration
4. **Built-in defaults** — used when none of the above supply a value

---

## CLI flags

These flags are accepted by `gadgetron serve`. They take precedence over environment variables and `gadgetron.toml`.

### `--config <PATH>`

Path to the TOML configuration file.

- Default: `./gadgetron.toml` (relative to the working directory at process start)
- Example: `gadgetron serve --config /etc/gadgetron/gadgetron.toml`

Equivalent environment variable: `GADGETRON_CONFIG`

If the file does not exist, the server starts with built-in defaults. If the file exists but cannot be parsed, the server exits with an error identifying the malformed field.

---

### `--bind <ADDR>`

The TCP address and port the HTTP server listens on.

- Default: `0.0.0.0:8080`
- Example: `gadgetron serve --bind 127.0.0.1:9000`

Equivalent environment variable: `GADGETRON_BIND`

Overrides `[server].bind` in `gadgetron.toml`.

---

### `--tui`

Launch the real-time TUI dashboard alongside the gateway server. When set, the terminal is taken over by the dashboard and all gateway request events are displayed in the Requests panel as they occur.

- Default: off (server runs in headless mode with log output to stdout)
- Example: `gadgetron serve --tui`

No equivalent environment variable or `gadgetron.toml` field. This flag is only meaningful in interactive terminal sessions.

---

### `--no-db`

Force no-db mode even when `GADGETRON_DATABASE_URL` is set.

- Default: off
- Example: `gadgetron serve --no-db`

In this mode, Gadgetron skips PostgreSQL, accepts format-valid API keys without database lookup, and disables quota persistence. This is useful for local development and quick evaluation, but not for production.

---

### `--provider <URL>`

Quick-start mode for a single vLLM-compatible endpoint.

- Example: `gadgetron serve --provider http://10.100.1.5:8100`
- Equivalent environment variable: `GADGETRON_PROVIDER`

When set, Gadgetron skips config file loading, injects one synthetic provider named `provider`, and implies no-db mode. This is the fastest path for testing against a single local or remote vLLM server.

---

## `gadgetron init` — generate an annotated config file

`gadgetron init` writes an annotated baseline `gadgetron.toml` to the current directory. Today that template is still **gateway-first**: it emits the shipped baseline sections for server/router/provider setup, but it does **not** yet emit the assistant-specific `[agent]`, `[agent.brain]`, `[knowledge]`, or `[knowledge.search]` blocks.

```sh
./target/release/gadgetron init
./target/release/gadgetron init --output /etc/gadgetron/gadgetron.toml
./target/release/gadgetron init --yes
```

If the target file already exists and `--yes` is not passed, the command prompts before overwriting it. In non-interactive mode without `--yes`, it leaves the existing file unchanged and exits successfully.

After running `gadgetron init`, open the generated file and follow the inline comments. If you need Penny and the knowledge layer, add the `[agent]`, `[agent.brain]`, `[knowledge]`, and optional `[knowledge.search]` blocks manually per [penny.md](penny.md). If you want a zero-config single-provider test instead, use `gadgetron serve --provider <URL>`.

---

## Environment variables

#### `GADGETRON_DATABASE_URL`

PostgreSQL connection URL for full database-backed mode.

```
GADGETRON_DATABASE_URL=postgres://user:password@localhost:5432/gadgetron
```

When this variable is set, `gadgetron serve` connects to PostgreSQL, runs migrations, and enables persistent tenant/key validation. When it is unset or empty, the server starts in no-db mode instead. The variable is required for PostgreSQL-backed commands such as `gadgetron tenant create`, `gadgetron key list`, and `gadgetron key revoke`.

Standard PostgreSQL connection string format. For connection pool tuning, the server creates a pool with a maximum of 20 connections and a 5-second acquire timeout (these are not yet configurable via environment variable).

This value is treated as a secret internally (`Secret<String>`). It is never written to logs or tracing spans.

---

#### `GADGETRON_BIND`

The TCP address and port the HTTP server listens on.

- Default: `0.0.0.0:8080` (from `gadgetron.toml` `[server].bind`, or the built-in default when no config file is present)
- Override: `GADGETRON_BIND=127.0.0.1:9000`

When set, `GADGETRON_BIND` overrides `[server].bind` in `gadgetron.toml`.

---

#### `GADGETRON_CONFIG`

Path to the TOML configuration file.

- Default: `./gadgetron.toml` (relative to the working directory at process start)
- Override: `GADGETRON_CONFIG=/etc/gadgetron/gadgetron.toml`

If the file at this path does not exist, the server starts with built-in defaults (bind `0.0.0.0:8080`, no providers, empty routing). If the file exists but cannot be parsed, the server exits with an error message identifying the malformed field.

---

#### `RUST_LOG`

Controls log verbosity using the [`tracing-subscriber` `EnvFilter` syntax](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html).

- Default: `gadgetron=info,tower_http=info`
- Common values:
  - `RUST_LOG=debug` — verbose output for all crates
  - `RUST_LOG=gadgetron=debug,tower_http=warn` — Gadgetron debug, suppress HTTP framework noise
  - `RUST_LOG=gadgetron=trace` — full trace including middleware internals

---

## gadgetron.toml reference

All fields are optional at the file level (the server boots without a config file). Fields marked "(required)" must be present if their parent section is present.

### `[server]`

```toml
[server]
# TCP bind address for the HTTP server.
# Override with GADGETRON_BIND environment variable.
# Default: "0.0.0.0:8080"
bind = "0.0.0.0:8080"
```

Two `ServerConfig` fields are parsed by the config loader but not currently consumed by the gateway runtime: `request_timeout_ms` (default 30000) and `api_key`. Leaving them absent is recommended; if present, they are ignored without error. Both are tracked for future wiring and should not be relied on as working knobs.

---

### `[router]`

```toml
[router]
# Routing strategy applied when more than one provider is configured.
# Valid values (as inline table with type field):
#   {type = "round_robin"}          — cycle providers in order (default)
#   {type = "cost_optimal"}         — prefer the provider with the lowest estimated cost
#   {type = "latency_optimal"}      — prefer the provider with the lowest average latency
#   {type = "quality_optimal"}      — prefer the provider with the lowest error rate
#   {type = "fallback", chain = ["openai", "anthropic"]}  — try providers in order
#   {type = "weighted", weights = {openai = 0.7, anthropic = 0.3}}
# Default: {type = "round_robin"}
default_strategy = {type = "round_robin"}
```

`default_strategy` is a TOML inline table (tagged enum). The `type` key selects the variant; some variants take additional keys in the same inline table. The value `{type = "round_robin"}` is the minimal form with no extra keys. All provider names in `chain` and `weights` must match the `[providers.*]` keys defined in the same config file.

`router.fallbacks` and `router.costs` are also accepted but are for advanced routing configuration not covered in this manual. Leave them absent to use defaults.

---

### `[providers]`

Each key under `[providers]` is a provider name you choose (used in routing and model listing). The `type` field selects the provider adapter.

**Supported provider types on trunk:** `openai`, `anthropic`, `gemini`, `ollama`, `vllm`, `sglang`

#### OpenAI

```toml
[providers.openai]
type = "openai"

# (required) Your OpenAI API key.
# Use ${ENV_VAR} syntax to read from an environment variable at runtime.
api_key = "${OPENAI_API_KEY}"

# (optional) Override the OpenAI base URL. Useful for Azure OpenAI or
# OpenAI-compatible self-hosted endpoints.
# Default: uses the OpenAI provider's built-in default (api.openai.com)
# base_url = "https://your-azure-endpoint.openai.azure.com/"

# (required) List of model IDs this provider can serve.
models = ["gpt-4o", "gpt-4o-mini", "gpt-3.5-turbo"]
```

#### Anthropic

```toml
[providers.anthropic]
type = "anthropic"

# (required) Your Anthropic API key.
api_key = "${ANTHROPIC_API_KEY}"

# (optional) Override the Anthropic base URL.
# base_url = "https://your-proxy.example.com/"

# (required) List of model IDs this provider can serve.
models = ["claude-opus-4-5", "claude-sonnet-4-5"]
```

#### Ollama

```toml
[providers.ollama]
type = "ollama"

# (required) Full URL to your Ollama instance, including port.
endpoint = "http://localhost:11434"
```

Ollama does not require an API key. The `models` field is not used for Ollama; available models are discovered from the Ollama API at runtime.

#### Gemini

```toml
[providers.gemini]
type = "gemini"

# (required) Google AI Studio API key (shell expansion via $VAR / ${VAR}).
api_key = "${GEMINI_API_KEY}"

# (required) List of model IDs this provider can serve.
models = ["gemini-2.5-pro", "gemini-2.5-flash"]
```

Gemini uses the default Google AI Studio endpoint; there is no `endpoint` or `base_url` field on this provider type. The `api_key` is required and is adapted to Google's request/response shape internally.

#### vLLM

```toml
[providers.gemma4]
type = "vllm"

# (required) Full URL to your vLLM instance, including port.
# Override with an environment variable if needed: endpoint = "${VLLM_ENDPOINT}"
endpoint = "http://10.100.1.5:8100"
```

`ProviderConfig::Vllm` accepts only `endpoint` and optional `api_key` — there is **no `models` field**. Gadgetron discovers the served models from vLLM's OpenAI-compatible `GET /v1/models` endpoint at runtime (same pattern as Ollama). If the model list is empty or missing, verify the vLLM server is running and that its `--served-model-name` / `--model` arguments are set (see [vLLM: OpenAI-Compatible Server](https://docs.vllm.ai/en/stable/serving/openai_compatible_server/)).

vLLM does not require an API key when running in its default open mode. If your vLLM instance has `--api-key` configured, add `api_key = "${VLLM_API_KEY}"`.

#### SGLang

```toml
[providers.glm]
type = "sglang"

# (required) Full URL to your SGLang instance, including port.
endpoint = "http://10.100.1.110:30000"
```

`ProviderConfig::Sglang` accepts only `endpoint` and optional `api_key` — there is **no `models` field**. Gadgetron discovers the served model from SGLang's OpenAI-compatible `GET /v1/models` endpoint at runtime (see [SGLang: OpenAI APIs](https://docs.sglang.io/basic_usage/openai_api_completions.html)). SGLang typically serves one model per process.

SGLang does not require an API key by default. For reasoning models such as GLM-5.1, Gadgetron forwards the `reasoning_content` field in the response if the model returns it. See [api-reference.md](api-reference.md) for the field description.

---

### `[web]`

Controls the embedded Web UI served under `/web/*`.

```toml
[web]
enabled = true
api_base_path = "/v1"
```

- `enabled`: `false`면 `/web/*` subtree 자체를 mount하지 않습니다.
- `api_base_path`: 브라우저가 호출할 API prefix. 기본값은 `/v1`이며, reverse proxy가 경로를 재작성할 때만 변경하십시오.

---

### `[agent]`

Penny subprocess runtime의 상위 설정입니다.

```toml
[agent]
binary = "claude"
claude_code_min_version = "2.1.104"
request_timeout_secs = 300
max_concurrent_subprocesses = 4
session_mode = "native_with_fallback"
session_ttl_secs = 86400
session_store_max_entries = 10000
# session_store_path = "/var/lib/gadgetron/sessions"  # 생략 시 gadgetron serve의 cwd 사용
```

- `binary`: Claude Code CLI 경로 또는 basename
- `claude_code_min_version`: 허용되는 최소 Claude Code 버전
- `request_timeout_secs`: 단일 Penny 요청 제한 시간 (범위: [10, 3600])
- `max_concurrent_subprocesses`: 동시 Claude Code subprocess 상한 (범위: [1, 32])
- `session_mode`: 네이티브 Claude Code 세션 정책. `native_with_fallback` (기본) — 네이티브 세션 먼저 시도, 실패 시 stateless fallback. `native_only` — 네이티브 세션 필수; 세션 초기화 실패 시 요청 거부. `stateless_only` — 항상 stateless 모드로 동작.
- `session_ttl_secs`: `SessionStore` 항목의 TTL(초). 범위 [60, 604800]. 기본값 86400 (24시간).
- `session_store_max_entries`: LRU 퇴출 전 `SessionStore`의 최대 항목 수. 범위 [1, 1000000]. 기본값 10000.
- `session_store_path`: Claude Code 프로젝트 디렉터리 오버라이드 (`-p` 인수용). 생략 시 `gadgetron serve` 시작 시점의 cwd 사용. 설정 시 해당 경로는 존재하는 쓰기 가능한 디렉터리여야 합니다.

### `[agent.brain]`

```toml
[agent.brain]
mode = "claude_max"

# external_anthropic 모드:
# external_anthropic_api_key_env = "ANTHROPIC_API_KEY"  # API 키가 담긴 환경변수명 (기본값)

# external_proxy 모드:
# external_base_url = "http://127.0.0.1:4000/v1"        # 필수; proxy endpoint

# gadgetron_local 모드 (P2C — P2A에서는 startup error):
# local_model = "ollama/gemma3:27b"
```

지원 모드:

| 모드 | 설명 | 필수 추가 필드 |
|------|------|----------------|
| `claude_max` | `~/.claude/` OAuth (Claude Max 구독). 기본값. | 없음 |
| `external_anthropic` | 명시적 Anthropic API 키 + 선택적 base URL 오버라이드 | `external_anthropic_api_key_env` (기본: `ANTHROPIC_API_KEY`) |
| `external_proxy` | 운영자 관리 프록시 (LiteLLM 등) | `external_base_url` (필수) |
| `gadgetron_local` | Gadgetron 내부 shim → 로컬 provider. P2C. | P2A에서는 startup error |

- `external_anthropic_api_key_env`: `external_anthropic` 모드에서 Anthropic API 키를 담은 환경변수 이름. 기본값 `"ANTHROPIC_API_KEY"`. 해당 환경변수가 설정되지 않으면 서버 시작 시 오류가 발생합니다.
- `external_base_url`: `external_proxy` 모드에서 ANTHROPIC_BASE_URL 오버라이드. `external_proxy` 모드에서는 필수이며 비어 있으면 startup error입니다. `external_anthropic` 모드에서도 선택적으로 사용할 수 있습니다.
- `local_model`: `gadgetron_local` 모드에서 router provider map의 `<provider_name>/<model_id>`. P2A에서는 동작하지 않으며 startup error입니다.

### `[agent.gadgets]`

Penny가 실행할 수 있는 Gadget(도구) 권한 모델입니다. `[agent.tools]`는 backward-compat alias입니다 (ADR-P2A-10 이전 설정 파일 지원).

각 모드: `auto` (즉시 실행, audit log 기록), `ask` (사용자 승인 카드 대기, 기본값), `never` (항상 거부 + allowed-tools에서 제외).

```toml
[agent.gadgets]
approval_timeout_secs = 60     # 승인 카드 만료 시간 (범위: [10, 600])

[agent.gadgets.write]
default_mode = "ask"           # T2 쓰기 기본 모드
wiki_write = "auto"            # wiki.write / wiki.create / wiki.delete (기본 auto — 단일 사용자 데스크탑 권장)
infra_write = "ask"            # infra 쓰기 도구 (P2C)
scheduler_write = "ask"        # 스케줄러 쓰기 도구 (P3)
provider_mutate = "ask"        # provider 변경 도구 (P2C)

[agent.gadgets.destructive]
enabled = false                # true로 설정해야 T3 도구가 allowed-tools에 노출됨
max_per_hour = 3               # 시간당 최대 승인 카드 수 (전역). enabled=true 시 > 0 필수 (V5)
extra_confirmation = "none"    # "none" | "env" | "file"
# extra_confirmation_token_file = "/run/secrets/gadgetron-destructive-token"
```

`read` 티어(T1)는 항상 `auto`이며 변경할 수 없습니다 (V1).

`[agent.gadgets.destructive]` 세부 필드:

- `enabled`: `false`(기본)이면 T3 도구는 `--allowed-tools`에서 완전히 제외됩니다. `true`로 설정해야 T3 승인 카드가 활성화됩니다. T3 모드는 항상 `ask`이며 변경할 수 없습니다.
- `max_per_hour`: 시간당 T3 승인 카드 발행 상한 (전역). `enabled = true`일 때 0이면 startup error (V5).
- `extra_confirmation`: 승인 카드 외 추가 확인 레이어. `"none"` (기본, 승인 카드만), `"env"` (`GADGETRON_DESTRUCTIVE_TOKEN` 환경변수와 일치 필요), `"file"` (`extra_confirmation_token_file` 파일 내용과 일치 필요).
- `extra_confirmation_token_file`: `extra_confirmation = "file"` 시 토큰 파일 경로. 파일은 존재해야 하며 퍼미션 0400 또는 0600이어야 합니다 (V6).

### `[agent.shared_context]`

매 요청마다 Penny 컨텍스트에 주입되는 bootstrap 다이제스트를 제어합니다.

```toml
[agent.shared_context]
enabled = true                          # false = bootstrap 주입 비활성화 (비상 롤백용)
bootstrap_activity_limit = 6            # 최근 activity 항목 수 (범위: [1, 20])
bootstrap_candidate_limit = 4           # 보류 중인 지식 후보 수 (범위: [1, 12])
bootstrap_approval_limit = 3            # 보류 중인 승인 요청 수 (범위: [0, 10])
digest_summary_chars = 240              # 요약/타이틀 최대 코드포인트 수 (범위: [80, 512])
require_explicit_degraded_notice = true # MUST remain true — false로 설정하면 startup error
```

- `enabled`: `false`로 설정하면 bootstrap 주입 전체를 비활성화합니다. 비상 롤백 전용 스위치입니다. `require_explicit_degraded_notice`와 구별됩니다.
- `require_explicit_degraded_notice`: MUST remain `true`. `false`로 설정하면 서버 시작 오류가 발생합니다.

---

### `[knowledge]`

이 섹션이 있어야 `gadgetron serve`가 `penny` 모델을 등록합니다. 현재 `gadgetron init`은 이 블록을 자동으로 생성하지 않습니다.

```toml
[knowledge]
wiki_path = "./.gadgetron/wiki"   # config 파일 위치 기준 상대 경로 OK
wiki_autocommit = true
wiki_git_author = "Gadgetron <penny@gadgetron.local>"  # 생략 시 git 전역 설정 또는 기본값 사용
wiki_max_page_bytes = 1048576
```

- `wiki_path`: 위키 저장소 루트. 부모 디렉터리는 미리 존재해야 합니다.
  절대 경로와 상대 경로 모두 지원합니다 — 상대 경로는 **`gadgetron.toml`
  파일이 있는 디렉터리 기준**으로 해석됩니다 (cwd 기준이 아님). 이 덕분에
  Penny가 `gadgetron gadget serve`를 `~/.gadgetron/penny/work/` cwd로
  spawn 하더라도 상대 경로가 올바르게 풀립니다.
- `wiki_autocommit`: 쓰기마다 자동 git commit 수행 여부
- `wiki_git_author`: git commit 작성자 (`"Name <email>"` 형식). 생략 시 전역 gitconfig에서 자동 감지, 없으면 `"Penny <penny@gadgetron.local>"` 사용.
- `wiki_max_page_bytes`: 페이지 최대 크기 (범위: 1 ~ 100 MiB)

#### 시드 페이지 자동 주입

`[knowledge]` 가 활성화된 상태에서 `wiki_path` 가 비어 있으면 (user-authored `.md` 파일이 하나도 없는 경우) Gadgetron 은 첫 `Wiki::open()` 시점에 내장 시드 페이지를 자동으로 주입합니다. 시드는 `crates/gadgetron-knowledge/seeds/` 에 컴파일-타임 임베드되어 있으며 일반 `wiki.write` 경로를 타므로 동일한 secret scanner · 페이지 크기 제한 · autocommit 이 적용됩니다.

- 주입 조건: 첫 `open()` 호출 시 `list()` 가 비어 있을 때만. 이후에는 skip.
- 로그: `target = "wiki_seed"`, 메시지 `injected N seed pages into fresh wiki`. 실패해도 서버 시작은 중단되지 않습니다 (non-fatal).
- `wiki_autocommit = true` 이면 각 시드 페이지가 개별 commit 으로 히스토리에 기록됩니다.

`Wiki::list()` 는 `.git` 디렉터리만 제외하고 `wiki_path` 전체를 walk 하므로, `_archived/` 아래 있는 `.md` 파일도 "존재하는 페이지" 로 카운트됩니다. 즉:

- **일반 `wiki.delete` 호출로는 시드가 재주입되지 않습니다.** `wiki.delete` 는 soft-delete 로 파일을 `_archived/<YYYY-MM-DD>/` 로 rename 하므로 `list()` 가 여전히 non-empty 입니다 (`wiki_autocommit = true` 이면 rename이 git commit 으로도 기록됩니다).
- **수동으로 `wiki_path` 아래 모든 `.md` 를 제거한 뒤 재시작하면 시드가 다시 주입됩니다** (`_archived/` 포함). 시드를 영구적으로 원치 않으면 placeholder `.md` 파일을 하나 남겨두거나, `wiki_path` 자체를 초기화하지 마십시오.

### `[knowledge.search]`

```toml
[knowledge.search]
searxng_url = "http://127.0.0.1:8888"
timeout_secs = 10
max_results = 10
```

이 블록이 없으면 `web.search` MCP 도구는 노출되지 않습니다.

### `[knowledge.embedding]` (Semantic search setup)

pgvector 기반 시맨틱 검색을 활성화합니다. PostgreSQL + pgvector extension이 필요합니다. 이 섹션이 없으면 keyword-only 검색만 동작합니다.

```toml
[knowledge.embedding]
provider = "openai_compat"          # P2A: openai_compat 고정
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"      # 임베딩 API 키를 담은 환경변수명
model = "text-embedding-3-small"
dimension = 1536                    # 모델의 벡터 차원 (DDL과 일치 필요)
write_mode = "async"                # "async" (기본) 또는 "sync"
timeout_secs = 30                   # 범위: [1, 300]
```

- `provider`: 현재 `"openai_compat"` 만 지원. OpenAI 및 로컬 Ollama 등 OpenAI-compat endpoint 모두 수용.
- `base_url`: 임베딩 요청을 보낼 endpoint root. `{base_url}/embeddings` 에 POST.
- `api_key_env`: API 키 값이 아닌 **환경변수 이름**. 키가 없으면 서버 시작 시 오류.
- `dimension`: DDL `vector(N)` 과 반드시 일치해야 함. 불일치 시 `EmbeddingError::DimensionMismatch` 로 INSERT 차단.
- `write_mode`: 설정 타입은 존재하며 기본값 `"async"`. 현재 trunk에서 write completion semantics 차이는 보장되지 않음 (구현 진행 중).

로컬 임베딩 모델 사용 예 (Ollama):
```toml
[knowledge.embedding]
base_url = "http://localhost:11434/v1"
api_key_env = "OLLAMA_API_KEY"      # Ollama는 임의 값 허용
model = "nomic-embed-text"
dimension = 768
```

### `[knowledge.reindex]`

`gadgetron reindex` CLI와 `gadgetron wiki audit` 명령이 이 필드를 사용합니다. `stale_threshold_days` 는 `[knowledge.embedding]` 없이도 audit에서 독립적으로 사용됩니다.

```toml
[knowledge.reindex]
on_startup = true                   # 서버 시작 시 reindex 실행 여부 (설정 타입 존재; trunk 부팅 루프 wiring 진행 중)
on_startup_mode = "async"           # "async" | "sync" | "incremental" | "full"
stale_threshold_days = 90           # stale 청크 기준 일수 (1–3650)
```

수동 full reindex: `gadgetron reindex --full` (서버 중단 불필요).

`gadgetron wiki audit` 출력 예시 (`stale_threshold_days = 90` 기준):

```
Wiki audit report - 2026-04-19T00:00:00+00:00
Wiki path: /home/user/.gadgetron/wiki
Total pages: 47

## Stale pages (updated more than 90 days ago)
- ops/journal/2025-12-01/incident-summary
  updated: 2025-12-01 (139 days ago)
  suggestion: review for current relevance

## Pages without frontmatter
- ops/tools/2026-01-15/ad-hoc-query
  suggestion: add frontmatter (tags, type, created)
```

문제가 없으면 각 섹션에 `- none`이 출력됩니다.

### `[knowledge.curation]`

지식 후보(Knowledge Candidate) 수집 및 Penny 큐레이션 루프를 제어합니다. 이 섹션을 생략하면 모든 항목이 기본값으로 활성화됩니다. `[knowledge]` 섹션 없이 `enabled = true`를 설정하면 서버 시작 오류가 발생합니다.

```toml
[knowledge.curation]
enabled = true                          # false로 설정하면 후보 생성 루프 비활성화 (audit capture는 유지)
capture_retention_days = 90             # raw activity 이벤트 보존 기간 (최소 7일)
candidate_retention_days = 30           # 후보 행 보존 기간 (capture_retention_days 이하)
max_candidates_per_request = 8          # 요청당 최대 후보 수 (범위 1–32)
auto_prompt_penny = true                # 새 후보 발생 시 Penny에 자동 큐레이션 요청
require_user_confirmation_for = ["org_write", "policy_note", "destructive_action"]

[knowledge.curation.path_rules]
direct_action       = "ops/journal/{date}/{topic}"
gadget_tool_call    = "ops/tools/{date}/{author}"
approval_decision   = "ops/approvals/{date}/{topic}"
runtime_observation = "ops/runtime/{date}/{topic}"
knowledge_writeback = "ops/knowledge/{date}/{topic}"
```

- `enabled`: `false`로 설정 시 후보 생성 루프를 비활성화합니다. 단, activity audit capture는 계속 동작합니다.
- `capture_retention_days`: raw activity 이벤트 보존 기간. 최소 7일 (인시던트 리뷰 보장).
- `candidate_retention_days`: 후보 행 보존 기간. `capture_retention_days` 이하여야 합니다.
- `max_candidates_per_request`: 단일 capture 호출당 최대 후보 수. 범위 [1, 32].
- `auto_prompt_penny`: 새 대기 후보 발생 시 Penny에 자동 큐레이션 프롬프트를 주입할지 여부.
- `require_user_confirmation_for`: Penny가 단독으로 accept할 수 없는 후보 class 목록. 각 항목은 비어 있지 않은 문자열이어야 합니다.
- `path_rules`: 활동 종류별 wiki 경로 템플릿. 키는 `ActivityKind`의 snake_case 값(`direct_action`, `gadget_tool_call`, `approval_decision`, `runtime_observation`, `knowledge_writeback`)이어야 합니다. 매칭되는 규칙이 없으면 `ops/journal/<YYYY-MM-DD>/<candidate_uuid>` fallback 경로가 사용됩니다. `..` 세그먼트가 경로의 어느 위치에 있더라도 허용되지 않습니다.

  템플릿 변수:
  - `{date}` → 이벤트 `created_at`의 UTC `YYYY-MM-DD`
  - `{topic}` → snake_case `ActivityKind` 레이블 (예: `direct_action`)
  - `{author}` → actor UUID (bare, 하이픈 포함)

---

### Minimal working `gadgetron.toml`

The following file is the minimum configuration to serve requests through a single OpenAI provider. It is a **field-minimal example**, not the full canonical local demo path. For the full local path with PostgreSQL, Web UI, and optional Penny, follow [quickstart.md](quickstart.md) and [penny.md](penny.md).

```toml
[server]
bind = "0.0.0.0:8080"

[providers.openai]
type = "openai"
api_key = "${OPENAI_API_KEY}"
models = ["gpt-4o-mini"]
```

Then set environment variables before running:

```sh
export GADGETRON_DATABASE_URL="postgres://gadgetron:password@localhost:5432/gadgetron"
export OPENAI_API_KEY="sk-..."
./target/release/gadgetron
```

---

## Environment variable expansion in gadgetron.toml

The `api_key` field in any provider block supports `${VAR_NAME}` syntax. At load time, `${VAR_NAME}` is replaced with the value of the `VAR_NAME` environment variable. If the variable is not set, the literal string `${VAR_NAME}` is used (which will cause authentication failures at the provider).

Example:

```toml
[providers.openai]
type = "openai"
api_key = "${OPENAI_API_KEY}"  # reads from OPENAI_API_KEY at runtime
models = ["gpt-4o"]
```

Only single-variable expansion is supported. Shell-style expressions (`${VAR:-default}`, command substitution, etc.) are not supported.
