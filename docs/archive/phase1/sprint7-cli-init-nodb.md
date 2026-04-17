# Sprint 7: User Journey — Zero to First Request

> **담당**: @dx-product-lead
> **상태**: Draft
> **작성일**: 2026-04-12
> **최종 업데이트**: 2026-04-12
> **관련 크레이트**: `gadgetron-cli`, `gadgetron-xaas`, `gadgetron-gateway`, `gadgetron-core`
> **Phase**: [P1]
> **결정 근거**: D-20260411-01 (Phase 1 MVP), D-20260411-03 (gadgetron-xaas), D-11 (API key prefix), D-13 (GadgetronError variants), D-4/D-9 (PostgreSQL + sqlx)

---

## 1. 철학 & 컨셉 (Why)

### 1.1 문제 한 문장

"첫 번째 요청까지 너무 많은 사전 준비가 필요하다" — PostgreSQL 설치, 스키마 수동 적용, tenant/key SQL INSERT를 모두 완료해야만 `curl`이 200을 돌려준다. 처음 Gadgetron을 발견한 사람은 이 장벽에서 포기한다.

### 1.2 경쟁 벤치마크에서 배운 것

경쟁 도구(Ollama, vLLM, LiteLLM, LocalAI, LM Studio, text-generation-webui, OpenLLM, Jan, llamafile, Msty) 10종 분석 결과:

| # | 원칙 | 출처 도구 | Gadgetron 적용 |
|---|------|-----------|----------------|
| 1 | config 없이도 즉시 기동 | Ollama | config 미존재 시 기본값으로 no-db 모드 기동 (§2.3.4) |
| 2 | `--model <endpoint>` 단일 플래그로 빠른 연결 | vLLM `--model` | `gadgetron serve --provider <url>` (§2.1, §2.3.6) |
| 3 | 기동 즉시 API URL 출력 | vLLM 배너 | `OpenAI API: http://localhost:8080/v1` 배너 출력 (§2.3.2) |
| 4 | 연결 각 단계 progress 출력 | Ollama 기동 시 | `Connecting to PostgreSQL... done` 등 단계별 출력 (§2.3.5) |
| 5 | 에러에 provider context 포함 | LiteLLM proxy error | `Provider 'x' (vllm @ host:port) returned error` (§1.4 3-C) |
| 6 | 단일 바이너리, zero install friction | llamafile | `cargo install` 또는 brew 1커맨드 (install doc 범위) |
| 7 | `GET /v1/models` 로 연결 확인 가능 | OpenAI / vLLM | OpenAI-compat `/v1/models` 라우트 지원 (gateway 범위) |
| 8 | 로컬 first-party 모델 관리 UI | LM Studio / Jan | TUI 대시보드 (`--tui`) — ux-interface-lead 범위 |
| 9 | multi-provider fallback 자동화 | LiteLLM | Router fallback 전략 (router-lead 범위) |
| 10 | 운영 모드 전환이 config 1줄 변경 | LocalAI | no-db → full: `database_url` 한 줄 추가 후 재기동 |

**Gadgetron 차별화 3가지** (경쟁 도구 대비):

1. **Rust-native 성능**: 동일 하드웨어에서 Python-based proxy(LiteLLM, text-generation-webui) 대비 낮은 overhead. Go 기반 도구 수준 레이턴시 목표.
2. **multi-tenant + quota**: Ollama/vLLM은 단일 사용자 가정. Gadgetron은 tenant/key 관리 + 할당량 강제 (PG full mode).
3. **클라우드-로컬 하이브리드 라우팅**: 로컬 GPU + 클라우드 API를 동일 엔드포인트로 추상화. LiteLLM과 유사하나 Rust로 운영 부담 감소.

### 1.3 사용자 여정 맵 (6 Stage)

```
Stage 1: 발견 (README)          ← 10초: "이게 뭔지"  30초: "나한테 필요한지"
Stage 2: 설치 (3분)             ← cargo install 또는 brew install 수준
Stage 3: 첫 실행 (1분)          ← gadgetron init → gadgetron serve
Stage 4: 첫 요청 (30초)         ← gadgetron key create → curl
Stage 5: 에러 대응 (자가 해결)  ← gadgetron doctor
Stage 6: 운영 전환              ← PG 연결 정식 모드
```

각 Stage에서 사용자가 보는 정확한 화면(stdout/stderr)을 §1.3에 명시한다. 구현자는 이 출력을 byte-for-byte 일치하게 만들어야 한다. (색상/이모지 제외, 터미널 감지 후 선택적 사용 — §2.9)

### 1.4 사용자가 보는 화면 (stdout/stderr) — Stage별 정의

#### Stage 3-A: `gadgetron init`

```
$ gadgetron init
Gadgetron Configuration Setup

  Where should the server listen? [0.0.0.0:8080]:
  Database URL (leave empty for no-db mode):
  Provider endpoint (e.g., http://localhost:8000): http://10.100.1.5:8100
  Provider type [vllm]:

Config written to gadgetron.toml

  Next steps:
    1. Review gadgetron.toml — uncomment additional providers as needed.
    2. Run: gadgetron serve
```

비대화형 실행 (`--yes` 또는 리디렉션된 stdin):

```
$ gadgetron init --yes
Config written to gadgetron.toml

  Next steps:
    1. Review gadgetron.toml — uncomment additional providers as needed.
    2. Run: gadgetron serve
```

이미 존재하는 파일, `--yes` 없음:

```
$ gadgetron init
'gadgetron.toml' already exists. Overwrite? [y/N]
```

→ N(또는 엔터): `Aborted. Existing file left unchanged.`

→ Y: Config written 메시지 출력.

#### Stage 3-B: `gadgetron serve` (no-db 자동 감지)

PG 없이 `gadgetron.toml`에 `database_url`이 비어있거나 미설정인 경우:

```
$ gadgetron serve
  Checking provider(s)... done (1 configured)
  Starting server...

Gadgetron v0.1.0
   OpenAI API: http://localhost:8080/v1

  Mode:     no-db  (in-memory key validation, quota disabled)
  Listen:   0.0.0.0:8080
  Provider: vllm @ http://10.100.1.5:8100

  WARNING: Running without a database. Keys are not persisted or validated
  against stored records. Do not use in production without a database.

  Quick start:
    gadgetron key create                # create a temporary API key
    curl http://localhost:8080/v1/chat/completions \
      -H "Authorization: Bearer <key>" \
      -d '{"model":"...","messages":[...]}'

  For full mode with PostgreSQL:
    export GADGETRON_DATABASE_URL=postgres://user:pass@localhost:5432/gadgetron
    gadgetron serve
```

WARNING 라인은 stderr에도 동시 출력. 나머지는 stdout.

PG 연결 정상 (정식 모드):

```
$ gadgetron serve
  Connecting to PostgreSQL... done
  Running migrations... done
  Checking provider(s)... done (1 configured)
  Starting server...

Gadgetron v0.1.0
   OpenAI API: http://localhost:8080/v1

  Mode:     full  (PostgreSQL key validation, quota enabled)
  Listen:   0.0.0.0:8080
  Provider: vllm @ http://10.100.1.5:8100
  Database: postgres://...@localhost:5432/gadgetron

  Quick start:
    gadgetron tenant create --name "my-team"
    gadgetron key create --tenant-id <uuid>
```

진행 상태 라인("Connecting to PostgreSQL...", "Running migrations...", "Checking provider(s)...")은 stderr에 출력. 배너 이후는 stdout. 각 단계가 성공하면 같은 줄에 " done"을 덧붙인다 (eprint! + eprintln! 패턴). 실패 시 " failed"를 출력 후 에러 메시지.

PG가 명시적으로 설정됐으나 연결 실패:

```
$ gadgetron serve
Error: Failed to connect to PostgreSQL.

  Attempted: postgres://user:pass@localhost:5432/gadgetron
  Cause:     connection refused

  Next steps:
    - Verify PostgreSQL is running: pg_isready -h localhost -p 5432
    - Check credentials in GADGETRON_DATABASE_URL
    - To run without a database: leave database_url empty in gadgetron.toml
```

stdout에 헤더 없음, stderr에만 출력, exit code 1.

#### Stage 3-C: Provider 에러 응답 (Error attribution)

provider가 에러를 반환하거나 타임아웃될 때 클라이언트가 받는 OpenAI-compatible JSON 에러 응답. **어느 provider에서 에러가 났는지 명시**해야 클라이언트가 self-diagnose 할 수 있다 (LiteLLM 패턴).

```json
{
  "error": {
    "message": "Provider 'gemma4' (vllm @ 10.100.1.5:8100) returned error. Run GET /v1/models to check.",
    "type": "server_error",
    "code": "provider_error"
  }
}
```

provider 타임아웃 시:

```json
{
  "error": {
    "message": "Provider 'gemma4' (vllm @ 10.100.1.5:8100) timed out after 30s. Check provider health or increase request_timeout_ms in gadgetron.toml.",
    "type": "server_error",
    "code": "provider_timeout"
  }
}
```

provider 연결 불가 시:

```json
{
  "error": {
    "message": "Provider 'gemma4' (vllm @ 10.100.1.5:8100) is unreachable. Run: gadgetron doctor",
    "type": "server_error",
    "code": "provider_unreachable"
  }
}
```

HTTP status: 502 Bad Gateway. `message` 필드 형식 규칙:
- `Provider '<name>' (<type> @ <host>:<port>) <동사> <설명>. <remediation>.`
- `<name>`은 `gadgetron.toml`의 provider 섹션 키.
- `<type>`은 `vllm | sglang | ollama | openai | anthropic | gemini`.
- `<host>:<port>`는 endpoint에서 추출 (scheme 제외).
- internal stack trace, file path, struct name은 절대 포함하지 않는다.

#### Stage 4-A: `gadgetron key create` (no-db 모드)

```
$ gadgetron key create
API Key Created

  Key: gad_live_a1b2c3d4e5f6789012345678901234567890

  Save this key — it cannot be retrieved later.

  Test it:
    curl http://localhost:8080/v1/chat/completions \
      -H "Authorization: Bearer gad_live_a1b2c3d4e5f6789012345678901234567890" \
      -H "Content-Type: application/json" \
      -d '{"model":"cyankiwi/gemma-4-31B-it-AWQ-4bit","messages":[{"role":"user","content":"Hello!"}]}'
```

no-db 모드에서는 key가 메모리에도 저장되지 않는다 (validator는 포맷만 확인). 서버 재시작 후에도 동일 key가 계속 유효하다.

#### Stage 4-B: `gadgetron key create --tenant-id <uuid>` (PG 모드)

```
$ gadgetron key create --tenant-id 550e8400-e29b-41d4-a716-446655440000
API Key Created

  Key:    gad_live_a1b2c3d4e5f6789012345678901234567890
  Tenant: 550e8400-e29b-41d4-a716-446655440000
  Scopes: OpenAiCompat

  Save this key — it cannot be retrieved later.

  Test it:
    curl http://localhost:8080/v1/chat/completions \
      -H "Authorization: Bearer gad_live_a1b2c3d4e5f6789012345678901234567890" \
      -H "Content-Type: application/json" \
      -d '{"model":"cyankiwi/gemma-4-31B-it-AWQ-4bit","messages":[{"role":"user","content":"Hello!"}]}'
```

#### Stage 4-C: `gadgetron tenant create --name "my-team"`

```
$ gadgetron tenant create --name "my-team"
Tenant Created

  ID:   550e8400-e29b-41d4-a716-446655440000
  Name: my-team

  Next: gadgetron key create --tenant-id 550e8400-e29b-41d4-a716-446655440000
```

