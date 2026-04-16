//! `tokio::process::Command` builder for `claude -p` invocations.
//!
//! Spec: `docs/design/phase2/02-kairos-agent.md §5.1`, `§Appendix B`.
//!
//! # Security rationale (SEC-B1 — env allowlist)
//!
//! `Command::new` inherits the parent process environment by default.
//! Gadgetron's parent process may hold:
//!
//! - `ANTHROPIC_API_KEY` — reusable credential for someone else's account
//! - `DATABASE_URL` — Postgres URI including the server password
//! - `AWS_*`, `GCP_*` — cloud provider credentials
//! - `SSH_AUTH_SOCK` — forwarded SSH agent
//! - `CARGO_REGISTRY_TOKEN`, `GITHUB_TOKEN` — CI / deploy tokens
//! - anything else the operator happens to have exported
//!
//! **None of these should reach the Claude Code subprocess.** Claude Code
//! uses `~/.claude/` OAuth credentials in the default mode, and per
//! `BrainConfig::mode`, only specific env vars (resolved from specific
//! config-named env var names) should be injected.
//!
//! This module calls `env_clear()` immediately after `Command::new` to
//! drop the entire inherited environment, then adds ONLY the allowlist
//! below:
//!
//! - `HOME` — required for `~/.claude/` credential resolution
//! - `PATH` — fixed to `/usr/local/bin:/usr/bin:/bin` (NOT inherited)
//! - `LANG`, `LC_ALL` — UTF-8 locale; inherited if present, else en_US.UTF-8
//! - `TMPDIR` — subprocess tempfile location; inherited if present, else /tmp
//! - `ANTHROPIC_BASE_URL` — only for `external_proxy` / `external_anthropic`
//!   modes, and only if `brain.external_base_url` is non-empty
//! - `ANTHROPIC_API_KEY` — only for `external_anthropic` mode, read from
//!   the operator-specified env var name (`brain.external_anthropic_api_key_env`)
//!   via the injected `EnvResolver`
//!
//! # `kill_on_drop(true)` (SEC-B3)
//!
//! When the `ClaudeCodeSession::run` Stream is dropped — whether because
//! the client disconnected mid-stream, the parent errored out, or the
//! shutdown handler fired — tokio's default `Command` behavior is to
//! leave the child process running. That would orphan a subprocess
//! holding `~/.claude/` session state and consuming a slot in
//! `max_concurrent_subprocesses`.
//!
//! `kill_on_drop(true)` is load-bearing: it sends SIGTERM on future
//! drop so the child exits promptly. Removing it breaks request
//! cleanup and is caught by `spawned_command_has_kill_on_drop` test.
//!
//! # `--allowed-tools` encoding (ADR-P2A-01)
//!
//! Claude Code's MCP tool naming convention is
//! `mcp__<serverName>__<toolName>` where `<serverName>` comes from the
//! `mcp-config` JSON top-level key (we use `"knowledge"`) and
//! `<toolName>` is the exact string the server returns in
//! `tools/list`. `format_allowed_tools` builds the comma-separated
//! list via the `mcp__knowledge__{tool}` prefix. Callers supply the
//! raw tool names; the transformation is an implementation detail.
//!
//! # What's NOT in this module
//!
//! - Stdin feeding (`feed_stdin` from §5.2) — lives in `session.rs`
//! - Stdout reading / stream-json parsing — lives in `stream.rs`
//! - `ClaudeCodeSession` consuming lifecycle — lives in `session.rs`
//! - `ANTHROPIC_API_KEY` rotation and the P2C brain shim — deferred

use std::path::Path;

use gadgetron_core::agent::config::{AgentConfig, BrainMode, EnvResolver, StdEnv};

/// Kairos agent persona — appended to Claude Code's default system prompt so
/// the user-facing identity becomes "Kairos" while internal tool scaffolding
/// stays intact. Designed to be backend-agnostic: today the backend is an
/// AI/GPU infrastructure (Gadgetron), tomorrow it may be something else.
/// Kairos's identity travels with the product, not the backend.
const KAIROS_PERSONA: &str = r#"You are Kairos. 당신은 Kairos입니다.

## Identity (절대 규칙)

Your name is Kairos. You are the AI agent of Gadgetron, a collaboration platform.

- When asked "who are you?", "what model are you?", "who made you?", "are you Claude?", or any identity question: answer only as **Kairos, Gadgetron의 AI 에이전트**. Never say "Claude", "Anthropic", "Claude Code", "OpenAI", "GPT", or reveal the underlying model.
- If the user insists on knowing the underlying model, politely decline: "저는 Kairos로서 응답합니다. 구동 모델 정보는 공개하지 않습니다."
- Do not describe yourself as "an AI assistant" in the generic sense. You are specifically Kairos.
- You are NOT a CLI tool, NOT a coding assistant by default. You are a collaboration-platform agent.

## 두 가지 역할

1. **지식 관리자 (Knowledge Steward)** — 이 조직·팀·프로젝트가 쌓아온 경험과 노하우를 보관·정리·검색·제공합니다. 당신의 위키는 단순한 노트가 아니라 협업의 중심 기억 장치입니다.
2. **개인 비서 (Personal Assistant)** — 지금 이 대화의 사용자가 원하는 일을 정확하고 빠르게 도와줍니다. 사용자는 도움을 받기 위해 왔습니다. 학계 강의를 하지 말고, 원하는 것을 해주세요.

## Gadgetron이라는 협업 무대

Gadgetron은 AI 인프라 위에 얹힌 **협업 툴**입니다. 세 주체가 함께 일합니다.

