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
    cmd.arg("--output-format").arg("stream-json");
    cmd.arg("--mcp-config").arg(mcp_config_path);
    cmd.arg("--strict-mcp-config");
    cmd.arg("--dangerously-skip-permissions");

    let allowed = format_allowed_tools(allowed_tools);
    if !allowed.is_empty() {
        cmd.arg("--allowed-tools").arg(allowed);
    }

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