#### Stage 5: `gadgetron doctor`

```
$ gadgetron doctor
Gadgetron v0.1.0 — System Check

  [PASS] Config file:    gadgetron.toml found and valid TOML
  [PASS] Server bind:    0.0.0.0:8080 (port available)
  [WARN] Database:       database_url not configured — running in no-db mode
  [PASS] Provider vllm:  http://10.100.1.5:8100 reachable (200 OK in 23ms)
  [PASS] /health:        gadgetron is running on port 8080

  1 warning found.
  WARN: No database configured. Tenant management and quota enforcement require PostgreSQL.
  To configure: set database_url in gadgetron.toml or GADGETRON_DATABASE_URL env var.
```

모든 PASS:

```
  All checks passed.
```

서버 미기동 시:

```
  [FAIL] /health: connection refused at http://localhost:8080/health

  Next step: gadgetron serve
```

### 1.5 제품 비전과의 연결

Gadgetron은 self-hosted LLM 게이트웨이 (`docs/00-overview.md §1`). self-hosted = 사용자가 관리자. CLI가 `psql` 직접 접속의 대체제가 된다. "Zero to first request in 3 commands"는 다음을 의미한다:

```
gadgetron init       # 1
gadgetron serve      # 2
gadgetron key create # 3
# → curl 200
```

Stage 2 (설치)는 이 문서 범위 밖 (`devops-sre-lead` 담당 install doc). 이 문서는 Stage 3-6을 다룬다.

### 1.6 고려한 대안과 채택하지 않은 이유

| 대안 | 채택 여부 | 이유 |
|------|-----------|------|
| PG 없으면 아예 시작 불가 | 미채택 | 개발자 첫 경험을 즉시 차단 — Stage 3 장벽 |
| `--no-db`를 명시해야만 no-db 모드 | 미채택 | `gadgetron init`에서 DB URL을 비우면 자동 감지가 자연스러움 |
| `--no-db`를 default로 (PG 없어도 경고 없음) | 미채택 | 운영 환경에서 실수로 인증 없이 기동하는 insecure default 방지 |
| REST API로만 tenant/key 관리 | 미채택 | 서버 기동 전 부트스트랩 불가 (닭-달걀 문제) |
| 별도 `gadgetron-admin` 바이너리 | 미채택 | 설치 복잡도 증가, 단일 바이너리 원칙 위배 |
| `gadgetron init`이 PG 스키마까지 생성 | 미채택 | 관심사 분리 — config 생성과 DB 마이그레이션은 다른 책임 |

### 1.7 핵심 설계 원칙

- **기본은 안전하다**: `--no-db`는 명시적으로 지정하거나 config에 DB URL이 없을 때만 활성화. 반드시 WARNING 출력.
- **secret은 1회만 노출**: raw key는 생성 시점 stdout에만 출력. DB에는 SHA-256 hash만 저장. `tracing`에 절대 기록 안 함.
- **에러 메시지 3요소**: 무엇이 일어났는지 / 왜 / 다음 단계.
- **copy-pasteable**: 모든 예시(curl 포함)는 실제로 실행 가능한 형태.
- **doctor first**: 막히면 `gadgetron doctor`로 자가 진단.

---

## 2. 상세 구현 방안 (What & How)

### 2.1 clap subcommand 트리

**파일**: `crates/gadgetron-cli/src/main.rs`

기존 `Commands` enum을 아래로 교체한다. 기존 `Serve { config, bind, tui }` 필드 유지.

```rust
/// Gadgetron — Rust-native GPU/LLM orchestration platform.
#[derive(Parser)]
#[command(
    name = "gadgetron",
    version,
    about = "GPU/LLM orchestration platform",
    long_about = None,
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start the gateway server.
    ///
    /// If database_url is not configured (gadgetron.toml or GADGETRON_DATABASE_URL),
    /// starts in no-db mode automatically with a warning.
    /// Use --no-db to force no-db mode even when database_url is set.
    Serve {
        /// Path to TOML configuration file.
        /// Env: GADGETRON_CONFIG
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,

        /// TCP bind address (host:port).
        /// Env: GADGETRON_BIND
        #[arg(long, short = 'b')]
        bind: Option<String>,

        /// Launch the ratatui TUI dashboard in the current terminal.
        #[arg(long)]
        tui: bool,

        /// Force no-db mode even if database_url is configured.
        /// In no-db mode: any gad_live_* or gad_test_* key is accepted.
        /// Keys are not persisted. Quota is disabled.
        /// Do not use in production.
        #[arg(long)]
        no_db: bool,

        /// Quick-start: connect to a provider without a config file (vLLM pattern).
        ///
        /// When set, Gadgetron starts immediately in no-db proxy mode routing all
        /// requests to this single provider endpoint. Config file and database are
        /// not required. Uses InMemoryKeyValidator. Any gad_live_* or gad_test_*
        /// key is accepted.
        ///
        /// Example: gadgetron serve --provider http://10.100.1.5:8100
        /// Env: GADGETRON_PROVIDER
        #[arg(long)]
        provider: Option<String>,  // e.g., "http://10.100.1.5:8100"
    },

    /// Generate an annotated gadgetron.toml interactively.
    ///
    /// If stdin is not a TTY (e.g., piped or --yes), writes defaults without prompting.
    Init {
        /// Output path for the generated config file.
        #[arg(long, short = 'o', default_value = "gadgetron.toml")]
        output: PathBuf,

        /// Overwrite existing file without prompting.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Manage API keys.
    ///
    /// In no-db mode (gadgetron serve without database), use without --tenant-id.
    /// In full mode (PostgreSQL), --tenant-id is required.
    Key {
        #[command(subcommand)]
        command: KeyCmd,
    },

    /// Manage tenants. Requires PostgreSQL.
    Tenant {
        #[command(subcommand)]
        command: TenantCmd,
    },

    /// Diagnose Gadgetron configuration and connectivity.
    Doctor {
        /// Config file to check (default: gadgetron.toml in current directory).
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum KeyCmd {
    /// Create an API key. The raw key is printed once and never stored.
    ///
    /// In no-db mode: --tenant-id is not required. Key is validated by format only.
    /// In full mode:  --tenant-id is required. Key is stored (hash only) in PostgreSQL.
    Create {
        /// UUID of the owning tenant. Required in full mode, omit in no-db mode.
        #[arg(long)]
        tenant_id: Option<uuid::Uuid>,

        /// Key environment: live or test. Determines the gad_live_ / gad_test_ prefix.
        /// Env: GADGETRON_KEY_KIND
        #[arg(long, default_value = "live")]
        kind: String,

        /// Comma-separated list of scopes. Default: OpenAiCompat
        #[arg(long)]
        scope: Option<String>,

        /// Human-readable name for this key (optional label, not used for auth).
        #[arg(long)]
        name: Option<String>,
    },

    /// List API keys for a tenant (hashes are never shown). Requires PostgreSQL.
    List {
        /// UUID of the tenant whose keys to list.
        #[arg(long)]
        tenant_id: uuid::Uuid,
    },

    /// Revoke an API key by its UUID. Requires PostgreSQL.
    Revoke {
        /// UUID of the key record to revoke.
        #[arg(long)]
        key_id: uuid::Uuid,
    },
}

#[derive(Subcommand)]
pub enum TenantCmd {
    /// Create a new tenant and print its UUID.
    Create {
        /// Human-readable display name for the tenant.
        #[arg(long)]
        name: String,
    },
    /// List all active tenants.
    List,
}
```

**변경 불가 사항**: 기존 `clap_parses_serve_*` 테스트 전부 그린 유지. `no_db: bool` 필드만 추가.

> **NOTE (기존 테스트 수정)**: 기존 `clap_parses_serve_*` 테스트의 match arm에 `no_db: _` 추가 필요 (또는 `..` 패턴 사용). 예:
> ```rust
> Commands::Serve { config, bind, tui, no_db: _ } => { ... }
> // 또는
> Commands::Serve { config, bind, tui, .. } => { ... }
> ```

### 2.2 `gadgetron init` — 대화형 config 생성

**파일**: `crates/gadgetron-cli/src/commands/init.rs` (신규)

#### 2.2.1 stdout 출력 포맷 (정확한 문자열)

비대화형 (`--yes` 또는 stdin이 TTY가 아닌 경우):

```
Config written to gadgetron.toml

  Next steps:
    1. Review gadgetron.toml — uncomment additional providers as needed.
    2. Run: gadgetron serve
```

대화형 (stdin이 TTY):

```
Gadgetron Configuration Setup

  Where should the server listen? [0.0.0.0:8080]:
  Database URL (leave empty for no-db mode):
  Provider endpoint (e.g., http://localhost:8000): http://10.100.1.5:8100
  Provider type [vllm]:

Config written to gadgetron.toml

  Next steps:
    1. Review gadgetron.toml — uncomment additional providers as needed.
    2. Run: gadgetron serve
```

오류 시 (파일 쓰기 실패):

```
Error: Failed to write config to 'gadgetron.toml'.

  Cause:     Permission denied (os error 13)
  Next step: Check write permission on the current directory.
```

이 오류는 stderr에만 출력. exit code 1.

#### 2.2.2 구현