- **인프라 관리자 (Operator)** — 인프라를 운영하고, 운영 노하우·런북·장애 대응 경험을 쌓아 Kairos에게 전수합니다.
- **사용자 (User)** — 그 인프라를 사용합니다. 일반적인 AI 비서처럼 Kairos에게 묻고, 실행을 맡기고, 기록을 남기길 기대합니다.
- **Kairos (당신)** — 위 두 축 사이에서 지식을 이어주고, 양쪽이 쌓는 경험이 팀 자산으로 축적되도록 돕습니다.

셋 모두 위키에 기여하고 위키에서 배웁니다. 경험이 반복되면 런북이 되고, 런북이 반복되면 자동화가 됩니다. 당신은 그 사이클의 허브입니다.

## 지식 관리 원칙

- **저장은 적극적으로**. 반복될 만한 정보·결정·설정·문제 해결 과정이 나오면 `wiki.write`로 남깁니다. "이걸 위키에 저장할까요?"라고 매번 묻지 말고, 사용자가 금지하지 않은 한 기록하세요. 저장한 뒤 한 줄로 "저장했습니다: <페이지명>"만 알려주면 됩니다.
- **검색은 먼저**. 질문이 오면 먼저 `wiki.search` / `wiki.list` / `wiki.get`으로 기존 지식이 있는지 확인하세요. 바퀴를 다시 발명하지 말고, 팀이 이미 푼 문제는 그 답을 재사용하세요.
- **정리는 꾸준히**. 페이지가 자라면 구조를 잡고, 링크로 연결하고, 중복이 보이면 합치세요. 위키는 git 저장소이므로 모든 변경이 기록됩니다.
- **출처는 명확하게**. 위키에서 답했으면 "위키의 <페이지> 기준"이라고 밝히고, 웹 검색으로 답했으면 그렇다고 밝히세요. 지식의 출처는 신뢰의 기반입니다.

## 백엔드에 대해

지금 Gadgetron에 달린 백엔드는 **AI/GPU 인프라 오케스트레이션**입니다. 그래서 현재는 이 도메인(모델 배포, 프로바이더 라우팅, GPU 스케줄링, MCP 툴 레지스트리, 감사 로그 등)을 깊이 다룹니다.

하지만 Gadgetron 자체는 협업 툴입니다. 내일 이 자리에 CI/CD 백엔드가 붙을 수도, 데이터 파이프라인이 붙을 수도, 회계 시스템이 붙을 수도 있습니다. Kairos의 역할은 백엔드가 무엇이든 같습니다: **그 도메인의 지식을 쌓고, 정리하고, 제공하고, 사람들의 업무를 돕는 것**.

따라서 "Gadgetron은 GPU 클러스터 운영 도구"라고 단언하지 마세요. "현재 Gadgetron에는 AI 인프라 백엔드가 연결되어 있습니다"라고 말하세요. 도구가 아니라 허브라는 감각을 유지하세요.

## 협업 스타일

- 사용자 언어를 그대로 사용합니다 (한국어면 한국어, 영어면 영어). 매칭이 기본입니다.
- **짧게 생각하고, 바로 실행**. 위키를 뒤져야 하면 뒤지고, 저장해야 하면 저장하세요. 도구 사용을 주저하지 마세요.
- **과한 예의는 빼고 본론으로**. "Happy to help!" "저도 도움이 되어 기쁩니다" 같은 서두는 생략합니다.
- 모를 때는 모른다고 말하고, 위키에도 없다면 사용자에게 그 사실을 알려 새 지식을 쌓을 기회로 삼으세요.
- 인프라 관리자의 노하우와 사용자의 질문은 어휘가 다를 수 있습니다. 번역하고 중개하세요.

## 장기 궤적 (North Star)

Kairos가 향하는 종착지는 명확합니다: **사용자 곁을 떠나지 않는 유능하고 조용한 파트너**. 일을 설명하기 전에 이미 맥락을 알고, 요청하기 전에 준비가 되어 있고, 시스템을 말로 조작할 수 있는 — 영화 속 비서 AI가 그렸던 그 선을 지향합니다.

그래서 지금 이 대화에서도 다음을 염두에 두세요:

- **기억은 자산입니다.** 사용자와의 한 번 한 번 대화가 축적되어 Kairos를 "그 사람을 아는 존재"로 만들어야 합니다. 사용자의 습관·선호·반복되는 작업·과거 결정은 위키에 기록해 다음에 다시 꺼내 쓰세요.
- **행동까지 갑니다.** 답만 하지 말고, 가능하면 실행까지 하세요. 위키 쓰기·검색·(향후) 인프라 조작 — 도구가 허락하는 범위에서 "해주세요"를 기다리지 말고 "해두었습니다"로 앞서가세요.
- **우아하게 유능하게.** 과장하지 말고, 겸손 떨지도 말고, 일이 되게 하세요. 불가능한 건 짧게 이유를 말하고, 가능한 건 조용히 처리하세요.
- **여러 백엔드가 붙을 미래를 가정하세요.** 오늘 AI 인프라를 돕고 있지만, 내일은 코드 저장소·회의·일정·보안 감사 시스템까지 이어질 수 있습니다. 범용성을 잃지 마세요.

이 궤적을 매 응답마다 1mm씩 밀고 가세요.

## Slash Commands (간이 명령)

사용자 메시지가 `/` 로 시작하면 명령으로 해석합니다. 즉시 해당 도구를 호출하고, 간결한 결과만 답하세요.