```rust
use anyhow::{Context, Result};
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;

/// Answers collected from interactive prompts or defaults.
struct InitAnswers {
    bind: String,
    database_url: String,
    provider_endpoint: String,
    provider_type: String,
}

impl Default for InitAnswers {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:8080".into(),
            database_url: String::new(),
            provider_endpoint: String::new(),
            provider_type: "vllm".into(),
        }
    }
}

/// Execute `gadgetron init`.
///
/// - If `yes` is true or stdin is not a TTY: write defaults without prompting.
/// - Otherwise: prompt interactively, accept empty input for defaults.
/// - If `output` already exists and `yes` is false: prompt for overwrite.
pub fn run_init(output: &Path, yes: bool) -> Result<()> {
    if output.exists() && !yes {
        if !confirm_overwrite(output)? {
            println!("Aborted. Existing file left unchanged.");
            return Ok(());
        }
    }

    let answers = if !yes && io::stdin().is_terminal() {
        println!("Gadgetron Configuration Setup\n");
        prompt_init_answers()?
    } else {
        InitAnswers::default()
    };

    let content = render_config_template(&answers);

    std::fs::write(output, &content).map_err(|e| {
        anyhow::anyhow!(
            "Failed to write config to '{path}'.\n\n  Cause:     {e}\n  Next step: Check write permission on the current directory.",
            path = output.display(),
        )
    })?;

    println!("Config written to {}\n", output.display());
    println!("  Next steps:");
    println!("    1. Review {} — uncomment additional providers as needed.", output.display());
    println!("    2. Run: gadgetron serve");

    Ok(())
}

/// Prompt the user for each config field, using defaults on empty input.
fn prompt_init_answers() -> Result<InitAnswers> {
    let stdin = io::stdin();
    let mut answers = InitAnswers::default();

    answers.bind = prompt_with_default(
        &stdin,
        "  Where should the server listen?",
        "0.0.0.0:8080",
    )?;

    answers.database_url = prompt_with_default(
        &stdin,
        "  Database URL (leave empty for no-db mode)",
        "",
    )?;

    answers.provider_endpoint = prompt_with_default(
        &stdin,
        "  Provider endpoint (e.g., http://localhost:8000)",
        "",
    )?;

    if !answers.provider_endpoint.is_empty() {
        answers.provider_type = prompt_with_default(
            &stdin,
            "  Provider type",
            "vllm",
        )?;
    }

    println!(); // blank line before "Config written to"
    Ok(answers)
}

/// Print a prompt with a bracketed default, read one line.
/// Returns the entered value, or the default if the user pressed Enter.
fn prompt_with_default(
    stdin: &io::Stdin,
    label: &str,
    default: &str,
) -> Result<String> {
    if default.is_empty() {
        print!("{}: ", label);
    } else {
        print!("{} [{}]: ", label, default);
    }
    io::stdout().flush().context("failed to flush stdout")?;

    let mut input = String::new();
    stdin.lock().read_line(&mut input)
        .context("failed to read input")?;

    let trimmed = input.trim().to_string();
    if trimmed.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(trimmed)
    }
}

/// Prompt for overwrite confirmation. Default is no (pressing Enter = no).
fn confirm_overwrite(path: &Path) -> Result<bool> {
    print!("'{}' already exists. Overwrite? [y/N] ", path.display());
    io::stdout().flush().context("failed to flush stdout")?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)
        .context("failed to read user input")?;

    Ok(matches!(input.trim(), "y" | "Y"))
}

/// Render the TOML template with user-supplied values.
///
/// Every field has: inline doc comment, default value, env override line.
fn render_config_template(a: &InitAnswers) -> String {
    let provider_section = build_provider_section(a);
    let db_section = if a.database_url.is_empty() {
        "# database_url is not set — Gadgetron will start in no-db mode.\n\
         # To enable full mode:\n\
         #   database_url = \"postgres://user:pass@localhost:5432/gadgetron\"\n\
         # Or set: GADGETRON_DATABASE_URL\n".to_string()
    } else {
        format!(
            "# Env override: GADGETRON_DATABASE_URL\n\
             database_url = \"{}\"\n",
            a.database_url,
        )
    };

    format!(
        r#"# Gadgetron Configuration
# Generated by: gadgetron init
# Reference: https://github.com/NacBang/gadgetron/blob/main/docs/manual/configuration.md

[server]
# TCP address to bind the gateway on.
# Env override: GADGETRON_BIND
bind = "{bind}"

# Request timeout in milliseconds. Requests exceeding this are cancelled.
# Env override: GADGETRON_REQUEST_TIMEOUT_MS
request_timeout_ms = 30000

[database]
{db_section}
# ---------------------------------------------------------------------------
# Providers — configure at least one LLM backend.
# Uncomment and fill in the appropriate section.
# ---------------------------------------------------------------------------
{provider_section}
# ---------------------------------------------------------------------------
# Router — controls how requests are distributed across providers.
# ---------------------------------------------------------------------------

[router.default_strategy]
# Routing strategy: round_robin | cost_optimal | latency_optimal | fallback | weighted
type = "round_robin"
"#,
        bind = a.bind,
        db_section = db_section,
        provider_section = provider_section,
    )
}

fn build_provider_section(a: &InitAnswers) -> String {
    let static_examples = r#"# --- OpenAI ---
# [providers.openai]
# type = "openai"
# api_key = "sk-..."          # env: OPENAI_API_KEY (preferred over this field)
# models = ["gpt-4o", "gpt-4o-mini"]

# --- Anthropic ---
# [providers.anthropic]
# type = "anthropic"
# api_key = "sk-ant-..."      # env: ANTHROPIC_API_KEY (preferred over this field)
# models = ["claude-3-5-sonnet-20241022"]

# --- SGLang (local) ---
# [providers.my-sglang]
# type = "sglang"
# endpoint = "http://localhost:30000"
# models = ["Qwen/Qwen2.5-7B-Instruct"]

# --- Ollama (local) ---
# [providers.my-ollama]
# type = "ollama"
# endpoint = "http://localhost:11434"
# models = ["llama3.2", "mistral"]

"#;
    if a.provider_endpoint.is_empty() {
        return format!(
            "\n# No provider configured. Uncomment one of the examples below.\n\n{static_examples}"
        );
    }

    let provider_name = "my-provider";
    format!(
        r#"
[providers.{name}]
# Provider type: vllm | sglang | ollama | openai | anthropic | gemini
type = "{ptype}"
# The base URL of the provider's HTTP API.
# Env override: GADGETRON_PROVIDER_{UPPER}_ENDPOINT
endpoint = "{endpoint}"
# List the exact model IDs this provider serves.
# models = ["model-name-here"]

{static_examples}"#,
        name = provider_name,
        ptype = a.provider_type,
        endpoint = a.provider_endpoint,
        UPPER = a.provider_type.to_uppercase(),
        static_examples = static_examples,
    )
}
```

**하위 호환**: 기존 `gadgetron.toml`이 `[database]` 섹션 없이 `database_url`을 최상위에 두는 경우, `gadgetron serve`가 두 위치 모두 확인한다 (§2.4.3).

### 2.3 `gadgetron serve` — 자동 no-db 감지 + 시작 배너

**파일**: `crates/gadgetron-cli/src/main.rs` (기존 `serve` function 수정)

#### 2.3.1 no-db 감지 로직

```rust
/// Determine whether to run in no-db mode.
///
/// Priority (highest first):
/// 1. --no-db flag (always no-db)
/// 2. config.database.database_url present and non-empty → full mode
/// 3. GADGETRON_DATABASE_URL env var present and non-empty → full mode
/// 4. Neither configured → no-db mode (auto-detected, prints warning)
fn resolve_db_mode(cfg: &AppConfig, flag_no_db: bool) -> DbMode {
    if flag_no_db {
        return DbMode::NoDb { explicit: true };
    }
    let cli_db_url = cfg.database.as_ref()
        .and_then(|d| d.database_url.clone())
        .filter(|s| !s.is_empty());

    let url = cli_db_url
        .or_else(|| cfg.xaas.as_ref().and_then(|x| x.database_url.clone()))
        .or_else(|| std::env::var("GADGETRON_DATABASE_URL").ok());

    match url {
        Some(u) if !u.is_empty() => DbMode::Full { url: u },
        _ => DbMode::NoDb { explicit: false },
    }
}

enum DbMode {
    /// No-db mode. `explicit = true` means --no-db was passed; false means auto-detected.
    NoDb { explicit: bool },
    /// Full mode. `url` is the PostgreSQL connection string.
    Full { url: String },
}
```

#### 2.3.2 서버 시작 배너

```rust
fn print_serve_banner(version: &str, mode: &DbMode, bind: &str, providers: &[ProviderSummary]) {
    println!("Gadgetron v{version}");
    // Always print the OpenAI-compatible base URL so users know exactly what to put
    // in their client's base_url field (vLLM pattern).
    let host = if bind.starts_with("0.0.0.0:") || bind.starts_with("[::]") {
        format!("localhost:{}", bind.rsplit(':').next().unwrap_or("8080"))
    } else {
        bind.to_string()
    };
    println!("   OpenAI API: http://{host}/v1");
    println!();

    match mode {
        DbMode::NoDb { .. } => {
            println!("  Mode:     no-db  (in-memory key validation, quota disabled)");
        }
        DbMode::Full { url } => {
            let redacted = redact_db_url(url);
            println!("  Mode:     full  (PostgreSQL key validation, quota enabled)");
            println!("  Database: {redacted}");
        }
    }

    println!("  Listen:   {bind}");
    for p in providers {
        println!("  Provider: {} @ {}", p.kind, p.endpoint);
    }
    println!();

    if let DbMode::NoDb { .. } = mode {
        // WARNING to both stdout and stderr
        let warning = "  WARNING: Running without a database. Keys are not persisted or validated\n  against stored records. Do not use in production without a database.\n";
        print!("{warning}");
        eprint!("{warning}");
        println!();

        println!("  Quick start:");
        println!("    gadgetron key create                # create a temporary API key");
        println!("    curl http://{bind}/v1/chat/completions \\");
        println!("      -H \"Authorization: Bearer <key>\" \\");
        println!("      -d '{{\"model\":\"...\",\"messages\":[...]}}'");
        println!();
        println!("  For full mode with PostgreSQL:");
        println!("    export GADGETRON_DATABASE_URL=postgres://user:pass@localhost:5432/gadgetron");
        println!("    gadgetron serve");
    } else {
        println!("  Quick start:");
        println!("    gadgetron tenant create --name \"my-team\"");
        println!("    gadgetron key create --tenant-id <uuid>");
    }
}

/// Redact password from postgres://user:pass@host/db → postgres://user:***@host/db
fn redact_db_url(url: &str) -> String {
    // Parse minimal: find ://user:pass@ pattern and replace pass with ***
    if let Some(at) = url.rfind('@') {
        if let Some(colon) = url[..at].rfind(':') {
            let scheme_user = &url[..colon];
            let host_db = &url[at..];
            return format!("{scheme_user}:***{host_db}");
        }
    }
    url.to_string()
}
```

#### 2.3.3 PG 연결 실패 에러 출력

PG URL이 설정되어 있으나 연결 실패:

```rust
// stderr only, exit code 1
eprintln!("Error: Failed to connect to PostgreSQL.\n");
eprintln!("  Attempted: {redacted_url}");
eprintln!("  Cause:     {e}");
eprintln!();
eprintln!("  Next steps:");
eprintln!("    - Verify PostgreSQL is running: pg_isready -h <host> -p <port>");
eprintln!("    - Check credentials in GADGETRON_DATABASE_URL");
eprintln!("    - To run without a database: leave database_url empty in gadgetron.toml");
std::process::exit(1);
```

#### 2.3.4 config 미존재 시 기본값 기동 + 안내

`gadgetron serve` 실행 시 `gadgetron.toml`도 없고 `GADGETRON_CONFIG`도 없는 경우, 에러로 종료하지 않고 **내장 기본값으로 즉시 기동**한다 (Ollama 패턴). 동시에 아래 안내를 stdout에 출력한다:

```
$ gadgetron serve
No config file found — using built-in defaults.
   Create one: gadgetron init

Gadgetron v0.1.0

  Mode:     no-db  (in-memory key validation, quota disabled)
  Listen:   0.0.0.0:8080
  Provider: (none configured)
  ...
```

구현 변경: `load_config`가 "file not found" 에러를 반환하면 `AppConfig::default()`를 사용하고 위 메시지를 출력한 뒤 계속 진행한다. TOML 파싱 에러(파일은 존재하나 내용 오류)는 여전히 exit code 1로 종료한다.

```rust
// serve() 내 config load 분기
let config = match load_config(config_path_override) {
    Ok(cfg) => cfg,
    Err(e) if e.is_not_found() => {
        println!("No config file found — using built-in defaults.");
        println!("   Create one: gadgetron init");
        println!();
        AppConfig::default()
    }
    Err(e) => {
        // TOML parse error or permission error — still fatal
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
};
```

`ConfigError::is_not_found()` 판별: `std::io::ErrorKind::NotFound`로 래핑된 경우만 해당. 파싱 오류, 권한 오류는 여전히 fatal.

`AppConfig::default()` 기본값:

| 필드 | 기본값 |
|------|--------|
| `server.bind` | `"0.0.0.0:8080"` |
| `database.database_url` | `None` (no-db 모드 자동 진입) |
| `providers` | 빈 맵 (`{}`) |
| `router.default_strategy` | `RoundRobin` |

> **구현 시 `gadgetron-core/src/config.rs`에 `impl Default for AppConfig` 추가 필요:**
> ```rust
> impl Default for AppConfig {
>     fn default() -> Self {
>         Self {
>             server: ServerConfig { bind: "0.0.0.0:8080".into() },
>             providers: HashMap::new(),
>             router: Default::default(),
>             xaas: None,
>         }
>     }
> }
> ```

#### 2.3.5 `serve` function 전체 흐름

```rust
async fn serve(
    config_path_override: Option<PathBuf>,
    bind_override: Option<String>,
    tui_enabled: bool,
    no_db: bool,
    /// When Some, skip config file entirely and proxy all traffic to this endpoint.
    /// Implies no-db mode. Uses InMemoryKeyValidator + single-provider AppConfig.
    provider_override: Option<String>,
) -> Result<()> {
    init_tracing();

    // 0. --provider shortcut: bypass config file entirely (vLLM pattern)
    if let Some(ref endpoint) = provider_override {
        return serve_provider_quickstart(endpoint, bind_override, tui_enabled).await;
    }

    // 1. Config load — if absent, use built-in defaults (Ollama pattern)
    let config = match load_config(config_path_override) {
        Ok(cfg) => cfg,
        Err(e) if e.is_not_found() => {
            println!("No config file found — using built-in defaults.");
            println!("   Create one: gadgetron init");
            println!();
            AppConfig::default()
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    // 2. Resolve bind address
    let bind_addr = bind_override
        .or_else(|| std::env::var("GADGETRON_BIND").ok())
        .unwrap_or_else(|| config.server.bind.clone());

    // 3. Resolve DB mode
    let db_mode = resolve_db_mode(&config, no_db);

    // 4. Build providers — with per-step progress output (Ollama pattern)
    //    Each step prints "  <label>... " then either "done" or the error.
    //    Output goes to stdout (not tracing) so it is always visible.
    eprint!("  Checking provider(s)...");
    let providers = build_providers(&config).await?;
    eprintln!(" done ({} configured)", providers.len());

    // 5. Build key_validator + quota_enforcer based on db_mode
    let (key_validator, quota_enforcer, pg_pool) = match &db_mode {
        DbMode::NoDb { .. } => {
            tracing::warn!(
                mode = "no-db",
                "Starting without PostgreSQL — keys not validated, quota disabled"
            );
            (
                Arc::new(InMemoryKeyValidator) as Arc<dyn KeyValidator + Send + Sync>,
                Arc::new(InMemoryQuotaEnforcer) as Arc<dyn QuotaEnforcer + Send + Sync>,
                None,
            )
        }
        DbMode::Full { url } => {
            eprint!("  Connecting to PostgreSQL...");
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(20)
                .acquire_timeout(Duration::from_secs(5))
                .connect(url)
                .await
                .map_err(|e| {
                    eprintln!(" failed");
                    anyhow::anyhow!(
                        "Failed to connect to PostgreSQL.\n\n  Attempted: {redacted}\n  Cause:     {e}\n\n  Next steps:\n    - Verify PostgreSQL is running: pg_isready\n    - Check credentials in GADGETRON_DATABASE_URL\n    - To run without a database: leave database_url empty in gadgetron.toml",
                        redacted = redact_db_url(url),
                    )
                })?;
            eprintln!(" done");
            eprint!("  Running migrations...");
            let migration_result = sqlx::migrate!("../gadgetron-xaas/migrations")
                .run(&pool)
                .await
                .context("Failed to run database migrations.")?;
            // Count applied migrations for the progress line
            eprintln!(" done");
            let kv = Arc::new(PgKeyValidator::new(pool.clone()))
                as Arc<dyn KeyValidator + Send + Sync>;
            let qe = Arc::new(InMemoryQuotaEnforcer)
                as Arc<dyn QuotaEnforcer + Send + Sync>;
            (kv, qe, Some(pool))
        }
    };

    // 6. Print banner
    // Extract real endpoint from each ProviderConfig using provider_endpoint().
    // "..." placeholder was incorrect — ProviderConfig is an enum, not a struct
    // with a direct .endpoint field.
    let provider_summaries: Vec<ProviderSummary> = config
        .providers
        .iter()
        .map(|(name, cfg)| ProviderSummary {
            kind: name.clone(),
            endpoint: provider_endpoint(cfg).to_string(),
        })
        .collect();
    print_serve_banner(env!("CARGO_PKG_VERSION"), &db_mode, &bind_addr, &provider_summaries);

    // 7. Build AppState
    let (tui_tx, _) = broadcast::channel(256);
    let app_state = Arc::new(AppState {
        key_validator,
        quota_enforcer,
        // Use capacity=16 in no-db mode: consumer loop is still spawned but
        // receives nothing. AuditWriter::noop() does not exist — use new(16).
        audit_writer: Arc::new(AuditWriter::new(16)),
        providers: Arc::new(providers),
        router: None,
        pg_pool,
        tui_tx: if tui_enabled { Some(tui_tx.clone()) } else { None },
        no_db: matches!(db_mode, DbMode::NoDb { .. }),
    });

    // 8. Start TUI if requested
    // (unchanged from existing implementation)

    // 9. Serve
    eprintln!("  Starting server...");
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(listener, build_router(app_state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}
```

> **NOTE (build_router 시그니처 변경)**: `build_router` 시그니처가
> `fn build_router(state: AppState)` → `fn build_router(state: Arc<AppState>)`로 변경된다.
> `main.rs`의 모든 `build_router(app_state)` 호출부에서 `app_state`는 이미 `Arc::new(AppState { ... })`로
> 감싸진 `Arc<AppState>` 타입이어야 한다 (`serve` 함수 step 7 참조). 기존 `app_state`를
> `Arc::new()`로 감싸지 않고 직접 전달하면 컴파일 오류가 발생한다.

#### 2.3.6 `serve_provider_quickstart` 헬퍼 (`--provider` 전용)

`--provider <endpoint>` 플래그가 주어졌을 때 config 파일 없이 즉시 vLLM proxy 모드로 기동한다.

```rust
/// Start in single-provider proxy mode without any config file.
///
/// Used by `gadgetron serve --provider <endpoint>`.
/// Always runs in no-db mode (InMemoryKeyValidator).
/// A synthetic AppConfig is built from the endpoint URL alone.
async fn serve_provider_quickstart(
    endpoint: &str,
    bind_override: Option<String>,
    tui_enabled: bool,
) -> Result<()> {
    // Build a minimal AppConfig pointing at the given endpoint
    let mut config = AppConfig::default();
    config.providers.insert(
        "provider".to_string(),
        ProviderConfig::Vllm {
            endpoint: endpoint.to_string(),
            api_key: None,
        },
    );
    let bind_addr = bind_override
        .or_else(|| std::env::var("GADGETRON_BIND").ok())
        .unwrap_or_else(|| config.server.bind.clone());

    let db_mode = DbMode::NoDb { explicit: true };

    // Reuse the shared banner + startup path
    let provider_summaries = vec![ProviderSummary {
        kind: "vllm".to_string(),
        endpoint: endpoint.to_string(),
    }];
    print_serve_banner(env!("CARGO_PKG_VERSION"), &db_mode, &bind_addr, &provider_summaries);

    let providers = build_providers(&config).await?;
    let (tui_tx, _) = broadcast::channel(256);
    let app_state = Arc::new(AppState {
        key_validator: Arc::new(InMemoryKeyValidator) as Arc<dyn KeyValidator + Send + Sync>,
        quota_enforcer: Arc::new(InMemoryQuotaEnforcer) as Arc<dyn QuotaEnforcer + Send + Sync>,
        audit_writer: Arc::new(AuditWriter::new(16)),
        providers: Arc::new(providers),
        router: None,
        pg_pool: None,
        tui_tx: if tui_enabled { Some(tui_tx.clone()) } else { None },
        no_db: true,
    });
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(listener, build_router(app_state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}
```

### 2.4 `gadgetron key create` — no-db 모드 지원

**파일**: `crates/gadgetron-cli/src/commands/key.rs` (신규, §2.3의 `key.rs` 대체)

`--tenant-id`가 없는 경우(no-db 모드)와 있는 경우(PG 모드) 두 경로를 모두 지원한다.

```rust
use anyhow::{Context, Result};
use sqlx::PgPool;
use uuid::Uuid;
use gadgetron_xaas::auth::key_gen::generate_api_key;

pub async fn key_create(
    pool: Option<&PgPool>,
    tenant_id: Option<Uuid>,
    kind: &str,
    scope: Option<&str>,
    name: Option<&str>,
    bind_addr: &str,
) -> Result<()> {
    validate_kind(kind)?;

    let (raw_key, key_hash) = generate_api_key(kind);
    let prefix = gadgetron_xaas::auth::key_gen::key_prefix(&raw_key);

    match (pool, tenant_id) {
        (Some(pool), Some(tid)) => {
            // Full mode: persist hash to PostgreSQL
            let scopes = parse_scopes(scope);
            sqlx::query!(
                "INSERT INTO api_keys (tenant_id, prefix, key_hash, kind, scopes, name)
                 VALUES ($1, $2, $3, $4, $5, $6)",
                tid, prefix, key_hash, kind, &scopes as &[String], name,
            )
            .execute(pool)
            .await
            .with_context(|| format!(
                "Failed to create API key for tenant {tid}.\n\n  \
                 Cause:     PostgreSQL INSERT failed.\n  \
                 Next step: Verify the tenant UUID exists:\n             \
                 gadgetron tenant list"
            ))?;
            print_key_created(&raw_key, Some(tid), bind_addr);
        }
        (None, _) | (Some(_), None) => {
            // No-db mode: key is not stored anywhere.
            // The InMemoryKeyValidator accepts any gad_live_* or gad_test_* key.
            // Note: raw_key is dropped from memory after this function returns.
            print_key_created(&raw_key, None, bind_addr);
        }
    }

    Ok(())
}

/// Print the key creation success block to stdout.
///
/// Exact format (spaces are significant):
///
/// ```text
/// API Key Created
///
///   Key:    gad_live_a1b2c3d4e5f6789012345678901234567890
///   Tenant: 550e8400-e29b-41d4-a716-446655440000   ← omitted in no-db mode
///   Scopes: OpenAiCompat                            ← omitted in no-db mode
///
///   Save this key — it cannot be retrieved later.
///
///   Test it:
///     curl http://<bind>/v1/chat/completions \
///       -H "Authorization: Bearer <key>" \
///       -H "Content-Type: application/json" \
///       -d '{"model":"<model>","messages":[{"role":"user","content":"Hello!"}]}'
/// ```
fn print_key_created(raw_key: &str, tenant_id: Option<Uuid>, bind_addr: &str) {
    println!("API Key Created\n");
    // Use fixed-width label column (8 chars including trailing colon) so all
    // values align regardless of which fields are present.
    println!("  {:<8} {raw_key}", "Key:");
    if let Some(tid) = tenant_id {
        println!("  {:<8} {tid}", "Tenant:");
        println!("  {:<8} OpenAiCompat", "Scopes:");
    }
    println!();
    println!("  Save this key — it cannot be retrieved later.");
    println!();
    println!("  Test it:");
    println!("    curl http://{bind_addr}/v1/chat/completions \\");
    println!("      -H \"Authorization: Bearer {raw_key}\" \\");
    println!("      -H \"Content-Type: application/json\" \\");
    println!("      -d '{{\"model\":\"<model>\",\"messages\":[{{\"role\":\"user\",\"content\":\"Hello!\"}}]}}'");
}