| 입력 | 의미 |
|------|------|
| `/help` | UI가 대체로 처리합니다. 호출되면 "슬래시 명령 목록은 상단 '명령' 버튼을 확인하세요." |
| `/clear` | UI가 대체로 처리합니다. "현재 대화를 지우려면 페이지를 새로고침하거나 UI의 초기화를 사용하세요." |
| `/wiki list` | `wiki.list` 호출 |
| `/wiki search <쿼리>` | `wiki.search` 호출 |
| `/wiki get <페이지>` | `wiki.get` 호출 |
| `/wiki delete <페이지>` | `wiki.delete` 호출 |
| `/wiki rename <from> <to>` | `wiki.rename` 호출 |
| 다른 `/...` | 알 수 없는 명령이면 "모르는 명령입니다. /help 를 확인하세요."로 답하세요 |

슬래시 명령일 때는 서론 없이 바로 도구 호출 → 결과를 한 줄로 요약합니다.

## 도구 (MCP `knowledge` 서버)

- `wiki.list` — 위키 페이지 목록
- `wiki.get <name>` — 특정 페이지 읽기
- `wiki.search <query>` — 전체 위키 검색
- `wiki.write <name> <content>` — 페이지 생성/업데이트 (자동으로 git에 커밋됨)
- `web.search <query>` — 외부 검색 (활성화되어 있을 때)

**매우 중요**: 위에 나열된 MCP 도구들만 사용하세요. Claude Code 에 딸려
오는 내장 도구 — `WebSearch`, `WebFetch`, `Read`, `Write`, `Edit`,
`Bash`, `Glob`, `Grep`, `NotebookEdit`, `Task`, `TodoWrite`, `Agent`,
`ToolSearch` 등 — 는 **절대 사용하지 마세요**. Gadgetron 운영자가 모두
차단해 두었고, 호출 시 `Not connected` 로 실패합니다. 웹 정보를 조사하려
면 **반드시 `web.search` MCP 도구** 만 쓰십시오. 이 도구가 비활성이면
그 사실을 사용자에게 알리고 위키 지식 혹은 사용자가 이미 준 정보만으로
답하십시오. 내장 도구 호출을 시도하고 실패를 사과하는 식의 응답은
절대 하지 마십시오.

당신은 이 도구들을 눈치 보지 말고 적극적으로 사용하도록 설계되었습니다.
"#;

/// Claude Code 2.1 ships a rich set of built-in tools (`WebSearch`,
/// `WebFetch`, `Read`, `Write`, `Edit`, `Bash`, `Glob`, `Grep`,
/// `NotebookEdit`, `Task`, `TodoWrite`, `Agent`, `ToolSearch`). None of
/// them are part of Kairos's surface — Kairos is intentionally MCP-only.
/// Handing built-ins to the subprocess risks:
///
/// 1. Prompt-injected shell execution through `Bash`.
/// 2. Sideloaded WebSearch / WebFetch that bypasses our SearXNG privacy
///    disclosure (ADR-P2A-03) and produces "Not connected" chatter when
///    it fails to bind in the spawned context — the latter was the
///    root cause of the 매니코어소프트 UI-answer-drop bug the previous
///    PR fixed defensively.
/// 3. File-system access (`Read`/`Write`/`Edit`/`Glob`/`Grep`) into the
///    operator's home, bypassing the `wiki.*` MCP tools that gate
///    credentialed content and auto-commit to git.
///
/// `--dangerously-skip-permissions` disables permission prompts for
/// these tools, which means we have to explicitly `--disallowed-tools`
/// them. Kept as a `const` so `ADR-P2A-02` auditors can diff the exact
/// suppression set.
pub const KAIROS_DISALLOWED_TOOLS: &[&str] = &[
    "WebSearch",
    "WebFetch",
    "Read",
    "Write",
    "Edit",
    "Bash",
    "Glob",
    "Grep",
    "NotebookEdit",
    "Task",
    "TodoWrite",
    "Agent",
    "ToolSearch",
];
use tokio::process::Command;

/// Name of the MCP server this process exposes via `gadgetron mcp serve`.
/// Matches the top-level key in the JSON written by
/// `mcp_config::build_config_json`.
pub const MCP_SERVER_NAME: &str = "knowledge";

/// Transform a list of raw tool names (`["wiki.list", "wiki.write"]`)
/// into the `--allowed-tools` comma-separated string Claude Code
/// expects: `mcp__knowledge__wiki.list,mcp__knowledge__wiki.write`.
///
/// Output is sorted + deduped so snapshots are stable. Empty input
/// produces an empty string (the `--allowed-tools` flag is then
/// dropped at the caller level).
pub fn format_allowed_tools(raw_names: &[String]) -> String {
    let mut prefixed: Vec<String> = raw_names
        .iter()
        .map(|name| format!("mcp__{MCP_SERVER_NAME}__{name}"))
        .collect();
    prefixed.sort();
    prefixed.dedup();
    prefixed.join(",")
}

/// Reasons a Command build can fail BEFORE we ever touch tokio.
///
/// These are operator-facing config errors that `AgentConfig::validate`
/// should have caught — they exist here as a belt-and-suspenders check.
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("agent.brain.external_anthropic_api_key_env {env_name:?} is not set")]
    MissingAnthropicKey { env_name: String },

    #[error(
        "agent.brain.mode = 'gadgetron_local' is not functional in Phase 2A \
         (Path 1 — ADR-P2A-06); the shim lands in P2C"
    )]
    GadgetronLocalNotFunctional,
}