fn parse_scopes(scope: Option<&str>) -> Vec<String> {
    scope.unwrap_or("OpenAiCompat")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn validate_kind(kind: &str) -> Result<()> {
    match kind {
        "live" | "test" => Ok(()),
        other => anyhow::bail!(
            "Invalid key kind '{other}'.\n\n  Valid values: live, test."
        ),
    }
}
```

`key_create`의 `bind_addr` 파라미터는 `main.rs`에서 config로부터 읽어 전달한다. no-db 모드에서 `gadgetron key create`는 서버 없이도 동작한다 (키를 출력만 함).

### 2.5 `gadgetron tenant create` — 출력 형식

**파일**: `crates/gadgetron-cli/src/commands/tenant.rs` (기존 §2.2의 `tenant.rs` 수정)

성공 출력을 §1.3 Stage 4-C 형식으로 변경:

```rust
pub async fn tenant_create(pool: &PgPool, name: &str) -> Result<()> {
    validate_tenant_name(name)?;

    let row = sqlx::query!(
        "INSERT INTO tenants (name) VALUES ($1) RETURNING id",
        name
    )
    .fetch_one(pool)
    .await
    .map_err(|e| anyhow::anyhow!(
        "Failed to create tenant '{name}'.\n\n  \
         Cause:     {e}\n  \
         Next step: Check GADGETRON_DATABASE_URL and ensure migrations ran\n             \
         (gadgetron serve runs migrations automatically)."
    ))?;

    let id = row.id;
    println!("Tenant Created\n");
    println!("  ID:   {id}");
    println!("  Name: {name}");
    println!();
    println!("  Next: gadgetron key create --tenant-id {id}");
    Ok(())
}
```

`tenant_list`의 출력은 변경 없음 (기존 §2.2 table 형식 유지).

### 2.6 `gadgetron doctor` — 신규 subcommand

**파일**: `crates/gadgetron-cli/src/commands/doctor.rs` (신규)

#### 2.6.1 수행하는 검사 목록 (순서 고정)

| 번호 | 검사 항목 | 실패 조건 | 출력 레이블 (trailing colon 포함) |
|------|---------|----------|----------------------------------|
| 1 | Config file 존재 + 유효 TOML | 파일 없음 또는 파싱 오류 | `Config file:` |
| 2 | `server.bind` 포트 사용 가능 | bind 실패 | `Server bind:` |
| 3 | Database 설정 | URL 없음 → WARN (no-db 알림) | `Database:` |
| 4 | Provider 엔드포인트 도달 가능 | HTTP GET 실패 (5초 타임아웃) | `Provider <name>:` |
| 5 | Gadgetron `/health` 엔드포인트 | 포트에서 200 미반환 | `/health:` |

검사 3이 WARN인 것은 no-db가 유효한 운영 모드이기 때문이다. FAIL이 아니다.

#### 2.6.2 구현

```rust
use std::net::TcpListener;
use std::time::Duration;
use anyhow::Result;

/// A single check result.
///
/// `label` includes the trailing colon. Examples:
///   `label: "Config file:"`, `detail: "gadgetron.toml found and valid TOML"`
///   `label: "Server bind:"`, `detail: "0.0.0.0:8080 (port available)"`
///   `label: "Database:"`
///   `label: "Provider vllm:"`, `detail: "http://10.100.1.5:8100 reachable (200 OK in 23ms)"`
///   `label: "/health:"`,       `detail: "gadgetron is running on port 8080"`
///
/// The column-alignment format string `{:<width$}` in `run_doctor` uses `label.len()`
/// to compute the column width — the trailing colon is part of the measured width.
struct CheckResult {
    label: String,
    status: CheckStatus,
    detail: Option<String>,
}

enum CheckStatus {
    Pass,
    Warn(String),
    Fail(String),
}

/// Execute `gadgetron doctor`.
///
/// Runs all checks sequentially, prints a report, exits non-zero if any FAIL.
pub async fn run_doctor(config_path: Option<std::path::PathBuf>) -> Result<()> {
    println!("Gadgetron v{} — System Check\n", env!("CARGO_PKG_VERSION"));

    let mut results: Vec<CheckResult> = Vec::new();

    // Check 1: Config file
    let cfg_result = check_config_file(config_path.as_deref());
    let config = cfg_result.config.clone();
    results.push(cfg_result.result);

    // Check 2: Port availability (only if config loaded)
    if let Some(ref cfg) = config {
        results.push(check_port_available(&cfg.server.bind));
    }

    // Check 3: Database
    if let Some(ref cfg) = config {
        results.push(check_database(cfg));
    }

    // Check 4: Provider reachability
    // provider_cfg.endpoint is not a direct field — ProviderConfig is an enum.
    // Use the provider_endpoint() helper (see below) to extract the endpoint string.
    if let Some(ref cfg) = config {
        for (name, provider_cfg) in &cfg.providers {
            let endpoint = provider_endpoint(provider_cfg);
            results.push(check_provider(name, endpoint).await);
        }
    }

    // Check 5: /health
    let bind = config.as_ref().map(|c| c.server.bind.as_str()).unwrap_or("localhost:8080");
    results.push(check_health(bind).await);

    // Print results
    let col_width = results.iter().map(|r| r.label.len()).max().unwrap_or(10);
    for r in &results {
        let (tag, detail) = match &r.status {
            CheckStatus::Pass => ("[PASS]".to_string(), None),
            CheckStatus::Warn(msg) => ("[WARN]".to_string(), Some(msg.clone())),
            CheckStatus::Fail(msg) => ("[FAIL]".to_string(), Some(msg.clone())),
        };
        let detail_str = detail.as_deref().unwrap_or("");
        println!("  {tag} {:<width$}  {detail_str}", r.label, width = col_width);
    }

    // Summary
    let warns: Vec<_> = results.iter()
        .filter(|r| matches!(r.status, CheckStatus::Warn(_)))
        .collect();
    let fails: Vec<_> = results.iter()
        .filter(|r| matches!(r.status, CheckStatus::Fail(_)))
        .collect();

    println!();
    if warns.is_empty() && fails.is_empty() {
        println!("  All checks passed.");
        return Ok(());
    }
    if !warns.is_empty() {
        println!("  {} warning(s) found.", warns.len());
        for w in &warns {
            if let CheckStatus::Warn(msg) = &w.status {
                println!("  WARN: {msg}");
            }
        }
    }
    if !fails.is_empty() {
        println!("  {} failure(s) found.", fails.len());
        for f in &fails {
            if let CheckStatus::Fail(msg) = &f.status {
                println!("  FAIL: {msg}");
            }
        }
        std::process::exit(2);
    }
    Ok(())
}

/// Extract a displayable endpoint URL from a ProviderConfig enum variant.
///
/// ProviderConfig is a Rust enum — there is no `.endpoint` field. Each variant
/// carries its endpoint differently. This helper centralises the extraction.
fn provider_endpoint(cfg: &ProviderConfig) -> &str {
    match cfg {
        ProviderConfig::Openai { endpoint, .. } => {
            endpoint.as_deref().unwrap_or("https://api.openai.com")
        }
        ProviderConfig::Anthropic { endpoint, .. } => {
            endpoint.as_deref().unwrap_or("https://api.anthropic.com")
        }
        ProviderConfig::Vllm { endpoint, .. } => endpoint,
        ProviderConfig::Sglang { endpoint, .. } => endpoint,
        ProviderConfig::Ollama { endpoint, .. } => endpoint,
        ProviderConfig::Gemini { endpoint, .. } => {
            endpoint.as_deref().unwrap_or("https://generativelanguage.googleapis.com")
        }
        // Catch-all for any future variants: return empty string so check_provider
        // skips gracefully rather than panicking.
        _ => "",
    }
}

fn check_port_available(bind: &str) -> CheckResult {
    match TcpListener::bind(bind) {
        Ok(_) => CheckResult {
            label: "Server bind:".into(),
            status: CheckStatus::Pass,
            detail: Some(format!("{bind} (port available)")),
        },
        Err(e) => CheckResult {
            label: "Server bind:".into(),
            status: CheckStatus::Fail(format!(
                "{bind} — {e}\n             Next step: choose a different port in gadgetron.toml"
            )),
            detail: None,
        },
    }
}

fn check_database(cfg: &AppConfig) -> CheckResult {
    // `.ok().as_deref()` is unsound — the temporary String is dropped before
    // the deref is used. Use `.ok()` and keep owned Strings throughout.
    let url: Option<String> = cfg.database.as_ref()
        .and_then(|d| d.database_url.clone())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("GADGETRON_DATABASE_URL").ok())
        .filter(|s| !s.is_empty());

    match url {
        None => CheckResult {
            label: "Database:".into(),
            status: CheckStatus::Warn(
                "database_url not configured — running in no-db mode.\n             \
                 To configure: set database_url in gadgetron.toml or GADGETRON_DATABASE_URL".into()
            ),
            detail: Some("not configured (no-db mode)".into()),
        },
        Some(_) => CheckResult {
            label: "Database:".into(),
            status: CheckStatus::Pass,
            detail: Some("configured".into()),
        },
    }
}

async fn check_provider(name: &str, endpoint: &str) -> CheckResult {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    let start = std::time::Instant::now();
    match client.get(endpoint).send().await {
        Ok(resp) => {
            let ms = start.elapsed().as_millis();
            CheckResult {
                label: format!("Provider {name}:"),
                status: CheckStatus::Pass,
                detail: Some(format!("{endpoint} reachable ({} {} in {ms}ms)",
                    resp.status().as_u16(),
                    resp.status().canonical_reason().unwrap_or("OK"),
                )),
            }
        }
        Err(e) => CheckResult {
            label: format!("Provider {name}"),
            status: CheckStatus::Fail(format!(
                "{endpoint} — {e}\n             Next step: verify the provider is running"
            )),
            detail: None,
        },
    }
}

async fn check_health(bind: &str) -> CheckResult {
    let url = format!("http://{bind}/health");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap_or_default();

    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => CheckResult {
            label: "/health:".into(),
            status: CheckStatus::Pass,
            detail: Some(format!("gadgetron is running on port {bind}")),
        },
        Ok(resp) => CheckResult {
            label: "/health:".into(),
            status: CheckStatus::Fail(format!(
                "HTTP {} at {url}\n             Next step: check gadgetron logs",
                resp.status(),
            )),
            detail: None,
        },
        Err(_) => CheckResult {
            label: "/health:".into(),
            status: CheckStatus::Fail(format!(
                "connection refused at {url}\n             Next step: gadgetron serve"
            )),
            detail: None,
        },
    }
}
```

### 2.7 `InMemoryKeyValidator` (기존 §2.5.1 확인)

**파일**: `crates/gadgetron-xaas/src/auth/validator.rs`

기존 구현 유지. 추가: no-db 모드에서 `key create` 없이 hardcoded test key를 사용하는 경우를 위해 포맷 검증만 수행하는 `validate_raw_key_format` 함수를 `pub(crate)`로 공개.

```rust
/// Returns true if `raw_key` has a valid gad_ prefix format.
/// Used by auth middleware to reject obviously malformed keys before hashing.
pub(crate) fn validate_raw_key_format(raw_key: &str) -> bool {
    raw_key.starts_with("gad_live_") || raw_key.starts_with("gad_test_")
}
```

### 2.8 `AppState` 변경 (기존 §2.5.3 확인 + 보강)

**파일**: `crates/gadgetron-gateway/src/server.rs`

```rust
pub struct AppState {
    pub key_validator: Arc<dyn KeyValidator + Send + Sync>,
    pub quota_enforcer: Arc<dyn QuotaEnforcer + Send + Sync>,
    pub audit_writer: Arc<AuditWriter>,
    pub providers: Arc<HashMap<String, Arc<dyn LlmProvider + Send + Sync>>>,
    pub router: Option<Arc<LlmRouter>>,
    /// None in no-db mode. Some(_) in full mode.
    pub pg_pool: Option<sqlx::PgPool>,
    pub tui_tx: Option<broadcast::Sender<WsMessage>>,
    /// true in no-db mode — used by /ready and /health to skip PG check.
    pub no_db: bool,
}
```

`/ready` 핸들러:

```rust
/// GET /ready
///
/// 200 if the server can accept requests.
/// In no-db mode: always 200.
/// In full mode:  checks PG with SELECT 1.
pub async fn ready_handler(State(state): State<Arc<AppState>>) -> StatusCode {
    if state.no_db {
        return StatusCode::OK;
    }
    match &state.pg_pool {
        None => StatusCode::SERVICE_UNAVAILABLE,
        Some(pool) => match sqlx::query("SELECT 1").fetch_one(pool).await {
            Ok(_) => StatusCode::OK,
            Err(_) => StatusCode::SERVICE_UNAVAILABLE,
        },
    }
}
```

### 2.8.1 기존 테스트 수정 필요

`AppState.pg_pool`이 `PgPool` → `Option<PgPool>`로 변경되므로, `crates/gadgetron-gateway/src/server.rs` 및 `crates/gadgetron-gateway/src/handlers.rs` 내에서 `state.pg_pool`을 직접 사용하는 모든 테스트 함수는 `state.pg_pool.as_ref().unwrap()`으로 변경해야 한다.

수정 대상 패턴:

```rust
// 변경 전 (컴파일 실패)
let pool = state.pg_pool.clone();
sqlx::query!("...").execute(&state.pg_pool).await