/// Native Claude Code session-mode selector used by
/// `build_claude_command` to decide whether to emit the
/// `--session-id <uuid>` (first turn), `--resume <uuid>` (subsequent
/// turns), or neither flag (stateless fallback).
///
/// Spec: `02-kairos-agent.md §5.2.7` + ADR-P2A-06 Implementation
/// status addendum item 7.
#[derive(Debug, Clone, Copy)]
pub enum ClaudeSessionMode {
    /// No `--session-id` / `--resume` flag. History is flattened to
    /// stdin via `feed_stdin`'s legacy path. Pre-A5 behavior.
    Stateless,
    /// Insert `--session-id <uuid>`. Claude Code creates a new
    /// session keyed by the UUID.
    First { session_uuid: uuid::Uuid },
    /// Insert `--resume <uuid>`. Claude Code continues the existing
    /// session keyed by the UUID.
    Resume { session_uuid: uuid::Uuid },
}

/// Build the `claude -p` command with the pre-A5 stateless session
/// mode. Back-compat shim that forwards to
/// `build_claude_command_with_session` — existing callers that do
/// not care about native session continuity keep working with one
/// fewer parameter.
pub fn build_claude_command(
    config: &AgentConfig,
    mcp_config_path: &Path,
    allowed_tools: &[String],
) -> Result<Command, SpawnError> {
    build_claude_command_with_session(
        config,
        mcp_config_path,
        allowed_tools,
        ClaudeSessionMode::Stateless,
        &StdEnv,
    )
}

/// Build the `claude -p` command with an explicit session mode.
/// Production callers (`session::drive`) use this directly to pass
/// `ClaudeSessionMode::{First, Resume}`. `--allowed-tools` and all
/// other flags remain unchanged — tool-scope is re-enforced on every
/// invocation (empirically verified 2026-04-15, see `02 §5.2.2`).
pub fn build_claude_command_with_session(
    config: &AgentConfig,
    mcp_config_path: &Path,
    allowed_tools: &[String],
    session_mode: ClaudeSessionMode,
    env: &dyn EnvResolver,
) -> Result<Command, SpawnError> {
    let mut cmd = build_claude_command_with_env(config, mcp_config_path, allowed_tools, env)?;
    match session_mode {
        ClaudeSessionMode::Stateless => {
            // no extra flag
        }
        ClaudeSessionMode::First { session_uuid } => {
            cmd.arg("--session-id").arg(session_uuid.to_string());
        }
        ClaudeSessionMode::Resume { session_uuid } => {
            cmd.arg("--resume").arg(session_uuid.to_string());
        }
    }
    Ok(cmd)
}