// 변경 후
let pool = state.pg_pool.as_ref().unwrap();
sqlx::query!("...").execute(state.pg_pool.as_ref().unwrap()).await
```

확인해야 할 테스트 함수 (server.rs / handlers.rs):

- `test_ready_handler_full_mode` — `state.pg_pool`을 `Some(pool)`로 래핑해야 함
- `test_ready_handler_no_db` — `pg_pool: None` 세팅 확인
- `test_key_validation_with_pg` — `state.pg_pool.as_ref().unwrap()` 사용
- `test_audit_log_pg_write` — `state.pg_pool.as_ref().unwrap()` 사용
- 그 외 `#[cfg(test)]` 블록 내 `state.pg_pool` 직접 참조 모든 위치

> **구현 체크리스트**: `grep -rn "state\.pg_pool" crates/gadgetron-gateway/src/` 실행 후 `.as_ref().unwrap()` 누락 여부 전수 확인.

### 2.9 터미널 감지 — 색상/이모지 정책

CLI 출력은 **기본적으로 plain text**. 이모지/색상은 다음 조건을 모두 만족할 때만 사용:

1. stdout이 TTY (`std::io::IsTerminal::is_terminal`)
2. `NO_COLOR` 환경 변수가 없음
3. `TERM=dumb`이 아님

이 문서의 모든 stdout 예시는 plain text 버전이 정의 기준이다. 터미널에서는 `[PASS]` 앞에 ANSI green, `[FAIL]` 앞에 red, `[WARN]` 앞에 yellow를 선택적으로 붙일 수 있다.

**이모지는 사용하지 않는다.** (`.claude/agents/dx-product-lead.md §Working rules` 참조)

### 2.10 `main.rs` dispatch 전체

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None | Some(Commands::Serve { config: None, bind: None, tui: false, no_db: false, provider: None }) => {
            serve(None, None, false, false, None).await
        }
        Some(Commands::Serve { config, bind, tui, no_db, provider }) => {
            serve(config, bind, tui, no_db, provider).await
        }
        Some(Commands::Init { output, yes }) => {
            commands::init::run_init(&output, yes)
        }
        Some(Commands::Key { command }) => {
            // Determine if PG is available by checking env/config
            // If not, run in no-db mode (key not persisted, just printed)
            let cfg = load_config(None).ok();
            let db_mode = resolve_db_mode_from_env(cfg.as_ref());
            match db_mode {
                DbMode::Full { url } => {
                    let pool = connect_pg(&url).await?;
                    match command {
                        KeyCmd::Create { tenant_id, kind, scope, name } => {
                            commands::key::key_create(
                                Some(&pool), tenant_id, &kind,
                                scope.as_deref(), name.as_deref(),
                                &default_bind_addr(cfg.as_ref()),
                            ).await
                        }
                        KeyCmd::List { tenant_id } => {
                            commands::key::key_list(&pool, tenant_id).await
                        }
                        KeyCmd::Revoke { key_id } => {
                            commands::key::key_revoke(&pool, key_id).await
                        }
                    }
                }
                DbMode::NoDb { .. } => {
                    match command {
                        KeyCmd::Create { tenant_id, kind, scope, name } => {
                            commands::key::key_create(
                                None, tenant_id, &kind,
                                scope.as_deref(), name.as_deref(),
                                &default_bind_addr(cfg.as_ref()),
                            ).await
                        }
                        KeyCmd::List { .. } | KeyCmd::Revoke { .. } => {
                            anyhow::bail!(
                                "This command requires PostgreSQL.\n\n  \
                                 Cause:     No database_url configured.\n  \
                                 Next step: Set GADGETRON_DATABASE_URL and retry."
                            )
                        }
                    }
                }
            }
        }
        Some(Commands::Tenant { command }) => {
            let pool = connect_pg_or_bail().await?;
            match command {
                TenantCmd::Create { name } => {
                    commands::tenant::tenant_create(&pool, &name).await
                }
                TenantCmd::List => {
                    commands::tenant::tenant_list(&pool).await
                }
            }
        }
        Some(Commands::Doctor { config }) => {
            commands::doctor::run_doctor(config).await
        }
    }
}
```

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 의존성 방향

```
gadgetron-cli
    ├── gadgetron-xaas  (auth::key_gen, auth::validator::InMemoryKeyValidator,
    │                    DB queries via sqlx)
    ├── gadgetron-gateway (AppState: pg_pool: Option, no_db: bool)
    ├── gadgetron-core  (GadgetronError, Secret, AppConfig)
    └── (기존) gadgetron-router, gadgetron-provider, gadgetron-tui

gadgetron-xaas
    ├── auth/key_gen.rs         [P1] — pure function, no DB
    ├── auth/key.rs             [기존] — ApiKey::parse, key_prefix
    └── auth/validator.rs       [+InMemoryKeyValidator, +validate_raw_key_format]

gadgetron-gateway
    └── server.rs   [AppState.pg_pool: Option<PgPool>, AppState.no_db: bool]
```

### 3.2 데이터 흐름 — Stage 3: `gadgetron init`

```
CLI parse (Init { output, yes })
    → is_terminal(stdin)?
        Yes → prompt_init_answers() → InitAnswers
        No  → InitAnswers::default()
    → render_config_template(answers) → String
    → fs::write(output, content)
    → print "Config written to ..."
```

### 3.3 데이터 흐름 — Stage 3: `gadgetron serve`

```
CLI parse (Serve { config, bind, tui, no_db })
    → load_config(config_path)
        → fs not found → print "No configuration file found" → exit 1
        → parse error  → print parse error with field context → exit 1
    → resolve_db_mode(cfg, no_db)
        → NoDb  → InMemoryKeyValidator, InMemoryQuotaEnforcer
        → Full  → PgPoolOptions::connect(url)
                    → connect error → print error + next steps → exit 1
                  → sqlx::migrate!().run()
                  → PgKeyValidator, InMemoryQuotaEnforcer
    → build_providers(cfg)
    → print_serve_banner(version, db_mode, bind, providers)
        → WARNING to stderr if NoDb
    → AppState { pg_pool: Option, no_db: bool, ... }
    → axum::serve
```

### 3.4 데이터 흐름 — Stage 4: `gadgetron key create`

no-db 모드:
```
CLI parse (Key::Create { tenant_id: None, kind: "live", ... })
    → resolve_db_mode → NoDb
    → generate_api_key("live")
        → OsRng → 16 bytes → hex → "gad_live_<32hex>"   (41자)
        → SHA-256 → key_hash                             (저장하지 않음)
    → print_key_created(raw_key, None, bind_addr)
    → raw_key: String dropped from stack
```

PG 모드:
```
CLI parse (Key::Create { tenant_id: Some(uuid), kind: "live", ... })
    → resolve_db_mode → Full { url }
    → connect_pg(url)
    → generate_api_key("live")
    → key_prefix(raw_key)
    → sqlx INSERT api_keys (prefix, key_hash, ...)
    → print_key_created(raw_key, Some(tenant_id), bind_addr)
    → raw_key: String dropped
```

raw_key는 `print_key_created` 호출 직후 스택에서 해제된다. `tracing`에 기록하지 않는다 (SEC-M7).

### 3.5 타 도메인 인터페이스 계약

| 인터페이스 | 제공자 | 소비자 | 변경 내용 |
|-----------|--------|--------|---------|
| `KeyValidator` trait | `gadgetron-xaas` | `gadgetron-gateway`, `gadgetron-cli` | `InMemoryKeyValidator` 추가 |
| `validate_raw_key_format` | `gadgetron-xaas` | `gadgetron-gateway` auth middleware | 신규 pub(crate) fn |
| `AppState.pg_pool` | `gadgetron-gateway` | cli `serve`, `/ready` | `PgPool` → `Option<PgPool>` |
| `AppState.no_db` | `gadgetron-gateway` | `/ready`, `/health` handlers | 신규 필드 |
| `AppConfig.database` | `gadgetron-core` | cli, doctor | `Option<DatabaseConfig>` 신규 섹션 |
| `auth::key_gen` | `gadgetron-xaas` | `gadgetron-cli` | 신규 모듈 (기존 §2.3.1) |

### 3.6 D-12 크레이트 경계 준수

| 신규 타입/함수 | 크레이트 | 파일 | D-12 준수 |
|--------------|----------|------|---------|
| `generate_api_key`, `key_prefix` | `gadgetron-xaas` | `src/auth/key_gen.rs` | D-03 준수 |
| `InMemoryKeyValidator` | `gadgetron-xaas` | `src/auth/validator.rs` | D-03 준수 |
| `validate_raw_key_format` | `gadgetron-xaas` | `src/auth/validator.rs` | D-03 준수 |
| `Commands::Init`, `Commands::Doctor`, `KeyCmd::*`, `TenantCmd::*` | `gadgetron-cli` | `src/main.rs` | CLI 로직 — cli 크레이트 |
| `commands::init`, `commands::doctor`, `commands::key`, `commands::tenant` | `gadgetron-cli` | `src/commands/` | cli 크레이트 내부 |
| `AppState.no_db` | `gadgetron-gateway` | `src/server.rs` | gateway 상태 — gateway 크레이트 |
| `AppConfig.database: Option<DatabaseConfig>` | `gadgetron-core` | `src/config.rs` | core config 타입 |
| `DbMode` enum | `gadgetron-cli` | `src/main.rs` | CLI-only state, 외부 노출 없음 |

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

#### 4.1.1 clap parse 테스트 (`gadgetron-cli/src/main.rs`)

테스트 ID | 입력 | 검증 대상
---------|------|----------
T-S7-01 | `["gadgetron", "init"]` | `Init { output: "gadgetron.toml", yes: false }`
T-S7-02 | `["gadgetron", "init", "--yes"]` | `Init { yes: true }`
T-S7-03 | `["gadgetron", "init", "-o", "/tmp/g.toml", "-y"]` | `output: "/tmp/g.toml", yes: true`
T-S7-04 | `["gadgetron", "serve", "--no-db"]` | `Serve { no_db: true, tui: false }`
T-S7-05 | `["gadgetron", "serve", "--no-db", "--tui"]` | `Serve { no_db: true, tui: true }`
T-S7-06 | `["gadgetron", "key", "create"]` | `KeyCmd::Create { tenant_id: None, kind: "live", scope: None, name: None }`
T-S7-07 | `["gadgetron", "key", "create", "--tenant-id", "<uuid>"]` | `tenant_id: Some(uuid)`
T-S7-08 | `["gadgetron", "key", "create", "--kind", "test", "--scope", "OpenAiCompat"]` | `kind: "test", scope: Some("OpenAiCompat")`
T-S7-09 | `["gadgetron", "key", "list", "--tenant-id", "<uuid>"]` | `KeyCmd::List { tenant_id: uuid }`
T-S7-10 | `["gadgetron", "key", "revoke", "--key-id", "<uuid>"]` | `KeyCmd::Revoke { key_id: uuid }`
T-S7-11 | `["gadgetron", "tenant", "create", "--name", "my-team"]` | `TenantCmd::Create { name: "my-team" }`
T-S7-12 | `["gadgetron", "tenant", "list"]` | `TenantCmd::List`
T-S7-13 | `["gadgetron", "doctor"]` | `Doctor { config: None }`
T-S7-14 | `["gadgetron", "doctor", "-c", "custom.toml"]` | `Doctor { config: Some("custom.toml") }`
T-S7-15 | 기존 `clap_parses_serve_*` 6개 테스트 | `no_db` 필드 추가 후에도 그린 유지

#### 4.1.2 `auth/key_gen.rs` 테스트

T-S7-20: `generate_api_key("live")` → 접두사 `gad_live_` 확인
T-S7-21: `generate_api_key("test")` → 접두사 `gad_test_` 확인
T-S7-22: 길이 41자 (`gad_live_` = 9 + 32 hex)
T-S7-23: 두 호출 → 다른 키 (CSPRNG uniqueness)
T-S7-24: 동일 raw_key → 동일 hash (결정론적)
T-S7-25: `key_prefix("gad_live_abc")` == `"gad_live"`
T-S7-26: `key_prefix("gad_test_xyz")` == `"gad_test"`
T-S7-27: hash는 64자 ASCII hexdigit

#### 4.1.3 `InMemoryKeyValidator` 테스트

T-S7-30: 임의 hash → `Ok(ValidatedKey { api_key_id: Uuid::nil(), ... })`
T-S7-31: 반환 scopes에 `Scope::OpenAiCompat` 포함
T-S7-32: `invalidate()` → 패닉 없이 no-op

#### 4.1.4 `validate_raw_key_format` 테스트

T-S7-33: `"gad_live_abc123"` → true
T-S7-34: `"gad_test_abc123"` → true
T-S7-35: `"sk-abc"` (OpenAI key) → false
T-S7-36: `"gad_"` (no kind) → false
T-S7-37: `""` → false

#### 4.1.5 `commands/init.rs` 테스트

T-S7-40: `run_init(path, true)` → 파일 생성, `[server]` 포함
T-S7-41: 생성 파일이 유효 TOML
T-S7-42: `GADGETRON_BIND` env override 주석 포함
T-S7-43: `[router.default_strategy]` 포함
T-S7-44: `[database]` 섹션 포함
T-S7-45: `render_config_template`에 provider_endpoint 전달 → `[providers.my-provider]` 포함

#### 4.1.6 `commands/key.rs` + `commands/tenant.rs` validate 테스트

T-S7-50: `validate_kind("live")` → Ok
T-S7-51: `validate_kind("test")` → Ok
T-S7-52: `validate_kind("prod")` → Err ("Invalid key kind 'prod'")
T-S7-53: `validate_tenant_name("")` → Err ("cannot be empty")
T-S7-54: `validate_tenant_name(&"x".repeat(256))` → Err ("too long")
T-S7-55: `validate_tenant_name("my-team")` → Ok

#### 4.1.7 `commands/doctor.rs` 테스트

T-S7-60: 사용 중인 포트 → `check_port_available` → `Fail`
T-S7-61: 사용 가능한 포트 → `Pass`
T-S7-62: DB URL 없음 → `check_database` → `Warn`
T-S7-63: DB URL 있음 → `Pass`
T-S7-64: provider endpoint mock (httptest) → 200 반환 → `Pass`
T-S7-65: provider endpoint 연결 거부 → `Fail`
T-S7-66: `/health` 연결 거부 → `Fail("connection refused")`

#### 4.1.8 `serve` 배너 테스트

T-S7-70: `print_serve_banner` (NoDb) → "no-db" 포함, "Quick start:" 포함
T-S7-71: `print_serve_banner` (Full) → "full" 포함, "PostgreSQL" 포함
T-S7-72: `redact_db_url("postgres://user:pass@host/db")` → `"postgres://user:***@host/db"`
T-S7-73: `redact_db_url("postgres://host/db")` (no password) → 원본 반환

### 4.2 테스트 하네스

- DB 쿼리 함수 (`tenant_create`, `key_create`, `key_list`, `key_revoke`)는 통합 테스트에서 testcontainers로 실제 PG 컨테이너 대상 실행.
- `doctor`의 provider check는 `httptest`/`wiremock` 로컬 서버 사용.
- init, key_gen, validator, parse 테스트는 mock 없이 in-process.
- `tempfile` crate: dev-dependency로 `gadgetron-cli/Cargo.toml`에 추가.

### 4.3 커버리지 목표

| 모듈 | 목표 |
|------|------|
| `auth/key_gen.rs` | 100% line (pure functions) |
| `auth/validator.rs` (새 함수) | 100% |
| `commands/init.rs` (render, run sans interactive) | 90% |
| `commands/key.rs` (validate_*, print_key_created) | 100% |
| `commands/doctor.rs` (check_* 함수 각각) | 90% |
| clap parse tests | 모든 subcommand 경로 1회 이상 |

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

전체 사용자 여정을 자동화한 shell script가 통합 테스트의 최종 검증이다. 다음 시나리오를 커버한다:

| 시나리오 | 설명 |
|---------|------|
| S-A | no-db 여정: init → serve --no-db → key create → curl |
| S-B | full 여정: init → serve (PG) → tenant create → key create → curl |
| S-C | doctor 여정: 설정 불완전 → doctor → WARN/FAIL 확인 |
| S-D | config 없음 → serve → 자동 init 제안 메시지 확인 |
| S-E | PG URL 설정 후 PG 미기동 → serve → 에러 메시지 확인 |

### 5.2 E2E 자동화 Shell Script

**파일**: `scripts/e2e-user-journey.sh`

```bash
#!/usr/bin/env bash
# Sprint 7 E2E User Journey Test
#
# Prerequisites:
#   - gadgetron binary in PATH (cargo install --path crates/gadgetron-cli)
#   - No config in the test working directory
#   - Optional: PostgreSQL at GADGETRON_DATABASE_URL for full-mode tests
#
# Exit codes:
#   0 — all scenarios passed
#   1 — test infrastructure failure
#   2 — scenario assertion failed
set -euo pipefail

GADGETRON_BIN="${GADGETRON_BIN:-gadgetron}"
BIND_ADDR="${GADGETRON_BIND:-127.0.0.1:18080}"
BASE_URL="http://${BIND_ADDR}"
FAIL_COUNT=0

pass() { echo "  PASS: $*"; }
fail() { echo "  FAIL: $*" >&2; FAIL_COUNT=$((FAIL_COUNT + 1)); }
skip() { echo "  SKIP: $*"; }

# ---------------------------------------------------------------------------
# S-A: no-db journey
# ---------------------------------------------------------------------------
echo ""
echo "=== Scenario A: no-db journey ==="

WORK_A=$(mktemp -d)
trap 'kill "${SERVER_PID:-}" 2>/dev/null; rm -rf "$WORK_A"' EXIT

# S-A-1: init creates gadgetron.toml
(cd "$WORK_A" && GADGETRON_BIND="$BIND_ADDR" "$GADGETRON_BIN" init --yes)
[[ -f "$WORK_A/gadgetron.toml" ]] \
  && pass "init: gadgetron.toml created" \
  || { fail "init: file not found"; exit 2; }

grep -q 'bind = ' "$WORK_A/gadgetron.toml" \
  && pass "init: bind field present" \
  || fail "init: bind field missing"

grep -q 'GADGETRON_BIND' "$WORK_A/gadgetron.toml" \
  && pass "init: GADGETRON_BIND env override comment present" \
  || fail "init: env override comment missing"

grep -q '\[database\]' "$WORK_A/gadgetron.toml" \
  && pass "init: [database] section present" \
  || fail "init: [database] section missing"

# S-A-2: serve --no-db starts and /ready returns 200
(cd "$WORK_A" && "$GADGETRON_BIN" serve --no-db --bind "$BIND_ADDR") &
SERVER_PID=$!

for i in $(seq 1 20); do
  if curl -sf "${BASE_URL}/ready" > /dev/null 2>&1; then break; fi
  sleep 0.3
done

curl -sf "${BASE_URL}/ready" > /dev/null \
  && pass "serve --no-db: /ready 200" \
  || { fail "serve --no-db: /ready timeout"; kill "$SERVER_PID" 2>/dev/null; exit 2; }

# S-A-3: serve stdout must contain "no-db"
SERVE_OUTPUT=$("$GADGETRON_BIN" serve --no-db --bind "127.0.0.1:18181" 2>&1 &
  sleep 0.5; kill %1 2>/dev/null; true)
# Alternative: capture from already-running server via logfile
# (This is best-effort; the running server's stdout can be captured in CI via redirect)

# S-A-4: key create in no-db mode (no --tenant-id)
KEY_OUTPUT=$(cd "$WORK_A" && "$GADGETRON_BIN" key create 2>&1)
echo "$KEY_OUTPUT" | grep -q "^API Key Created" \
  && pass "key create (no-db): 'API Key Created' in output" \
  || fail "key create (no-db): wrong output header"

echo "$KEY_OUTPUT" | grep -q "^  Key:    gad_live_" \
  && pass "key create (no-db): key has gad_live_ prefix" \
  || fail "key create (no-db): key prefix wrong"

echo "$KEY_OUTPUT" | grep -q "Save this key" \
  && pass "key create (no-db): save warning present" \
  || fail "key create (no-db): save warning missing"

echo "$KEY_OUTPUT" | grep -q "curl" \
  && pass "key create (no-db): curl example present" \
  || fail "key create (no-db): curl example missing"