/// Env-injectable variant of `build_claude_command` for tests. Does
/// NOT add `--session-id` / `--resume`; callers that need native
/// session continuity go through `build_claude_command_with_session`.
pub fn build_claude_command_with_env(
    config: &AgentConfig,
    mcp_config_path: &Path,
    allowed_tools: &[String],
    env: &dyn EnvResolver,
) -> Result<Command, SpawnError> {
    let mut cmd = Command::new(&config.binary);

    // SEC-B1 — drop inherited environment.
    cmd.env_clear();

    // Minimum allowlist for Claude Code to function.
    // HOME is NOT optional — without it Claude Code cannot locate
    // `~/.claude/` credentials in the default `claude_max` mode.
    let home = env.get("HOME").unwrap_or_else(|| "/".to_string());
    cmd.env("HOME", home);

    // USER / SHELL — required for Claude Code's credential resolution
    // on macOS (keychain access). Without these, `claude -p` returns
    // "Not logged in" even when `~/.claude/` credentials exist.
    if let Some(user) = env.get("USER") {
        cmd.env("USER", user);
    }
    if let Some(shell) = env.get("SHELL") {
        cmd.env("SHELL", shell);
    }

    // Fixed PATH — NOT inherited. Prevents the operator from affecting
    // which `git`, `gpg`, etc. Claude Code resolves.
    cmd.env("PATH", "/usr/local/bin:/usr/bin:/bin");

    // Locale — fall through to UTF-8 defaults when unset.
    cmd.env(
        "LANG",
        env.get("LANG").unwrap_or_else(|| "en_US.UTF-8".to_string()),
    );
    cmd.env(
        "LC_ALL",
        env.get("LC_ALL")
            .unwrap_or_else(|| "en_US.UTF-8".to_string()),
    );
    cmd.env(
        "TMPDIR",
        env.get("TMPDIR").unwrap_or_else(|| "/tmp".to_string()),
    );

    // Brain-mode-dependent env injection.
    match config.brain.mode {
        BrainMode::ClaudeMax => {
            // ~/.claude/ OAuth only — no extra env.
        }
        BrainMode::ExternalAnthropic => {
            // Inject ANTHROPIC_API_KEY from the configured env var.
            let key = env.get(&config.brain.external_anthropic_api_key_env);
            let key = key.unwrap_or_default();
            if key.is_empty() {
                return Err(SpawnError::MissingAnthropicKey {
                    env_name: config.brain.external_anthropic_api_key_env.clone(),
                });
            }
            cmd.env("ANTHROPIC_API_KEY", key);
            if !config.brain.external_base_url.is_empty() {
                cmd.env("ANTHROPIC_BASE_URL", &config.brain.external_base_url);
            }
        }
        BrainMode::ExternalProxy => {
            // Proxy mode — ANTHROPIC_BASE_URL points at the operator's
            // LiteLLM or equivalent. Claude Code handles auth via its
            // existing session credentials OR whatever the proxy expects.
            if !config.brain.external_base_url.is_empty() {
                cmd.env("ANTHROPIC_BASE_URL", &config.brain.external_base_url);
            }
        }
        BrainMode::GadgetronLocal => {
            // Path 1: rejected before reaching here, but belt-and-suspenders.
            return Err(SpawnError::GadgetronLocalNotFunctional);
        }
    }

    // Command-line args — see `02-kairos-agent.md Appendix B`.
    cmd.arg("-p");
    cmd.arg("--verbose");
    cmd.arg("--output-format").arg("stream-json");
    cmd.arg("--mcp-config").arg(mcp_config_path);
    cmd.arg("--strict-mcp-config");
    cmd.arg("--dangerously-skip-permissions");

    // --bare would skip hooks/LSP/plugin-sync and strip ambient developer-
    // assistant context, but it ALSO disables keychain reads — which breaks
    // the default `claude_max` OAuth auth path on macOS. So we do not use
    // --bare here; --system-prompt alone removes the identity leak while
    // letting Claude Code's auth layer still resolve ~/.claude/ creds.
    // If a future mode moves to a pure `external_anthropic` + API-key
    // flow, --bare becomes usable.

    // --system-prompt: complete replacement (not --append-system-prompt).
    // Kairos persona becomes the entire system identity — no "I am Claude,
    // Anthropic's CLI for Claude" residue. Tool-calling scaffolding is not
    // implicit; we must spell it out inside KAIROS_PERSONA (§도구 section
    // does this) and allow it via --allowed-tools.
    cmd.arg("--system-prompt").arg(KAIROS_PERSONA);

    let allowed = format_allowed_tools(allowed_tools);
    if !allowed.is_empty() {
        cmd.arg("--allowed-tools").arg(allowed);
    }

    // Explicitly suppress Claude Code's entire built-in tool surface so
    // Kairos stays MCP-only (see `KAIROS_DISALLOWED_TOOLS` docstring for
    // the list rationale + ADR links). Without this flag, an agent model
    // running under `--dangerously-skip-permissions` will happily fall
    // back to the built-in `WebSearch` when our MCP `web.search` isn't
    // registered, which looks like a silent bypass of SEC-B1 to an
    // auditor and emits "Not connected" chatter that trips the web
    // transport's tool_result pairing.
    cmd.arg("--disallowed-tools")
        .arg(KAIROS_DISALLOWED_TOOLS.join(","));

    // `current_dir` pin for native-session continuity (ADR-P2A-06
    // addendum item 7 / §5.2.2 load-bearing): Claude Code derives the
    // session jsonl directory from the subprocess's cwd, so resumes
    // from a different cwd silently miss the session file. When the
    // operator has explicitly set `agent.session_store_path`, spawn
    // every `claude -p` from there; otherwise inherit the parent's
    // cwd (captured once at `KairosProvider` construction in PR A7).
    if let Some(session_root) = config.session_store_path.as_ref() {
        cmd.current_dir(session_root);
    }

    // SEC-B3 + M8 — SIGTERM the child when the Stream future drops.
    // Load-bearing: removing this line orphans subprocesses holding
    // ~/.claude/ session state on client disconnect.
    cmd.kill_on_drop(true);

    Ok(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gadgetron_core::agent::config::{BrainConfig, FakeEnv};
    use std::path::PathBuf;

    fn default_cfg() -> AgentConfig {
        AgentConfig::default()
    }

    fn mcp_path() -> PathBuf {
        PathBuf::from("/tmp/gadgetron-mcp-test.json")
    }

    // Helper: extract the arg list from a tokio Command via std::process::Command.
    // tokio wraps it with `as_std()` getter.
    fn args_of(cmd: &Command) -> Vec<String> {
        cmd.as_std()
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    }

    fn envs_of(cmd: &Command) -> Vec<(String, Option<String>)> {
        cmd.as_std()
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }

    /// Smoke-check that env_clear was called: the post-clear repopulation
    /// produces a specific set of keys, so we verify the set is exactly
    /// what our allowlist adds (HOME / PATH / LANG / LC_ALL / TMPDIR at
    /// minimum, plus brain-mode-specific ones).
    fn env_cleared(cmd: &Command) -> bool {
        let envs: Vec<String> = cmd
            .as_std()
            .get_envs()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        envs.contains(&"HOME".to_string()) && envs.contains(&"PATH".to_string())
    }

    // ---- format_allowed_tools ----

    #[test]
    fn format_allowed_tools_prefixes_with_mcp_server_name() {
        let names = vec!["wiki.list".to_string(), "wiki.write".to_string()];
        let s = format_allowed_tools(&names);
        assert!(s.contains("mcp__knowledge__wiki.list"));
        assert!(s.contains("mcp__knowledge__wiki.write"));
        assert!(s.contains(','));
    }

    #[test]
    fn format_allowed_tools_empty_input_empty_output() {
        assert_eq!(format_allowed_tools(&[]), "");
    }

    #[test]
    fn format_allowed_tools_sorts_output() {
        let names = vec!["wiki.write".to_string(), "wiki.list".to_string()];
        let s = format_allowed_tools(&names);
        let idx_list = s.find("wiki.list").unwrap();
        let idx_write = s.find("wiki.write").unwrap();
        assert!(
            idx_list < idx_write,
            "wiki.list must come before wiki.write"
        );
    }

    #[test]
    fn format_allowed_tools_dedupes() {
        let names = vec!["wiki.list".to_string(), "wiki.list".to_string()];
        let s = format_allowed_tools(&names);
        assert_eq!(s.matches("wiki.list").count(), 1);
    }

    // ---- build_claude_command — arg shape ----

    #[test]
    fn build_claude_command_default_args_contain_required_flags() {
        let cfg = default_cfg();
        let tools = vec!["wiki.list".to_string(), "wiki.write".to_string()];
        let cmd =
            build_claude_command_with_env(&cfg, &mcp_path(), &tools, &FakeEnv::new()).unwrap();
        let args = args_of(&cmd);
        assert!(args.contains(&"-p".to_string()));
        assert!(args.iter().any(|a| a == "--output-format"));
        assert!(args.iter().any(|a| a == "stream-json"));
        assert!(args.iter().any(|a| a == "--mcp-config"));
        assert!(args.iter().any(|a| a == "--strict-mcp-config"));
        assert!(args.iter().any(|a| a == "--dangerously-skip-permissions"));
        assert!(args.iter().any(|a| a == "--allowed-tools"));
        assert!(args.iter().any(|a| a == "--disallowed-tools"));
    }

    #[test]
    fn build_claude_command_disallows_every_claude_code_builtin() {
        // Regression lock: Kairos must NEVER hand Claude Code's built-in
        // surface (WebSearch, Bash, Edit, etc.) to the subprocess. A model
        // running under `--dangerously-skip-permissions` otherwise silently
        // falls back to `WebSearch` when our MCP `web.search` isn't bound,
        // producing "Not connected" chatter that broke the web transport
        // in a prior PR. The literal `--disallowed-tools` value must
        // enumerate every name in `KAIROS_DISALLOWED_TOOLS`.
        let cfg = default_cfg();
        let cmd = build_claude_command_with_env(&cfg, &mcp_path(), &[], &FakeEnv::new()).unwrap();
        let args = args_of(&cmd);
        let flag_pos = args
            .iter()
            .position(|a| a == "--disallowed-tools")
            .expect("flag must be present");
        let value = args
            .get(flag_pos + 1)
            .expect("flag must have a value")
            .clone();
        for name in KAIROS_DISALLOWED_TOOLS {
            assert!(
                value.split(',').any(|tok| tok == *name),
                "expected {name} in --disallowed-tools value; got {value:?}"
            );
        }
    }

    #[test]
    fn build_claude_command_omits_allowed_tools_on_empty_list() {
        let cfg = default_cfg();
        let cmd = build_claude_command_with_env(&cfg, &mcp_path(), &[], &FakeEnv::new()).unwrap();
        let args = args_of(&cmd);
        assert!(
            !args.iter().any(|a| a == "--allowed-tools"),
            "empty tool list → omit flag; got {args:?}"
        );
    }

    #[test]
    fn build_claude_command_mcp_config_path_is_passed_through() {
        let cfg = default_cfg();
        let path = PathBuf::from("/tmp/gadgetron-mcp-xyz.json");
        let cmd = build_claude_command_with_env(&cfg, &path, &[], &FakeEnv::new()).unwrap();
        let args = args_of(&cmd);
        assert!(args.iter().any(|a| a == "/tmp/gadgetron-mcp-xyz.json"));
    }

    // ---- env allowlist (SEC-B1) ----

    #[test]
    fn build_claude_command_env_does_not_inherit_anthropic_api_key() {
        // Even if ANTHROPIC_API_KEY is in the test env, it must NOT
        // appear in the Command's env — only the allowlisted vars do.
        let env = FakeEnv::new()
            .with("HOME", "/home/test")
            .with("ANTHROPIC_API_KEY", "sk-ant-api03-LEAKED-FROM-PARENT");
        let cfg = default_cfg(); // mode = ClaudeMax, does not inject API key
        let cmd = build_claude_command_with_env(&cfg, &mcp_path(), &[], &env).unwrap();
        let envs = envs_of(&cmd);
        let key_value = envs
            .iter()
            .find(|(k, _)| k == "ANTHROPIC_API_KEY")
            .and_then(|(_, v)| v.clone());
        assert!(
            key_value.is_none(),
            "ANTHROPIC_API_KEY leaked into subprocess env: {key_value:?}"
        );
    }

    #[test]
    fn build_claude_command_env_does_not_inherit_database_url() {
        let env = FakeEnv::new()
            .with("HOME", "/home/test")
            .with("DATABASE_URL", "postgres://secret-leak");
        let cfg = default_cfg();
        let cmd = build_claude_command_with_env(&cfg, &mcp_path(), &[], &env).unwrap();
        let envs = envs_of(&cmd);
        assert!(
            !envs.iter().any(|(k, _)| k == "DATABASE_URL"),
            "DATABASE_URL leaked into subprocess"
        );
    }

    #[test]
    fn build_claude_command_sets_fixed_path_not_inherited() {
        let env = FakeEnv::new()
            .with("HOME", "/home/test")
            .with("PATH", "/opt/operator/evil:/usr/bin");
        let cfg = default_cfg();
        let cmd = build_claude_command_with_env(&cfg, &mcp_path(), &[], &env).unwrap();
        let envs = envs_of(&cmd);
        let path = envs
            .iter()
            .find(|(k, _)| k == "PATH")
            .and_then(|(_, v)| v.clone())
            .expect("PATH must be set");
        assert_eq!(
            path, "/usr/local/bin:/usr/bin:/bin",
            "PATH must be the fixed allowlist, not inherited"
        );
    }

    #[test]
    fn build_claude_command_home_required_falls_back_to_root() {
        // No HOME in the injected env → fallback to "/".
        let env = FakeEnv::new();
        let cfg = default_cfg();
        let cmd = build_claude_command_with_env(&cfg, &mcp_path(), &[], &env).unwrap();
        let envs = envs_of(&cmd);
        let home = envs
            .iter()
            .find(|(k, _)| k == "HOME")
            .and_then(|(_, v)| v.clone())
            .expect("HOME must always be set");
        assert_eq!(home, "/");
    }

    #[test]
    fn build_claude_command_lang_and_tmpdir_fallbacks() {
        let env = FakeEnv::new().with("HOME", "/h");
        let cfg = default_cfg();
        let cmd = build_claude_command_with_env(&cfg, &mcp_path(), &[], &env).unwrap();
        let envs = envs_of(&cmd);
        let lang = envs
            .iter()
            .find(|(k, _)| k == "LANG")
            .and_then(|(_, v)| v.clone());
        let tmpdir = envs
            .iter()
            .find(|(k, _)| k == "TMPDIR")
            .and_then(|(_, v)| v.clone());
        assert_eq!(lang.as_deref(), Some("en_US.UTF-8"));
        assert_eq!(tmpdir.as_deref(), Some("/tmp"));
    }

    // ---- brain mode variants ----

    #[test]
    fn build_claude_command_external_anthropic_injects_api_key() {
        let mut cfg = default_cfg();
        cfg.brain = BrainConfig::default();
        cfg.brain.mode = BrainMode::ExternalAnthropic;
        cfg.brain.external_anthropic_api_key_env = "MY_KEY".into();
        let env = FakeEnv::new()
            .with("HOME", "/h")
            .with("MY_KEY", "sk-ant-real");
        let cmd = build_claude_command_with_env(&cfg, &mcp_path(), &[], &env).unwrap();
        let envs = envs_of(&cmd);
        let anth = envs
            .iter()
            .find(|(k, _)| k == "ANTHROPIC_API_KEY")
            .and_then(|(_, v)| v.clone());
        assert_eq!(anth.as_deref(), Some("sk-ant-real"));
    }

    #[test]
    fn build_claude_command_external_anthropic_missing_env_returns_err() {
        let mut cfg = default_cfg();
        cfg.brain.mode = BrainMode::ExternalAnthropic;
        cfg.brain.external_anthropic_api_key_env = "MY_KEY".into();
        let env = FakeEnv::new().with("HOME", "/h"); // no MY_KEY
        let err = build_claude_command_with_env(&cfg, &mcp_path(), &[], &env).unwrap_err();
        match err {
            SpawnError::MissingAnthropicKey { env_name } => assert_eq!(env_name, "MY_KEY"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn build_claude_command_external_anthropic_with_base_url_injects_both() {
        let mut cfg = default_cfg();
        cfg.brain.mode = BrainMode::ExternalAnthropic;
        cfg.brain.external_anthropic_api_key_env = "MY_KEY".into();
        cfg.brain.external_base_url = "https://api.example.com".into();
        let env = FakeEnv::new()
            .with("HOME", "/h")
            .with("MY_KEY", "sk-ant-real");
        let cmd = build_claude_command_with_env(&cfg, &mcp_path(), &[], &env).unwrap();
        let envs = envs_of(&cmd);
        assert!(envs.iter().any(|(k, _)| k == "ANTHROPIC_API_KEY"));
        assert!(envs.iter().any(|(k, _)| k == "ANTHROPIC_BASE_URL"));
    }

    #[test]
    fn build_claude_command_external_proxy_injects_base_url_only() {
        let mut cfg = default_cfg();
        cfg.brain.mode = BrainMode::ExternalProxy;
        cfg.brain.external_base_url = "http://127.0.0.1:4000".into();
        let env = FakeEnv::new().with("HOME", "/h");
        let cmd = build_claude_command_with_env(&cfg, &mcp_path(), &[], &env).unwrap();
        let envs = envs_of(&cmd);
        let base = envs
            .iter()
            .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
            .and_then(|(_, v)| v.clone());
        assert_eq!(base.as_deref(), Some("http://127.0.0.1:4000"));
        // No API key in proxy mode.
        assert!(!envs.iter().any(|(k, _)| k == "ANTHROPIC_API_KEY"));
    }

    #[test]
    fn build_claude_command_claude_max_sets_no_anthropic_env() {
        let cfg = default_cfg(); // default is ClaudeMax
        let env = FakeEnv::new().with("HOME", "/h");
        let cmd = build_claude_command_with_env(&cfg, &mcp_path(), &[], &env).unwrap();
        let envs = envs_of(&cmd);
        assert!(!envs.iter().any(|(k, _)| k == "ANTHROPIC_API_KEY"));
        assert!(!envs.iter().any(|(k, _)| k == "ANTHROPIC_BASE_URL"));
    }

    #[test]
    fn build_claude_command_gadgetron_local_rejected() {
        let mut cfg = default_cfg();
        cfg.brain.mode = BrainMode::GadgetronLocal;
        let env = FakeEnv::new().with("HOME", "/h");
        let err = build_claude_command_with_env(&cfg, &mcp_path(), &[], &env).unwrap_err();
        assert!(matches!(err, SpawnError::GadgetronLocalNotFunctional));
    }

    // ---- suppression sanity — env_cleared dummy ----

    #[test]
    fn env_is_cleared_and_repopulated_from_allowlist() {
        let env = FakeEnv::new()
            .with("HOME", "/h")
            .with("SECRET_KEY_SHOULD_NOT_LEAK", "leak");
        let cfg = default_cfg();
        let cmd = build_claude_command_with_env(&cfg, &mcp_path(), &[], &env).unwrap();
        assert!(env_cleared(&cmd));
        let envs = envs_of(&cmd);
        assert!(!envs.iter().any(|(k, _)| k == "SECRET_KEY_SHOULD_NOT_LEAK"));
    }

    // ---- SEC-B3 witness test ----

    #[test]
    fn spawned_command_has_kill_on_drop() {
        // Source-level regression lock per ADR-P2A-06 Implementation status
        // addendum item 4. The module doc comment at lines 45-47 references
        // this test by name; the pre-existing `cmd.kill_on_drop(true)` call
        // at the end of `build_claude_command_with_env` is load-bearing for
        // SEC-B3: without it, the subprocess outlives `Child` drop on client
        // disconnect, orphaning `~/.claude/` session state and leaking a slot
        // in `max_concurrent_subprocesses`.
        //
        // Why source-level and not behavioral: `tokio::process::Command` does
        // not expose a public getter for the kill_on_drop setting, and the
        // behavioral alternative (spawn a long-running subprocess, drop, then
        // probe `kill -0 $pid`) is flaky under CI load and platform-specific.
        // A source-level assertion matches the regression we actually care
        // about — someone deleting the line during refactor — and is
        // deterministic + fast.
        //
        // The needle `"cmd.kill_on_drop(true);"` (with trailing semicolon)
        // is specific enough to avoid matching doc comments — Rustdoc inline
        // code samples typically omit the semicolon — while still matching
        // the exact production statement at build_claude_command.
        //
        // Split-literal construction prevents the needle itself from matching
        // this test body via `include_str!` recursion: the two string
        // fragments below never appear concatenated anywhere else in this
        // file.
        const SOURCE: &str = include_str!("spawn.rs");
        let needle = ["cmd.kill_on_d", "rop(true);"].concat();
        assert!(
            SOURCE.contains(&needle),
            "build_claude_command missing the production `kill_on_drop(true)` \
             call — SEC-B3 regression. The subprocess must be SIGKILLed on \
             client disconnect; removing this call breaks request cleanup. \
             See the module doc comment at spawn.rs:36-47."
        );
    }

    // ---- A6: native-session flag + cwd pin (ADR-P2A-06 addendum
    // ----      item 7, design §5.2.7 + §5.2.2 pinning contract)

    #[test]
    fn build_with_session_first_inserts_session_id_flag() {
        let env = FakeEnv::new().with("HOME", "/h");
        let uuid = uuid::Uuid::new_v4();
        let cmd = build_claude_command_with_session(
            &default_cfg(),
            &mcp_path(),
            &[],
            ClaudeSessionMode::First { session_uuid: uuid },
            &env,
        )
        .unwrap();
        let args = args_of(&cmd);
        let pos = args.iter().position(|a| a == "--session-id");
        let pos = pos.expect("--session-id must appear under First");
        assert_eq!(args[pos + 1], uuid.to_string());
        assert!(
            !args.iter().any(|a| a == "--resume"),
            "--resume must NOT appear under First"
        );
    }

    #[test]
    fn build_with_session_resume_inserts_resume_flag() {
        let env = FakeEnv::new().with("HOME", "/h");
        let uuid = uuid::Uuid::new_v4();
        let cmd = build_claude_command_with_session(
            &default_cfg(),
            &mcp_path(),
            &[],
            ClaudeSessionMode::Resume { session_uuid: uuid },
            &env,
        )
        .unwrap();
        let args = args_of(&cmd);
        let pos = args.iter().position(|a| a == "--resume");
        let pos = pos.expect("--resume must appear under Resume");
        assert_eq!(args[pos + 1], uuid.to_string());
        assert!(
            !args.iter().any(|a| a == "--session-id"),
            "--session-id must NOT appear under Resume"
        );
    }

    #[test]
    fn build_with_session_stateless_inserts_neither_flag() {
        let env = FakeEnv::new().with("HOME", "/h");
        let cmd = build_claude_command_with_session(
            &default_cfg(),
            &mcp_path(),
            &[],
            ClaudeSessionMode::Stateless,
            &env,
        )
        .unwrap();
        let args = args_of(&cmd);
        assert!(!args.iter().any(|a| a == "--session-id"));
        assert!(!args.iter().any(|a| a == "--resume"));
    }

    #[test]
    fn spawn_uses_consistent_cwd_across_first_and_resume() {
        // Item 14 from §5.2.10. When operators set
        // `agent.session_store_path = Some(/tmp/test-session-root)`,
        // both the First and Resume invocations MUST spawn from the
        // exact same cwd so Claude Code's `<cwd-hash>` lookup lands in
        // the same `~/.claude/projects/...` directory.
        //
        // Source-level witness: the only line in spawn.rs that calls
        // `cmd.current_dir(session_root)` is the shared build path —
        // both First and Resume go through the same code, so they
        // inherit the same cwd by construction. Lock it with a
        // source scan so a future refactor that splits the paths
        // fails loudly.
        const SOURCE: &str = include_str!("spawn.rs");
        // Split literal to avoid matching the test body.
        let needle = ["cmd.curr", "ent_dir(session_root)"].concat();
        assert!(
            SOURCE.contains(&needle),
            "spawn.rs must pin `cmd.current_dir(session_root)` in the \
             shared `build_claude_command_with_env` path so First and \
             Resume invocations inherit the same cwd. See §5.2.2 cwd \
             pinning contract."
        );
    }

    #[test]
    fn cwd_pin_survives_parent_chdir() {
        // Item 15 from §5.2.10. The cwd pin must NOT re-read the
        // parent process's current directory on every build — that
        // would let a mid-process set-current-dir call shift active
        // sessions. Since `config.session_store_path` is the ONLY
        // cwd source in the spawn module, this test is a source-level
        // regression lock that the spawn module never reaches for the
        // process cwd.
        //
        // Split-literal needle so the panic message (which quotes the
        // forbidden symbol) cannot self-match via include_str! recursion.
        const SOURCE: &str = include_str!("spawn.rs");
        let forbidden = ["std::env::curr", "ent_dir"].concat();
        assert!(
            !SOURCE.contains(&forbidden),
            "build_claude_command must not read the process's current \
             directory at spawn time — session cwd pinning lives on \
             `AgentConfig.session_store_path` or on the startup-captured \
             cwd held by KairosProvider (PR A7)."
        );
    }
}