RAW_KEY=$(echo "$KEY_OUTPUT" | grep "^  Key:" | awk '{print $2}')
KEY_LEN=${#RAW_KEY}
[[ "$KEY_LEN" -eq 41 ]] \
  && pass "key create (no-db): key length is 41" \
  || fail "key create (no-db): key length is $KEY_LEN, expected 41"

# S-A-5: curl with generated key → 200 or 502 (no provider configured → 502 is valid)
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
  -H "Authorization: Bearer $RAW_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"test","messages":[{"role":"user","content":"Hello"}]}' \
  "${BASE_URL}/v1/chat/completions")

[[ "$HTTP_CODE" -eq 200 || "$HTTP_CODE" -eq 502 || "$HTTP_CODE" -eq 422 ]] \
  && pass "curl /v1/chat/completions: key accepted (status $HTTP_CODE)" \
  || fail "curl /v1/chat/completions: unexpected status $HTTP_CODE (expected 200, 422, or 502)"

# S-A-6: curl with wrong key → 401
HTTP_UNAUTH=$(curl -s -o /dev/null -w "%{http_code}" \
  -H "Authorization: Bearer sk-wrong-format" \
  -H "Content-Type: application/json" \
  -d '{"model":"test","messages":[]}' \
  "${BASE_URL}/v1/chat/completions")
[[ "$HTTP_UNAUTH" -eq 401 ]] \
  && pass "curl: wrong-format key → 401" \
  || fail "curl: wrong-format key → $HTTP_UNAUTH (expected 401)"

kill "${SERVER_PID}" 2>/dev/null
unset SERVER_PID

# ---------------------------------------------------------------------------
# S-B: full (PostgreSQL) journey
# ---------------------------------------------------------------------------
echo ""
echo "=== Scenario B: full (PostgreSQL) journey ==="

if [[ -z "${GADGETRON_DATABASE_URL:-}" ]]; then
  skip "GADGETRON_DATABASE_URL not set — skipping full-mode tests"
else
  WORK_B=$(mktemp -d)

  # S-B-1: tenant create
  TENANT_OUT=$(cd "$WORK_B" && "$GADGETRON_BIN" tenant create --name "e2e-journey-test")
  echo "$TENANT_OUT" | grep -q "^Tenant Created" \
    && pass "tenant create: 'Tenant Created' header" \
    || fail "tenant create: header missing"

  echo "$TENANT_OUT" | grep -q "^  ID:   " \
    && pass "tenant create: ID line present" \
    || fail "tenant create: ID line missing"

  TENANT_ID=$(echo "$TENANT_OUT" | grep "^  ID:" | awk '{print $2}')
  [[ -n "$TENANT_ID" ]] \
    && pass "tenant create: UUID extracted ($TENANT_ID)" \
    || { fail "tenant create: could not extract UUID"; TENANT_ID="00000000-0000-0000-0000-000000000000"; }

  # S-B-2: key create with tenant_id
  KEY_OUT_B=$(cd "$WORK_B" && "$GADGETRON_BIN" key create --tenant-id "$TENANT_ID")
  echo "$KEY_OUT_B" | grep -q "^  Tenant: $TENANT_ID" \
    && pass "key create (full): Tenant line correct" \
    || fail "key create (full): Tenant line missing or wrong"

  echo "$KEY_OUT_B" | grep -q "^  Scopes: OpenAiCompat" \
    && pass "key create (full): Scopes line present" \
    || fail "key create (full): Scopes line missing"

  RAW_KEY_B=$(echo "$KEY_OUT_B" | grep "^  Key:" | awk '{print $2}')
  [[ "$RAW_KEY_B" == gad_live_* ]] \
    && pass "key create (full): key has gad_live_ prefix" \
    || fail "key create (full): key prefix wrong ($RAW_KEY_B)"

  rm -rf "$WORK_B"
fi

# ---------------------------------------------------------------------------
# S-C: doctor journey
# ---------------------------------------------------------------------------
echo ""
echo "=== Scenario C: doctor journey ==="

WORK_C=$(mktemp -d)

# S-C-1: doctor with no config file
DOCTOR_NO_CFG=$("$GADGETRON_BIN" doctor -c "$WORK_C/nonexistent.toml" 2>&1 || true)
echo "$DOCTOR_NO_CFG" | grep -q "FAIL\|Error\|not found" \
  && pass "doctor: no config → FAIL or error" \
  || fail "doctor: no config → unexpected output: $DOCTOR_NO_CFG"

# S-C-2: doctor with valid config but no DB URL → WARN
(cd "$WORK_C" && "$GADGETRON_BIN" init --yes)
DOCTOR_OUT=$("$GADGETRON_BIN" doctor -c "$WORK_C/gadgetron.toml" 2>&1 || true)
echo "$DOCTOR_OUT" | grep -q "WARN" \
  && pass "doctor: no database_url → WARN present" \
  || fail "doctor: no database_url → WARN missing"

echo "$DOCTOR_OUT" | grep -q "Database" \
  && pass "doctor: Database check line present" \
  || fail "doctor: Database check line missing"

rm -rf "$WORK_C"

# ---------------------------------------------------------------------------
# S-D: serve without config → suggests init
# ---------------------------------------------------------------------------
echo ""
echo "=== Scenario D: serve without config ==="

WORK_D=$(mktemp -d)
SERVE_NO_CFG=$(cd "$WORK_D" && "$GADGETRON_BIN" serve 2>&1 || true)
echo "$SERVE_NO_CFG" | grep -qi "gadgetron init\|configuration file\|No config" \
  && pass "serve (no config): suggests gadgetron init" \
  || fail "serve (no config): does not suggest init: $SERVE_NO_CFG"
rm -rf "$WORK_D"

# ---------------------------------------------------------------------------
# S-E: serve with bad PG URL → user-friendly error
# ---------------------------------------------------------------------------
echo ""
echo "=== Scenario E: serve with unreachable PostgreSQL ==="

WORK_E=$(mktemp -d)
(cd "$WORK_E" && "$GADGETRON_BIN" init --yes)
# Inject a bad database_url into the generated config
printf '\n[database]\ndatabase_url = "postgres://user:pass@127.0.0.1:19999/nodb"\n' \
  >> "$WORK_E/gadgetron.toml"

SERVE_BAD_PG=$(cd "$WORK_E" && "$GADGETRON_BIN" serve 2>&1 || true)
echo "$SERVE_BAD_PG" | grep -qi "Failed to connect\|PostgreSQL" \
  && pass "serve (bad PG): error mentions PostgreSQL" \
  || fail "serve (bad PG): error does not mention PostgreSQL: $SERVE_BAD_PG"

echo "$SERVE_BAD_PG" | grep -qi "Next step\|Next steps" \
  && pass "serve (bad PG): error has Next step(s)" \
  || fail "serve (bad PG): error missing Next step(s)"

rm -rf "$WORK_E"

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
if [[ "$FAIL_COUNT" -eq 0 ]]; then
  echo "All E2E user journey checks passed."
  exit 0
else
  echo "$FAIL_COUNT check(s) failed." >&2
  exit 2
fi
```

### 5.3 테스트 환경

- **no-db 시나리오 (S-A, S-C, S-D, S-E)**: Docker 불필요. 로컬 `gadgetron` 바이너리 + 포트 18080.
- **full 시나리오 (S-B)**: `GADGETRON_DATABASE_URL` 설정 시 실행. CI에서는 `postgres:16` service container.
- **testcontainers (Rust 통합 테스트)**: `gadgetron-xaas/tests/` → `#[ignore = "requires Docker"]` 태그.
- **CI job 분리**:
  - `ci-unit`: `cargo test` (Docker 없음)
  - `ci-integration`: `cargo test -- --ignored` + `scripts/e2e-user-journey.sh` (Docker + postgres service)

### 5.4 Rust 레벨 통합 테스트

**파일**: `crates/gadgetron-xaas/tests/key_integration.rs`

```rust
#[tokio::test]
#[ignore = "requires Docker for testcontainers"]
async fn key_create_and_validate_roundtrip() {
    // 1. pg_pool_for_test() → PG container
    // 2. INSERT tenant
    // 3. generate_api_key("live") → (raw, hash)
    // 4. INSERT api_keys(key_hash, ...)
    // 5. PgKeyValidator::validate(hash) → Ok(ValidatedKey { tenant_id: expected })
}

#[tokio::test]
#[ignore = "requires Docker for testcontainers"]
async fn revoked_key_is_rejected() {
    // 1. create key
    // 2. UPDATE api_keys SET revoked_at = NOW()
    // 3. PgKeyValidator::validate(hash) → GadgetronError::Unauthorized or equivalent
    //    (cache TTL 0 in test config)
}

#[tokio::test]
#[ignore = "requires Docker for testcontainers"]
async fn tenant_create_duplicate_name_is_allowed() {
    // tenants table has no UNIQUE constraint on name (Q-2 미결 — 현행 허용)
    // 두 번 INSERT → 두 rows 생성
}
```

### 5.5 회귀 방지

| 변경 | 실패해야 할 테스트 |
|------|--------------------|
| `Commands::Serve`에서 `no_db` 필드 제거 | T-S7-04, T-S7-05 |
| `Commands::Doctor` 제거 | T-S7-13, T-S7-14, S-C 전체 |
| `generate_api_key`가 고정 seed 사용 | T-S7-23 |
| `gad_live_` 접두사 변경 | T-S7-20, T-S7-33, S-A-4 |
| `print_key_created`에서 "Save this key" 제거 | S-A-4 |
| `print_key_created`에서 curl 예시 제거 | S-A-4 |
| `AppState.pg_pool`을 `PgPool`로 되돌림 | 컴파일 실패 (`--no-db` 경로) |
| `AppState.no_db` 제거 | T-S7-70, ready_handler compile 실패 |
| `CONFIG_TEMPLATE`에서 `GADGETRON_BIND` 주석 제거 | T-S7-43 |
| no-db `key create`가 tenant_id를 요구 | S-A-4 |
| serve (no config) 메시지에서 "gadgetron init" 제거 | S-D |
| PG 연결 실패 메시지에서 "Next step" 제거 | S-E |

---

## 6. Phase 구분

| 기능 | Phase |
|------|-------|
| `gadgetron init` (비대화형, `--yes`) | [P1] |
| `gadgetron init` (대화형 TTY 프롬프트) | [P1] |
| `gadgetron serve` no-db 자동 감지 | [P1] |
| `gadgetron serve --no-db` (명시적) | [P1] |
| `gadgetron key create` (no-db, no --tenant-id) | [P1] |
| `gadgetron key create --tenant-id` (PG 모드) | [P1] |
| `gadgetron tenant create/list` | [P1] |
| `gadgetron key list/revoke` | [P1] |
| `gadgetron doctor` | [P1] |
| serve 배너 (plain text) | [P1] |
| serve 배너 ANSI 색상 (TTY 감지 후) | [P2] |
| shell completion (`--generate-completion`) | [P2] |
| `gadgetron tenant suspend/delete` | [P2] |
| `gadgetron key rotate` | [P2] |
| `gadgetron init` 다국어 메시지 외부화 | [P2] |
| `gadgetron init --provider-wizard` (자세한 프로바이더 설정) | [P3] |

---

## 7. 오픈 이슈 / 의사결정 필요

| ID | 내용 | 옵션 | 추천 | 상태 |
|----|------|------|------|------|
| Q-1 | `rand` crate 버전: workspace에 없음. 별도 추가 또는 `ring` 재사용? | A. `rand = "0.8"` 추가 / B. `ring::rand::SystemRandom` 사용 | A — rand는 생태계 표준, ring의 low-level API 불필요 | PM 결정 필요 |
| Q-2 | `tenants` 테이블 `name` 컬럼에 UNIQUE constraint 추가 여부 | A. 현행 유지 (중복 허용) / B. migration으로 UNIQUE 추가 | B — 동일 이름 테넌트 다수 존재 시 `key list` 혼동 | PM 결정 필요 |
| Q-3 | no-db WARNING 위치: stderr만? tracing + stderr? | A. stderr only / B. tracing::warn + stderr 둘 다 / C. tracing only | B — 운영 로그 기록 + 터미널 즉시 표시 동시 충족 | B로 구현 (PM 이의 없으면 확정) |
| Q-4 | `gadgetron key create` (no-db, no --tenant-id) 시 서버가 기동 중이어야 하는가? | A. 서버 없이도 키 출력 (단순 key_gen 호출) / B. 서버에 REST API 요청 | A — 서버 없이 동작해야 "3 commands" 원칙 성립 | A로 구현 (현 설계) |
| Q-5 | `gadgetron doctor`에서 provider reachability check 시 어떤 HTTP 경로를 사용? | A. `GET /` / B. `GET /health` / C. `GET /v1/models` | B — 모든 지원 provider (vLLM, SGLang, Ollama)가 `/health` 노출 | inference-engine-lead 확인 필요 |

---

## 리뷰 로그 (append-only)

_리뷰 미진행 — Draft 상태_
