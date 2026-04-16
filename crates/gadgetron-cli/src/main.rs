use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use gadgetron_core::config::{AppConfig, ProviderConfig};
use gadgetron_core::provider::LlmProvider;
use gadgetron_core::secret::Secret;
use gadgetron_core::ui::WsMessage;
use gadgetron_gateway::server::{build_router, AppState};
use gadgetron_router::{MetricsStore, Router as LlmRouter};
use gadgetron_xaas::audit::writer::AuditWriter;
use gadgetron_xaas::auth::validator::PgKeyValidator;
use gadgetron_xaas::quota::enforcer::InMemoryQuotaEnforcer;
use sqlx::Row as _;
use tokio::sync::broadcast;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// Gadgetron — Rust-native GPU/LLM orchestration platform.
#[derive(Parser)]
#[command(
    name = "gadgetron",
    version,
    about = "GPU/LLM orchestration platform",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the gateway server.
    ///
    /// If database_url is not configured (gadgetron.toml or GADGETRON_DATABASE_URL),
    /// starts in no-db mode automatically with a warning.
    Serve {
        /// Path to TOML configuration file.
        /// Overrides GADGETRON_CONFIG environment variable.
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,

        /// TCP bind address (host:port).
        /// Overrides GADGETRON_BIND environment variable and config.server.bind.
        #[arg(long, short = 'b')]
        bind: Option<String>,

        /// Launch the ratatui TUI dashboard in the current terminal.
        /// Opens alternate screen; press 'q' or Esc to quit.
        #[arg(long)]
        tui: bool,

        /// Force no-db mode even if database_url is configured.
        /// Keys are validated by format only. Quota is disabled.
        /// Do not use in production.
        #[arg(long)]
        no_db: bool,

        /// Quick-start: connect to a single provider endpoint without a config file.
        /// Any gad_live_* or gad_test_* key is accepted. No database required.
        /// Example: gadgetron serve --provider http://10.100.1.5:8100
        /// Env: GADGETRON_PROVIDER
        #[arg(long)]
        provider: Option<String>,
    },

    /// Manage tenants. Requires PostgreSQL.
    Tenant {
        #[command(subcommand)]
        command: TenantCmd,
    },

    /// Manage API keys.
    ///
    /// In no-db mode (gadgetron serve without database), use without --tenant-id.
    /// In full mode (PostgreSQL), --tenant-id is required.
    Key {
        #[command(subcommand)]
        command: KeyCmd,
    },

    /// Generate an annotated gadgetron.toml in the current directory.
    ///
    /// If stdin is a TTY, prompts for each field interactively.
    /// Pass --yes to write defaults without any prompts (useful in scripts/CI).
    ///
    /// Example:
    ///   gadgetron init                     # interactive
    ///   gadgetron init --yes               # non-interactive, accept all defaults
    ///   gadgetron init --output /etc/gadgetron/gadgetron.toml
    Init {
        /// Destination path for the generated config file.
        /// Env: GADGETRON_CONFIG (used only by serve; init always writes to this path)
        #[arg(long, short = 'o', default_value = "gadgetron.toml")]
        output: PathBuf,

        /// Overwrite existing file without prompting.
        /// Required when stdout is not a TTY (e.g., scripts, CI).
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Diagnose Gadgetron configuration and connectivity.
    ///
    /// Checks config file, port availability, database connectivity,
    /// provider reachability, and the running server health endpoint.
    ///
    /// Example:
    ///   gadgetron doctor
    ///   gadgetron doctor --config /etc/gadgetron/gadgetron.toml
    Doctor {
        /// Config file to check (default: gadgetron.toml in current directory).
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,
    },

    /// Run the Model Context Protocol (MCP) stdio server.
    ///
    /// This subcommand is invoked by Claude Code as a child process via
    /// the `--mcp-config` JSON file that Kairos writes per request.
    /// It reads JSON-RPC 2.0 messages from stdin, dispatches tool calls
    /// through the registered `McpToolProvider`s (currently just
    /// `KnowledgeToolProvider` from `[knowledge]`), and writes responses
    /// to stdout.
    ///
    /// Not intended for direct operator use. `gadgetron serve` is the
    /// user-facing entry point; the mcp serve subcommand exists only as
    /// the child-side of the Kairos subprocess bridge.
    Mcp {
        #[command(subcommand)]
        command: McpCmd,
    },
}

/// Subcommands for `gadgetron mcp`.
#[derive(Subcommand)]
enum McpCmd {
    /// Run the stdio MCP server on the current process's stdin/stdout.
    /// Exits cleanly on stdin EOF (parent process exit).
    Serve {
        /// Path to the config file containing the `[knowledge]` section.
        /// Defaults to `gadgetron.toml` in the current directory.
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,
    },
}

/// Subcommands for `gadgetron tenant`.
#[derive(Subcommand)]
enum TenantCmd {
    /// Create a new tenant and print its UUID.
    Create {
        /// Human-readable display name for the tenant.
        #[arg(long)]
        name: String,
    },
    /// List all active tenants. Requires PostgreSQL.
    List,
}

/// Subcommands for `gadgetron key`.
#[derive(Subcommand)]
enum KeyCmd {
    /// Create an API key. The raw key is printed once and never stored.
    ///
    /// In no-db mode (no GADGETRON_DATABASE_URL): --tenant-id is not required.
    /// In full mode (PostgreSQL): --tenant-id is required.
    /// The key hash is stored; the raw key is shown once and cannot be retrieved.
    Create {
        /// UUID of the owning tenant. Required in full (PostgreSQL) mode.
        /// Omit in no-db mode — key is validated by format only and not persisted.
        #[arg(long)]
        tenant_id: Option<Uuid>,

        /// Comma-separated list of scopes granted to this key.
        /// Default: OpenAiCompat
        #[arg(long, default_value = "OpenAiCompat")]
        scope: String,
    },
    /// List API keys for a tenant (hashes are never shown). Requires PostgreSQL.
    List {
        /// UUID of the tenant whose keys to list.
        #[arg(long)]
        tenant_id: Uuid,
    },
    /// Revoke an API key by its UUID. Requires PostgreSQL.
    Revoke {
        /// UUID of the key record to revoke.
        #[arg(long)]
        key_id: Uuid,
    },
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Serve {
            config,
            bind,
            tui,
            no_db,
            provider,
        }) => serve(config, bind, tui, no_db, provider).await,
        Some(Commands::Tenant {
            command: TenantCmd::Create { name },
        }) => {
            let pool = connect_pg().await?;
            tenant_create(&pool, &name).await
        }
        Some(Commands::Tenant {
            command: TenantCmd::List,
        }) => {
            let pool = connect_pg().await?;
            tenant_list(&pool).await
        }
        Some(Commands::Key {
            command: KeyCmd::Create { tenant_id, scope },
        }) => {
            // no-db mode: tenant_id is None — print key without PG insert.
            // full mode: tenant_id is Some — requires PG.
            match tenant_id {
                None => {
                    // no-db mode: generate and print key, no PG required.
                    key_create_no_db(&scope)
                }
                Some(tid) => {
                    let pool = connect_pg().await?;
                    key_create(&pool, tid, &scope).await
                }
            }
        }
        Some(Commands::Key {
            command: KeyCmd::List { tenant_id },
        }) => {
            let pool = connect_pg().await?;
            key_list(&pool, tenant_id).await
        }
        Some(Commands::Key {
            command: KeyCmd::Revoke { key_id },
        }) => {
            let pool = connect_pg().await?;
            key_revoke(&pool, key_id).await
        }
        Some(Commands::Init { output, yes }) => cmd_init(&output, yes),
        Some(Commands::Doctor { config }) => cmd_doctor(config).await,

        Some(Commands::Mcp {
            command: McpCmd::Serve { config },
        }) => cmd_mcp_serve(config).await,

        // No subcommand given: default to `serve` with no flags.
        None => serve(None, None, false, false, None).await,
    }
}

/// `gadgetron mcp serve` — run the stdio MCP server.
///
/// Reads the `[knowledge]` section from the config file, builds a
/// `KnowledgeToolProvider`, freezes it into an `McpToolRegistry`, and
/// calls `gadgetron_kairos::serve_stdio(registry)` to handle the
/// JSON-RPC 2.0 message loop on stdin/stdout.
///
/// Exits cleanly on stdin EOF. Errors in config loading or provider
/// construction produce a descriptive message on stderr and exit
/// code 1.
async fn cmd_mcp_serve(config_path_override: Option<PathBuf>) -> Result<()> {
    use std::sync::Arc;

    let config_path: PathBuf = config_path_override
        .or_else(|| std::env::var("GADGETRON_CONFIG").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("gadgetron.toml"));

    if !config_path.exists() {
        anyhow::bail!(
            "config file not found: {}. Create a gadgetron.toml with a `[knowledge]` section, or pass --config.",
            config_path.display()
        );
    }

    let raw = std::fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;

    let knowledge_cfg = gadgetron_knowledge::config::KnowledgeConfig::extract_from_toml_str(&raw)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "`[knowledge]` section is missing in {}. `gadgetron mcp serve` \
                 requires the knowledge layer to be configured.",
                config_path.display()
            )
        })?;

    knowledge_cfg
        .validate()
        .map_err(|e| anyhow::anyhow!("[knowledge] config invalid: {e}"))?;

    // Also load the `[agent]` section from the same TOML so the
    // registry can enforce L3 defense-in-depth (ADR-P2A-06 addendum
    // item 3). The stdio MCP server is a grandchild process that does
    // not go through the gateway's `AppConfig::load` path, so we parse
    // the full AppConfig here and pick out `.agent`. A missing
    // section is tolerated because `AgentConfig` implements
    // `#[serde(default)]` at the AppConfig level.
    let agent_cfg = gadgetron_core::config::AppConfig::load(&config_path.to_string_lossy())
        .map(|app| app.agent)
        .unwrap_or_default();

    let provider = gadgetron_knowledge::KnowledgeToolProvider::new(knowledge_cfg)
        .map_err(|e| anyhow::anyhow!("failed to open knowledge provider: {e:?}"))?;

    let mut builder = gadgetron_kairos::McpToolRegistryBuilder::new();
    builder
        .register(Arc::new(provider))
        .map_err(|e| anyhow::anyhow!("failed to register KnowledgeToolProvider: {e:?}"))?;
    // Freeze against the operator's [agent] config so `dispatch()` can
    // enforce L3 defense-in-depth on any tool call that reaches this
    // stdio server (ADR-P2A-06 Implementation status addendum item 3).
    let registry = Arc::new(builder.freeze(&agent_cfg));

    // Drive the stdio loop until EOF.
    gadgetron_kairos::serve_stdio(registry)
        .await
        .map_err(|e| anyhow::anyhow!("mcp stdio server error: {e}"))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// PostgreSQL connection helper for CLI commands
// ---------------------------------------------------------------------------

/// Connect to PostgreSQL using `GADGETRON_DATABASE_URL`.
///
/// Error UX: answers what happened, why, and what the user should do.
async fn connect_pg() -> Result<sqlx::PgPool> {
    let url = std::env::var("GADGETRON_DATABASE_URL").map_err(|_| {
        anyhow::anyhow!(
            "GADGETRON_DATABASE_URL is not set.\n\n  \
             This command requires a PostgreSQL database.\n  \
             Next step: export GADGETRON_DATABASE_URL=postgres://user:pass@localhost:5432/gadgetron"
        )
    })?;

    sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&url)
        .await
        .with_context(|| {
            format!(
                "Failed to connect to PostgreSQL.\n\n  \
                 Attempted: {url}\n  \
                 Next steps:\n    \
                 - Verify PostgreSQL is running: pg_isready\n    \
                 - Check credentials in GADGETRON_DATABASE_URL"
            )
        })
}

// ---------------------------------------------------------------------------
// `gadgetron tenant create`
// ---------------------------------------------------------------------------

/// Create a new tenant and print its UUID.
///
/// Output matches design doc §1.4 Stage 4-C exactly.
async fn tenant_create(pool: &sqlx::PgPool, name: &str) -> Result<()> {
    let row = sqlx::query("INSERT INTO tenants (name) VALUES ($1) RETURNING id")
        .bind(name)
        .fetch_one(pool)
        .await
        .with_context(|| {
            format!(
                "Failed to create tenant '{name}'.\n\n  \
                 Cause: database INSERT failed.\n  \
                 Next step: Verify the tenants table exists — run 'gadgetron serve' to apply migrations."
            )
        })?;
    let id: Uuid = row.get("id");

    println!("Tenant Created");
    println!();
    println!("  {:<6} {id}", "ID:");
    println!("  {:<6} {name}", "Name:");
    println!();
    println!("  Next: gadgetron key create --tenant-id {id}");

    Ok(())
}

// ---------------------------------------------------------------------------
// `gadgetron key create`
// ---------------------------------------------------------------------------

/// Create an API key for a tenant, insert the hash into PostgreSQL, and print
/// the raw key to stdout exactly once.
///
/// SEC-M7: The raw key is printed to stderr (not stdout) so that it cannot be
/// accidentally captured in a script without the operator noticing.
/// The hash is stored in `api_keys`; the raw key is never logged or persisted.
///
/// Output matches design doc §1.4 Stage 4-B exactly.
async fn key_create(pool: &sqlx::PgPool, tenant_id: Uuid, scope: &str) -> Result<()> {
    let (raw_key, key_hash) = gadgetron_xaas::auth::key_gen::generate_api_key("live");

    sqlx::query(
        "INSERT INTO api_keys (tenant_id, prefix, key_hash, kind, scopes) \
         VALUES ($1, 'gad_live', $2, 'live', ARRAY[$3]::TEXT[])",
    )
    .bind(tenant_id)
    .bind(&key_hash)
    .bind(scope)
    .execute(pool)
    .await
    .with_context(|| {
        format!(
            "Failed to create API key for tenant {tenant_id}.\n\n  \
             Cause: database INSERT failed.\n  \
             Next steps:\n    \
             - Verify the tenant exists: gadgetron tenant list\n    \
             - Verify migrations are applied: gadgetron serve (applies migrations on boot)"
        )
    })?;

    eprintln!();
    eprintln!("  API Key Created");
    eprintln!();
    eprintln!("  {:<8} {raw_key}", "Key:");
    eprintln!("  {:<8} {tenant_id}", "Tenant:");
    eprintln!("  {:<8} {scope}", "Scopes:");
    eprintln!();
    eprintln!("  Save this key — it will not be shown again.");
    eprintln!();

    Ok(())
}

/// Create an API key without PostgreSQL (no-db mode).
///
/// Generates a `gad_live_*` key, prints it to stdout, and returns.
/// The key is never stored anywhere — the `InMemoryKeyValidator` accepts
/// any key with a valid `gad_live_` or `gad_test_` prefix.
///
/// Output matches design doc §1.4 Stage 4-A exactly.
fn key_create_no_db(scope: &str) -> Result<()> {
    let (raw_key, _key_hash) = gadgetron_xaas::auth::key_gen::generate_api_key("live");

    println!("API Key Created");
    println!();
    println!("  {:<8} {raw_key}", "Key:");
    println!();
    println!("  Save this key — it cannot be retrieved later.");
    println!();
    println!("  Scopes: {scope}");
    println!();
    println!("  Test it:");
    println!("    curl http://localhost:8080/v1/chat/completions \\");
    println!("      -H \"Authorization: Bearer {raw_key}\" \\");
    println!("      -H \"Content-Type: application/json\" \\");
    println!("      -d '{{\"model\":\"<model>\",\"messages\":[{{\"role\":\"user\",\"content\":\"Hello!\"}}]}}'");

    Ok(())
}

// ---------------------------------------------------------------------------
// `gadgetron tenant list`
// ---------------------------------------------------------------------------

/// List all tenants ordered by creation time (newest first).
///
/// Output: aligned table with ID, Name, Status, Created columns.
/// Empty state: user-friendly message with next-step hint.
async fn tenant_list(pool: &sqlx::PgPool) -> Result<()> {
    let rows =
        sqlx::query("SELECT id, name, status, created_at FROM tenants ORDER BY created_at DESC")
            .fetch_all(pool)
            .await
            .with_context(|| {
                "Failed to query tenants.\n\n  \
         Cause: database SELECT failed.\n  \
         Next step: Verify migrations are applied — run 'gadgetron serve' to apply them."
                    .to_string()
            })?;

    if rows.is_empty() {
        println!("No tenants found.");
        println!();
        println!("  Next: gadgetron tenant create --name <name>");
        return Ok(());
    }

    println!("{:<38} {:<20} {:<12} Created", "ID", "Name", "Status");
    println!("{}", "-".repeat(90));
    for row in &rows {
        let id: Uuid = row.get("id");
        let name: String = row.get("name");
        let status: String = row.get("status");
        let created: chrono::DateTime<chrono::Utc> = row.get("created_at");
        println!(
            "{:<38} {:<20} {:<12} {}",
            id,
            name,
            status,
            created.format("%Y-%m-%d %H:%M")
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// `gadgetron key list`
// ---------------------------------------------------------------------------

/// List active (non-revoked) API keys for a tenant, ordered newest first.
///
/// Key hashes are never shown. Only prefix, kind, and scopes are displayed.
/// Output: aligned table with ID, Prefix, Kind, Scopes, Created columns.
async fn key_list(pool: &sqlx::PgPool, tenant_id: Uuid) -> Result<()> {
    let rows = sqlx::query(
        "SELECT id, prefix, kind, scopes, created_at \
         FROM api_keys \
         WHERE tenant_id = $1 AND revoked_at IS NULL \
         ORDER BY created_at DESC",
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await
    .with_context(|| {
        format!(
            "Failed to query API keys for tenant {tenant_id}.\n\n  \
             Cause: database SELECT failed.\n  \
             Next step: Verify the tenant exists: gadgetron tenant list"
        )
    })?;

    if rows.is_empty() {
        println!("No active keys found for tenant {tenant_id}.");
        println!();
        println!("  Next: gadgetron key create --tenant-id {tenant_id}");
        return Ok(());
    }

    println!(
        "{:<38} {:<12} {:<8} {:<20} Created",
        "ID", "Prefix", "Kind", "Scopes"
    );
    println!("{}", "-".repeat(100));
    for row in &rows {
        let id: Uuid = row.get("id");
        let prefix: String = row.get("prefix");
        let kind: String = row.get("kind");
        let scopes: Vec<String> = row.get("scopes");
        let created: chrono::DateTime<chrono::Utc> = row.get("created_at");
        println!(
            "{:<38} {:<12} {:<8} {:<20} {}",
            id,
            prefix,
            kind,
            scopes.join(","),
            created.format("%Y-%m-%d %H:%M")
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// `gadgetron key revoke`
// ---------------------------------------------------------------------------

/// Revoke an API key by its UUID.
///
/// Sets `revoked_at = NOW()` only if the key is not already revoked.
/// Idempotency: a key that was already revoked (or does not exist) returns an
/// actionable error — the operator knows they need to verify the key ID.
async fn key_revoke(pool: &sqlx::PgPool, key_id: Uuid) -> Result<()> {
    let result = sqlx::query(
        "UPDATE api_keys SET revoked_at = NOW() \
         WHERE id = $1 AND revoked_at IS NULL \
         RETURNING id",
    )
    .bind(key_id)
    .fetch_optional(pool)
    .await
    .with_context(|| {
        format!(
            "Failed to revoke key {key_id}.\n\n  \
             Cause: database UPDATE failed.\n  \
             Next step: Verify the database is reachable and the api_keys table exists."
        )
    })?;

    match result {
        Some(_) => {
            println!("Key revoked: {key_id}");
            println!();
            println!("  The key can no longer be used to authenticate requests.");
        }
        None => {
            anyhow::bail!(
                "Key not found or already revoked: {key_id}\n\n  \
                 Cause: No active key with this UUID exists in the database.\n  \
                 Next step: Verify the key ID with 'gadgetron key list --tenant-id <uuid>'."
            )
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Core serve function
// ---------------------------------------------------------------------------

/// Pre-check: `--tui` requires a TTY on both stdin and stdout.
///
/// Separated from the `std::io::stdin()/stdout()` calls so unit tests can
/// exercise every (tui_enabled × has_tty) combination without a real TTY.
///
/// Returns `Ok(())` when `tui_enabled == false` OR `has_tty == true`.
/// Returns `Err` with a multi-line actionable message otherwise.
fn require_tty_for_tui(tui_enabled: bool, has_tty: bool) -> anyhow::Result<()> {
    if !tui_enabled || has_tty {
        return Ok(());
    }
    anyhow::bail!(
        "--tui requires an interactive terminal (stdin or stdout is not a TTY).\n\
         \n  \
         Cause: stdin/stdout is not connected to a terminal — this happens under systemd,\n  \
         \x20      CI runners, SSH with -T, IDE task runners, and pipe redirects.\n\
         \n  \
         Next steps:\n    \
         1. Run gadgetron from a regular shell (iTerm, Terminal.app, Alacritty, ...)\n    \
         2. Remove --tui to run headless — the server is reachable at GET /health\n       \
            and GET /v1/models once started.\n    \
         3. For systemd/CI: omit --tui or set tui = false in gadgetron.toml.\n       \
            See docs/manual/configuration.md for the full option reference."
    )
}

/// Run the gateway server.
///
/// `no_db`: force no-db mode even when `GADGETRON_DATABASE_URL` is set.
/// `provider_override`: when Some, skip config file and inject a synthetic vLLM
/// provider pointing at the given endpoint (gadgetron serve --provider pattern).
/// Implies no-db mode.
async fn serve(
    config_path_override: Option<PathBuf>,
    bind_override: Option<String>,
    tui_enabled: bool,
    no_db: bool,
    provider_override: Option<String>,
) -> Result<()> {
    // Step 1: Initialise structured tracing (always pretty for now; future --log-format flag).
    init_tracing();

    // Step 1b: When --provider is set, inject the endpoint as the sole vLLM provider
    // and skip config file loading entirely (vLLM quick-start pattern).
    // Print progress so the user knows something is happening.
    let provider_quickstart_endpoint: Option<String> =
        provider_override.or_else(|| std::env::var("GADGETRON_PROVIDER").ok());

    // Step 2: Resolve config path.
    //   Priority: CLI --config > GADGETRON_CONFIG env > default ./gadgetron.toml
    let config_path: PathBuf = config_path_override
        .or_else(|| std::env::var("GADGETRON_CONFIG").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("gadgetron.toml"));

    // Load AppConfig; tolerate missing file at run time — we only fail if the
    // file exists but is malformed. If absent, use built-in defaults (Ollama pattern)
    // so that `gadgetron serve` works without any setup.
    let config: AppConfig = if let Some(ref endpoint) = provider_quickstart_endpoint {
        // --provider mode: build a synthetic config with one vLLM provider.
        // Config file is intentionally bypassed.
        let mut cfg = AppConfig::default();
        cfg.providers.insert(
            "provider".to_string(),
            gadgetron_core::config::ProviderConfig::Vllm {
                endpoint: endpoint.clone(),
                api_key: None,
            },
        );
        cfg
    } else if config_path.exists() {
        AppConfig::load(config_path.to_str().unwrap_or("gadgetron.toml"))
            .with_context(|| format!("failed to load config from {}", config_path.display()))?
    } else {
        // No config file found — print user-visible message and use built-in defaults.
        println!("No config file found — using built-in defaults.");
        println!("   Create one: gadgetron init");
        println!();
        AppConfig::default()
    };

    // Resolve bind address.
    // Priority: CLI --bind > GADGETRON_BIND env > config.server.bind
    let bind_addr = bind_override
        .or_else(|| std::env::var("GADGETRON_BIND").ok())
        .unwrap_or_else(|| config.server.bind.clone());

    tracing::info!(bind = %bind_addr, tui = tui_enabled, "gadgetron starting");

    // Step 2.5: TTY pre-check when --tui requested.
    //
    // Without this, crossterm's enable_raw_mode() fails inside the TUI thread
    // with ENXIO ("Device not configured"), a single tracing::warn is emitted,
    // and the server silently runs headless — a terrible DX when the user
    // explicitly asked for an interactive dashboard.
    //
    // Fail-fast with sysexits.h EX_USAGE (2) matches `gadgetron doctor`.
    if tui_enabled {
        use std::io::IsTerminal;
        let has_tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
        if let Err(e) = require_tty_for_tui(tui_enabled, has_tty) {
            eprintln!("Error: {e}");
            std::process::exit(2);
        }
    }

    // Step 3: Determine DB mode.
    //   Priority: --no-db flag > --provider flag > env/config database_url
    let use_no_db = no_db
        || provider_quickstart_endpoint.is_some()
        || std::env::var("GADGETRON_DATABASE_URL").map_or(true, |s| s.is_empty());

    // Step 4: PostgreSQL connection pool (skipped in no-db mode).
    let (key_validator, pg_pool_opt): (
        Arc<dyn gadgetron_xaas::auth::validator::KeyValidator + Send + Sync>,
        Option<sqlx::PgPool>,
    ) = if use_no_db {
        // Both eprintln! and tracing::warn! are kept: eprintln! ensures visibility
        // when RUST_LOG=off silences the tracing subscriber. No redundant "WARNING:"
        // prefix — stderr channel and WARN level already imply it.
        let msg = "Running without database — keys not validated, quota disabled";
        eprintln!("{msg}");
        tracing::warn!(mode = "no-db", "{}", msg);
        (
            Arc::new(gadgetron_xaas::auth::validator::InMemoryKeyValidator),
            None,
        )
    } else {
        //   SEC-M7: DATABASE_URL is wrapped in Secret<String> and is never emitted
        //   to tracing span fields. We call .expose() only inside connect().
        let db_url_str = std::env::var("GADGETRON_DATABASE_URL")
            .context("GADGETRON_DATABASE_URL environment variable is required")?;
        let db_url = Secret::new(db_url_str);

        eprint!("  Connecting to PostgreSQL...");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(20)
            .acquire_timeout(Duration::from_secs(5))
            .connect(db_url.expose())
            .await
            .map_err(|e| {
                eprintln!(" failed");
                let redacted = redact_db_url(db_url.expose());
                anyhow::anyhow!(
                    "Failed to connect to PostgreSQL.\n\n  Attempted: {redacted}\n  Cause:     {e}\n\n  \
                     Next steps:\n    \
                     - Verify PostgreSQL is running: pg_isready\n    \
                     - Check credentials in GADGETRON_DATABASE_URL\n    \
                     - To run without a database: leave database_url empty in gadgetron.toml"
                )
            })?;
        eprintln!(" done");

        // Step 4b: Run sqlx migrations from gadgetron-xaas/migrations/.
        eprint!("  Running migrations...");
        sqlx::migrate!("../gadgetron-xaas/migrations")
            .run(&pool)
            .await
            .context("failed to run database migrations")?;
        eprintln!(" done");
        tracing::info!("database migrations applied");

        let kv = Arc::new(PgKeyValidator::new(pool.clone()))
            as Arc<dyn gadgetron_xaas::auth::validator::KeyValidator + Send + Sync>;
        (kv, Some(pool))
    };

    // Step 5: InMemoryQuotaEnforcer (Phase 1; PostgreSQL-backed is Phase 2).
    let quota_enforcer = Arc::new(InMemoryQuotaEnforcer)
        as Arc<dyn gadgetron_xaas::quota::enforcer::QuotaEnforcer + Send + Sync>;

    // Step 6: AuditWriter — mpsc channel capacity 4 096.
    let (audit_writer, audit_rx) = AuditWriter::new(4_096);
    let audit_writer = Arc::new(audit_writer);

    // Step 7: Audit consumer task — Phase 1: log to tracing only.
    //   JoinHandle retained for graceful drain in Step 14.
    let audit_handle = tokio::spawn(audit_consumer_loop(audit_rx));

    // Step 8: TUI broadcast channel.
    //   broadcast::channel capacity 1_024: 1_000 QPS ceiling × ~1s drain period + 24 headroom.
    //   Sender is Clone; the initial Receiver is dropped — TUI thread will call subscribe().
    let tui_tx: Option<broadcast::Sender<WsMessage>> = if tui_enabled {
        let (tx, _initial_rx) = broadcast::channel::<WsMessage>(1_024);
        // _initial_rx is dropped immediately.
        // Sender remains valid even with 0 receivers.
        Some(tx)
    } else {
        None
    };

    // Step 9: Build providers from config (empty HashMap boots fine).
    eprint!("  Checking provider(s)...");
    let providers_ss = build_providers(&config).context("failed to initialise LLM providers")?;
    eprintln!(" done ({} configured)", providers_ss.len());
    if providers_ss.is_empty() {
        eprintln!("  WARNING: No providers configured.");
        eprintln!("           All /v1/chat/completions requests will return 503.");
        eprintln!("           Fix: add [providers.*] to gadgetron.toml or use --provider <url>");
    }

    // Coerce Arc<dyn LlmProvider + Send + Sync> → Arc<dyn LlmProvider> for Router::new.
    let mut providers_for_router: HashMap<String, Arc<dyn LlmProvider>> = providers_ss
        .iter()
        .map(|(k, v)| (k.clone(), Arc::clone(v) as Arc<dyn LlmProvider>))
        .collect();

    // Step 9a (P2A Kairos wiring): if `[knowledge]` is present in the
    // config file, construct the knowledge layer, register it as a
    // KnowledgeToolProvider in an McpToolRegistry, and register
    // KairosProvider as the `"kairos"` entry in the router map.
    //
    // Silent skip when `[knowledge]` is absent — operators who have not
    // configured the knowledge layer get a normal non-Kairos server.
    register_kairos_if_configured(&config_path, &config, &mut providers_for_router);

    // Step 10: Build the LLM router.
    let metrics_store = Arc::new(MetricsStore::new());
    let llm_router = Arc::new(LlmRouter::new(
        providers_for_router,
        config.router.clone(),
        metrics_store,
    ));

    // Step 11: Assemble AppState.
    let state = AppState {
        key_validator,
        quota_enforcer,
        audit_writer: audit_writer.clone(),
        providers: Arc::new(providers_ss),
        router: Some(llm_router),
        pg_pool: pg_pool_opt.clone(),
        no_db: use_no_db,
        tui_tx: tui_tx.clone(),
    };

    // Step 13: TUI thread (only when --tui is active).
    //
    // Concurrency decision: `std::thread::spawn` + internal `tokio::runtime::Builder::new_current_thread()`.
    // Rationale: crossterm's `event::poll` is a blocking syscall. Running on a tokio
    // worker thread would consume async budget. A dedicated OS thread with its own
    // single-threaded tokio runtime provides isolation (design §1.3).
    let tui_thread = if let Some(ref tx) = tui_tx {
        let rx = tx.subscribe(); // new Receiver from the Sender
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("TUI tokio runtime must build");
            rt.block_on(async move {
                let mut app = gadgetron_tui::App::with_channel(rx);
                if let Err(e) = app.run().await {
                    tracing::warn!(error = %e, "TUI exited with error");
                }
            });
        });
        Some(handle)
    } else {
        None
    };

    // Step 14: Build the axum Router (Tower middleware stack).
    // Print "Starting server..." progress line before binding (matches design §1.4 Stage 3-B).
    eprint!("  Starting server...");
    let app = build_router(state);

    // Step 15: Bind TCP listener.
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("failed to bind to {bind_addr}"))?;

    eprintln!(" done");
    tracing::info!(addr = %bind_addr, "listening");

    // Print startup banner to stdout (matches design §1.4 Stage 3-B).
    print_serve_banner(
        env!("CARGO_PKG_VERSION"),
        &bind_addr,
        &config
            .providers
            .values()
            .map(provider_endpoint_summary)
            .collect::<Vec<_>>(),
    );

    // Step 16: axum::serve with graceful shutdown on SIGTERM or Ctrl-C.
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    tracing::info!("axum serve exited — starting drain sequence");

    // Step 17: Shutdown drain sequence.
    //
    // 17a: Drop TUI broadcast Sender → channel closes → App::drain_updates sees
    //      TryRecvError::Closed → App sets running=false → TUI render loop exits.
    //      Must happen AFTER axum drains (no more RequestLog emits possible).
    drop(tui_tx);

    // 17b: Drop the Arc<AuditWriter> held here → internal mpsc::Sender drops →
    //      audit_consumer_loop's rx.recv() returns None → loop exits naturally.
    drop(audit_writer);

    // 17c: Wait up to 5 s for the audit consumer to flush (§2.F.4 budget).
    match tokio::time::timeout(Duration::from_secs(5), audit_handle).await {
        Ok(Ok(())) => tracing::info!("audit consumer drained cleanly"),
        Ok(Err(e)) => tracing::warn!(error = %e, "audit consumer task panicked"),
        Err(_) => tracing::warn!("audit consumer drain timed out after 5s — entries may be lost"),
    }

    // 17d: Join TUI thread (blocking — TUI should already be stopped after tui_tx drop).
    if let Some(handle) = tui_thread {
        let _ = handle.join();
    }

    // 17e: Close PG pool gracefully (no-op in no-db mode).
    if let Some(pool) = pg_pool_opt {
        pool.close().await;
    }

    tracing::info!("shutdown complete");
    Ok(())
}

// ---------------------------------------------------------------------------
// Graceful shutdown: SIGTERM (Unix) or Ctrl-C
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Phase 2A Kairos registration — P2A Step 22
// ---------------------------------------------------------------------------

/// Build the knowledge layer + McpToolRegistry + KairosProvider and
/// register it into the router's provider map under the `"kairos"`
/// key, IF the operator has a `[knowledge]` section in their
/// gadgetron.toml.
///
/// Silent no-op when `[knowledge]` is absent — non-Kairos operators
/// get a standard server with no knowledge-layer behavior.
///
/// Errors (malformed `[knowledge]`, wiki init failure, etc.) are
/// surfaced via `tracing::error!` and the Kairos provider is skipped,
/// NOT propagated — the server still starts with the other providers.
/// This matches the Phase 1 tolerance model for individual provider
/// construction failures.
fn register_kairos_if_configured(
    config_path: &std::path::Path,
    app_config: &AppConfig,
    providers: &mut HashMap<String, Arc<dyn LlmProvider>>,
) {
    // We re-read the toml file to extract the `[knowledge]` section.
    // The main AppConfig load path doesn't include a `knowledge`
    // field — adding one would require cross-crate type sharing
    // (gadgetron-core ↔ gadgetron-knowledge) that's more churn than
    // a ~5 ms second file read.
    if !config_path.exists() {
        return;
    }
    let raw = match std::fs::read_to_string(config_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(
                path = %config_path.display(),
                error = %e,
                "kairos: cannot re-read config file; skipping knowledge layer"
            );
            return;
        }
    };

    let knowledge_cfg =
        match gadgetron_knowledge::config::KnowledgeConfig::extract_from_toml_str(&raw) {
            Ok(Some(cfg)) => cfg,
            Ok(None) => {
                // No [knowledge] section → kairos not available.
                return;
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "kairos: [knowledge] section malformed; skipping"
                );
                return;
            }
        };

    if let Err(e) = knowledge_cfg.validate() {
        tracing::error!(error = %e, "kairos: [knowledge] validation failed; skipping");
        return;
    }

    // Build the KnowledgeToolProvider (opens/inits the wiki repo).
    let provider = match gadgetron_knowledge::KnowledgeToolProvider::new(knowledge_cfg) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = ?e, "kairos: KnowledgeToolProvider::new failed; skipping");
            return;
        }
    };

    // Freeze the registry with the single P2A provider. The `[agent]`
    // config is captured at freeze time so `dispatch()` can enforce L3
    // defense-in-depth (ADR-P2A-06 Implementation status addendum item 3).
    let mut builder = gadgetron_kairos::McpToolRegistryBuilder::new();
    if let Err(e) = builder.register(Arc::new(provider)) {
        tracing::error!(error = ?e, "kairos: registry.register failed; skipping");
        return;
    }
    let registry = Arc::new(builder.freeze(&app_config.agent));

    // Register KairosProvider under the "kairos" model id in the
    // router map. The existing provider map already holds concrete
    // OpenAI/Anthropic/vLLM/etc entries; this adds one more.
    //
    // P2A PR A4 wires a `NoopToolAuditEventSink` as the audit sink —
    // the real `ToolAuditEventWriter` lives in `gadgetron-xaas` and
    // is connected to the DB writer loop there. The composition root
    // for that wiring lands with the broader `AppState` audit plumbing;
    // for now Kairos silently drops tool-call events when the DB is
    // not configured, which preserves the previous tracing-only
    // behavior.
    let agent_cfg = Arc::new(app_config.agent.clone());
    let audit_sink: std::sync::Arc<dyn gadgetron_core::audit::ToolAuditEventSink> =
        std::sync::Arc::new(gadgetron_core::audit::NoopToolAuditEventSink);
    let session_store = std::sync::Arc::new(gadgetron_kairos::SessionStore::new(
        agent_cfg.session_ttl_secs,
        agent_cfg.session_store_max_entries,
    ));
    gadgetron_kairos::register_with_router(
        agent_cfg,
        registry,
        audit_sink,
        session_store,
        providers,
    );
    tracing::info!(
        model = "kairos",
        "kairos: registered (KnowledgeToolProvider active; web.search = {})",
        if providers.get("kairos").is_some() {
            "configured_via_knowledge_section"
        } else {
            "none"
        }
    );
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    {
        let terminate = async {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("failed to install SIGTERM handler")
                .recv()
                .await;
        };

        tokio::select! {
            _ = ctrl_c => tracing::info!("Ctrl-C received"),
            _ = terminate => tracing::info!("SIGTERM received"),
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await;
        tracing::info!("Ctrl-C received");
    }

    tracing::info!("shutdown signal received, draining connections");
}

// ---------------------------------------------------------------------------
// Audit consumer loop — Phase 1: log to tracing only
// ---------------------------------------------------------------------------

async fn audit_consumer_loop(
    mut rx: tokio::sync::mpsc::Receiver<gadgetron_xaas::audit::writer::AuditEntry>,
) {
    while let Some(entry) = rx.recv().await {
        // Phase 1: write to tracing.
        // Phase 2+: batch INSERT into audit_log PostgreSQL table.
        tracing::info!(
            tenant_id = %entry.tenant_id,
            api_key_id = %entry.api_key_id,
            request_id = %entry.request_id,
            status = entry.status.as_str(),
            input_tokens = entry.input_tokens,
            output_tokens = entry.output_tokens,
            latency_ms = entry.latency_ms,
            "audit"
        );
    }
    tracing::info!("audit consumer loop exiting — channel closed");
}

// ---------------------------------------------------------------------------
// `gadgetron init` — generate annotated gadgetron.toml
// ---------------------------------------------------------------------------

/// Execute `gadgetron init`.
///
/// Writes an annotated TOML template to `output`. Every field has a doc comment,
/// default value, and env-override hint so users can discover the config schema
/// without reading external docs.
///
/// What happened / why / what to do:
/// - File exists and `--yes` not passed → print warning and exit without writing.
/// - Write fails (permissions, disk full) → actionable error with cause + next step.
fn cmd_init(output: &std::path::Path, yes: bool) -> Result<()> {
    if output.exists() && !yes {
        println!("'{}' already exists. Overwrite? [y/N] ", output.display());
        // In non-interactive / piped mode the user cannot respond → treat as N.
        use std::io::BufRead as _;
        let stdin = std::io::stdin();
        let mut line = String::new();
        stdin.lock().read_line(&mut line).ok();
        if !matches!(line.trim(), "y" | "Y") {
            println!("Aborted. Existing file left unchanged.");
            return Ok(());
        }
    }

    let content = ANNOTATED_CONFIG_TEMPLATE;

    std::fs::write(output, content).map_err(|e| {
        anyhow::anyhow!(
            "Failed to write config to '{path}'.\n\n  \
             Cause:     {e}\n  \
             Next step: Check write permission on the current directory.",
            path = output.display(),
        )
    })?;

    println!("Config written to {}\n", output.display());
    println!("  Next steps:");
    println!(
        "    1. Review {} — uncomment additional providers as needed.",
        output.display()
    );
    println!("    2. Run: gadgetron serve");

    Ok(())
}

// ---------------------------------------------------------------------------
// `gadgetron doctor` — system check
// ---------------------------------------------------------------------------

/// Execute `gadgetron doctor`.
///
/// Runs 5 checks sequentially, prints `[PASS]`, `[WARN]`, or `[FAIL]` for each,
/// then summarises. Exits non-zero if any check is `[FAIL]`.
///
/// Checks (in order):
/// 1. Config file — exists and is valid TOML
/// 2. Server port — bind address available
/// 3. Database — `GADGETRON_DATABASE_URL` or config `database_url` set
/// 4. Provider(s) — HTTP GET to each configured provider endpoint
/// 5. `/health` — Gadgetron is running and responding
async fn cmd_doctor(config_path: Option<PathBuf>) -> Result<()> {
    println!("Gadgetron v{} — System Check\n", env!("CARGO_PKG_VERSION"));

    let path = config_path.unwrap_or_else(|| PathBuf::from("gadgetron.toml"));

    // Check 1: Config file
    let (config_check, maybe_config) = check_config(&path);
    print_check(&config_check);

    // Check 2: Port availability
    let bind = maybe_config
        .as_ref()
        .map(|c: &AppConfig| c.server.bind.as_str())
        .unwrap_or("0.0.0.0:8080");
    let port_check = check_port(bind);
    print_check(&port_check);

    // Check 3: Database
    let db_check = check_database_doctor(maybe_config.as_ref());
    print_check(&db_check);

    // Check 4: Provider(s)
    let mut provider_checks = vec![];
    if let Some(ref cfg) = maybe_config {
        for (name, provider_cfg) in &cfg.providers {
            let endpoint = doctor_provider_endpoint(provider_cfg);
            let result = check_provider_reachable(name, endpoint).await;
            print_check(&result);
            provider_checks.push(result);
        }
    }

    // Check 5: /health
    let health_check = check_health_endpoint(bind).await;
    print_check(&health_check);

    // Summary
    let all_checks: Vec<&DoctorCheck> = [&config_check, &port_check, &db_check]
        .into_iter()
        .chain(provider_checks.iter())
        .chain(std::iter::once(&health_check))
        .collect();

    let warn_count = all_checks
        .iter()
        .filter(|c| matches!(c.status, DoctorStatus::Warn(_)))
        .count();
    let fail_count = all_checks
        .iter()
        .filter(|c| matches!(c.status, DoctorStatus::Fail(_)))
        .count();

    println!();

    if warn_count == 0 && fail_count == 0 {
        println!("  All checks passed.");
        return Ok(());
    }

    if warn_count > 0 {
        println!("  {} warning(s) found.", warn_count);
        for c in &all_checks {
            if let DoctorStatus::Warn(msg) = &c.status {
                println!("  WARN: {}", msg);
            }
        }
    }
    if fail_count > 0 {
        println!("  {} failure(s) found.", fail_count);
        for c in &all_checks {
            if let DoctorStatus::Fail(msg) = &c.status {
                println!("  FAIL: {}", msg);
            }
        }
        std::process::exit(2);
    }

    Ok(())
}

/// A single doctor check result.
struct DoctorCheck {
    /// Short label, e.g. "Config file:", "Server bind:", "/health:"
    label: String,
    status: DoctorStatus,
}

enum DoctorStatus {
    Pass(String),
    Warn(String),
    Fail(String),
}

fn print_check(c: &DoctorCheck) {
    match &c.status {
        DoctorStatus::Pass(detail) => println!("  [PASS] {:<18} {}", c.label, detail),
        DoctorStatus::Warn(detail) => println!("  [WARN] {:<18} {}", c.label, detail),
        DoctorStatus::Fail(detail) => println!("  [FAIL] {:<18} {}", c.label, detail),
    }
}

fn check_config(path: &std::path::Path) -> (DoctorCheck, Option<AppConfig>) {
    match std::fs::read_to_string(path) {
        Err(_) => (
            DoctorCheck {
                label: "Config file:".into(),
                status: DoctorStatus::Warn(format!(
                    "{} not found (will use defaults)",
                    path.display()
                )),
            },
            None,
        ),
        Ok(content) => match toml::from_str::<AppConfig>(&content) {
            Ok(cfg) => (
                DoctorCheck {
                    label: "Config file:".into(),
                    status: DoctorStatus::Pass(format!("{} found and valid TOML", path.display())),
                },
                Some(cfg),
            ),
            Err(e) => (
                DoctorCheck {
                    label: "Config file:".into(),
                    status: DoctorStatus::Fail(format!(
                        "{} exists but TOML parse failed: {}",
                        path.display(),
                        e
                    )),
                },
                None,
            ),
        },
    }
}

fn check_port(bind: &str) -> DoctorCheck {
    use std::net::TcpListener;
    match TcpListener::bind(bind) {
        Ok(_) => DoctorCheck {
            label: "Server port:".into(),
            status: DoctorStatus::Pass(format!("{} available", bind)),
        },
        Err(e) => DoctorCheck {
            label: "Server port:".into(),
            status: DoctorStatus::Fail(format!("{} in use — {}", bind, e)),
        },
    }
}

fn check_database_doctor(config: Option<&AppConfig>) -> DoctorCheck {
    // Check env var first, then config
    let url = std::env::var("GADGETRON_DATABASE_URL")
        .ok()
        .filter(|s| !s.is_empty());

    if url.is_some() {
        DoctorCheck {
            label: "Database:".into(),
            status: DoctorStatus::Pass("GADGETRON_DATABASE_URL configured".into()),
        }
    } else {
        // No env var — note it's no-db mode
        let _ = config; // config would be checked here in a future phase
        DoctorCheck {
            label: "Database:".into(),
            status: DoctorStatus::Warn(
                "database_url not configured — running in no-db mode".into(),
            ),
        }
    }
}

async fn check_provider_reachable(name: &str, endpoint: &str) -> DoctorCheck {
    if endpoint.is_empty() {
        return DoctorCheck {
            label: format!("Provider {}:", name),
            status: DoctorStatus::Warn("no endpoint configured".into()),
        };
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    let start = std::time::Instant::now();
    match client.get(endpoint).send().await {
        // Any HTTP response (2xx/3xx/4xx/5xx) means the server is reachable.
        // Only connection errors (refused/timeout/DNS) count as failure.
        Ok(_resp) => {
            let ms = start.elapsed().as_millis();
            DoctorCheck {
                label: format!("Provider {}:", name),
                status: DoctorStatus::Pass(format!("{} reachable in {}ms", endpoint, ms)),
            }
        }
        Err(e) => DoctorCheck {
            label: format!("Provider {}:", name),
            status: DoctorStatus::Fail(format!("{} unreachable — {}", endpoint, e)),
        },
    }
}

async fn check_health_endpoint(bind: &str) -> DoctorCheck {
    // Derive host from bind — 0.0.0.0 → localhost
    let host = if bind.starts_with("0.0.0.0:") || bind.starts_with("[::]:") {
        let port = bind.rsplit(':').next().unwrap_or("8080");
        format!("localhost:{port}")
    } else {
        bind.to_string()
    };
    let url = format!("http://{host}/health");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default();

    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => DoctorCheck {
            label: "/health:".into(),
            status: DoctorStatus::Pass(format!("gadgetron is running at {url}")),
        },
        Ok(resp) => DoctorCheck {
            label: "/health:".into(),
            status: DoctorStatus::Fail(format!(
                "HTTP {} at {url} — check gadgetron logs",
                resp.status().as_u16()
            )),
        },
        Err(_) => DoctorCheck {
            label: "/health:".into(),
            status: DoctorStatus::Fail(format!(
                "connection refused at {url} — run: gadgetron serve"
            )),
        },
    }
}

/// Extract a displayable endpoint URL from a `ProviderConfig` enum variant.
fn doctor_provider_endpoint(cfg: &ProviderConfig) -> &str {
    match cfg {
        ProviderConfig::Openai { base_url, .. } => {
            base_url.as_deref().unwrap_or("https://api.openai.com")
        }
        ProviderConfig::Anthropic { base_url, .. } => {
            base_url.as_deref().unwrap_or("https://api.anthropic.com")
        }
        ProviderConfig::Vllm { endpoint, .. } => endpoint.as_str(),
        ProviderConfig::Sglang { endpoint, .. } => endpoint.as_str(),
        ProviderConfig::Ollama { endpoint } => endpoint.as_str(),
        ProviderConfig::Gemini { .. } => "https://generativelanguage.googleapis.com",
    }
}

/// Annotated configuration template written by `gadgetron init`.
///
/// Every field has: inline doc comment, default value, env-override hint.
/// Commented-out provider blocks serve as copy-pasteable examples.
const ANNOTATED_CONFIG_TEMPLATE: &str = r#"# Gadgetron Configuration
# Docs: docs/manual/configuration.md
#
# Generated by: gadgetron init
# Edit this file, then run: gadgetron serve

[server]
# TCP address to bind the gateway on.
# Env override: GADGETRON_BIND
bind = "0.0.0.0:8080"

# Request timeout in milliseconds. Requests exceeding this are cancelled.
# Env override: GADGETRON_REQUEST_TIMEOUT_MS
request_timeout_ms = 30000

# ---------------------------------------------------------------------------
# Providers — configure at least one LLM backend.
# Uncomment and fill in the appropriate section.
# ---------------------------------------------------------------------------

# Uncomment and configure a provider:
# [providers.my-provider]
# type = "vllm"          # openai | anthropic | ollama | vllm | sglang
# endpoint = "http://localhost:8000"
# models = ["model-name"]
# api_key = ""           # env: OPENAI_API_KEY (for openai type)

# --- OpenAI ---
# [providers.openai]
# type = "openai"
# api_key = "${OPENAI_API_KEY}"
# models = ["gpt-4o", "gpt-4o-mini"]

# --- Anthropic ---
# [providers.anthropic]
# type = "anthropic"
# api_key = "${ANTHROPIC_API_KEY}"
# models = ["claude-3-5-sonnet-20241022"]

# --- vLLM (local GPU) ---
# [providers.my-vllm]
# type = "vllm"
# endpoint = "http://localhost:8000"

# --- Ollama (local) ---
# [providers.my-ollama]
# type = "ollama"
# endpoint = "http://localhost:11434"

# ---------------------------------------------------------------------------
# Router — controls how requests are distributed across providers.
# ---------------------------------------------------------------------------

[router.default_strategy]
# Routing strategy: round_robin | cost_optimal | latency_optimal | fallback | weighted
type = "round_robin"
"#;

// ---------------------------------------------------------------------------
// Startup banner helpers
// ---------------------------------------------------------------------------

/// A brief summary of a configured provider for the startup banner.
struct ProviderBannerEntry {
    kind: String,
    endpoint: String,
}

/// Extract a human-readable endpoint string from a `ProviderConfig`.
fn provider_endpoint_summary(cfg: &ProviderConfig) -> ProviderBannerEntry {
    match cfg {
        ProviderConfig::Vllm { endpoint, .. }
        | ProviderConfig::Sglang { endpoint, .. }
        | ProviderConfig::Ollama { endpoint } => ProviderBannerEntry {
            kind: provider_type_name(cfg).to_string(),
            endpoint: endpoint.clone(),
        },
        ProviderConfig::Openai { base_url, .. } => ProviderBannerEntry {
            kind: "openai".to_string(),
            endpoint: base_url
                .clone()
                .unwrap_or_else(|| "https://api.openai.com".to_string()),
        },
        ProviderConfig::Anthropic { base_url, .. } => ProviderBannerEntry {
            kind: "anthropic".to_string(),
            endpoint: base_url
                .clone()
                .unwrap_or_else(|| "https://api.anthropic.com".to_string()),
        },
        ProviderConfig::Gemini { .. } => ProviderBannerEntry {
            kind: "gemini".to_string(),
            endpoint: "https://generativelanguage.googleapis.com".to_string(),
        },
    }
}

fn provider_type_name(cfg: &ProviderConfig) -> &'static str {
    match cfg {
        ProviderConfig::Openai { .. } => "openai",
        ProviderConfig::Anthropic { .. } => "anthropic",
        ProviderConfig::Gemini { .. } => "gemini",
        ProviderConfig::Ollama { .. } => "ollama",
        ProviderConfig::Vllm { .. } => "vllm",
        ProviderConfig::Sglang { .. } => "sglang",
    }
}

/// Print the startup banner to stdout.
///
/// Format matches design doc §1.4 Stage 3-B exactly.
/// `bind` is the resolved listen address (e.g. "0.0.0.0:8080").
/// `providers` is a slice of brief provider summaries.
fn print_serve_banner(version: &str, bind: &str, providers: &[ProviderBannerEntry]) {
    // Derive the externally-reachable URL from the bind address:
    // 0.0.0.0 and [::] are wildcard addresses — show localhost for the user.
    let host = if bind.starts_with("0.0.0.0:") || bind.starts_with("[::]:") {
        let port = bind.rsplit(':').next().unwrap_or("8080");
        format!("localhost:{port}")
    } else {
        bind.to_string()
    };

    println!();
    println!("Gadgetron v{version}");
    println!("   OpenAI API: http://{host}/v1");
    println!();
    println!("  Listen:     {bind}");
    if providers.is_empty() {
        println!("  Providers:  (none configured — add providers to gadgetron.toml)");
    } else {
        for p in providers {
            println!("  Providers:  {} @ {}", p.kind, p.endpoint);
        }
    }
    println!();
}

/// Redact the password component of a PostgreSQL URL for safe display.
///
/// `postgres://user:secret@host:5432/db` → `postgres://user:***@host:5432/db`
/// URLs without a password are returned unchanged.
fn redact_db_url(url: &str) -> String {
    // Strategy: find the last '@' (host marker), then find the last ':'
    // before it (password separator). Replace between the two.
    if let Some(at_pos) = url.rfind('@') {
        let before_at = &url[..at_pos];
        if let Some(colon_pos) = before_at.rfind(':') {
            // Ensure the colon is not part of the scheme (://). The scheme ends
            // at `://` which is always before any user-info colon.
            if colon_pos > 3 {
                let scheme_user = &url[..colon_pos];
                let host_db = &url[at_pos..];
                return format!("{scheme_user}:***{host_db}");
            }
        }
    }
    url.to_string()
}

// ---------------------------------------------------------------------------
// Provider builder
// ---------------------------------------------------------------------------

/// Iterate `AppConfig.providers` and instantiate `LlmProvider` adapters.
///
/// Phase 1 supports: OpenAI, Anthropic, Gemini, Ollama, vLLM, SGLang.
/// An empty providers map is valid — the server boots but cannot route to
/// any provider until at least one is configured.
fn build_providers(
    config: &AppConfig,
) -> Result<HashMap<String, Arc<dyn LlmProvider + Send + Sync>>> {
    let mut map: HashMap<String, Arc<dyn LlmProvider + Send + Sync>> = HashMap::new();

    for (name, provider_cfg) in &config.providers {
        let provider: Arc<dyn LlmProvider + Send + Sync> = match provider_cfg {
            ProviderConfig::Openai {
                api_key, base_url, ..
            } => Arc::new(gadgetron_provider::OpenAiProvider::new(
                api_key.clone(),
                base_url.clone(),
            )),
            ProviderConfig::Anthropic {
                api_key, base_url, ..
            } => Arc::new(gadgetron_provider::AnthropicProvider::new(
                api_key.clone(),
                base_url.clone(),
            )),
            ProviderConfig::Ollama { endpoint } => {
                Arc::new(gadgetron_provider::OllamaProvider::new(endpoint.clone()))
            }
            ProviderConfig::Vllm { endpoint, api_key } => Arc::new(
                gadgetron_provider::VllmProvider::new(endpoint.clone(), api_key.clone()),
            ),
            ProviderConfig::Sglang { endpoint, api_key } => Arc::new(
                gadgetron_provider::SglangProvider::new(endpoint.clone(), api_key.clone()),
            ),
            ProviderConfig::Gemini { api_key, models } => {
                let mut provider = gadgetron_provider::GeminiProvider::new(
                    "https://generativelanguage.googleapis.com/v1".to_string(),
                    Some(api_key.clone()),
                );
                if !models.is_empty() {
                    provider = provider.with_models(models.clone());
                }
                Arc::new(provider) as Arc<dyn LlmProvider + Send + Sync>
            }
        };

        tracing::info!(name = %name, "provider registered");
        map.insert(name.clone(), provider);
    }

    Ok(map)
}

// ---------------------------------------------------------------------------
// Tracing initialisation
// ---------------------------------------------------------------------------

fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_env("RUST_LOG")
        .unwrap_or_else(|_| "gadgetron=info,tower_http=info".parse().unwrap());

    tracing_subscriber::fmt().with_env_filter(filter).init();
}

// ---------------------------------------------------------------------------
// Minimal default config (used when gadgetron.toml is absent)
// ---------------------------------------------------------------------------

#[cfg(test)]
fn default_config() -> AppConfig {
    AppConfig::default()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // Existing tests (preserved)
    // ------------------------------------------------------------------

    /// Verify the binary target compiles by importing the top-level symbols.
    #[test]
    fn main_compiles() {
        // Compilation is the assertion.
    }

    // ------------------------------------------------------------------
    // Hotfix: --tui TTY pre-check
    // (docs/design/polish/hotfix-tui-tty-precheck.md)
    // ------------------------------------------------------------------

    #[test]
    fn require_tty_for_tui_ok_when_tui_disabled_and_no_tty() {
        // --tui not requested, no TTY: OK (headless mode).
        assert!(require_tty_for_tui(false, false).is_ok());
    }

    #[test]
    fn require_tty_for_tui_ok_when_tui_disabled_and_has_tty() {
        // --tui not requested, TTY present: OK (headless with stray terminal).
        assert!(require_tty_for_tui(false, true).is_ok());
    }

    #[test]
    fn require_tty_for_tui_ok_when_tui_enabled_and_has_tty() {
        // Normal interactive path.
        assert!(require_tty_for_tui(true, true).is_ok());
    }

    #[test]
    fn require_tty_for_tui_errors_when_tui_enabled_and_no_tty() {
        let err = require_tty_for_tui(true, false).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("--tui requires"),
            "error must start with '--tui requires', got: {msg}"
        );
        assert!(
            msg.contains("stdin or stdout"),
            "error must mention stdin or stdout, got: {msg}"
        );
        assert!(
            msg.contains("Next steps:"),
            "error must include 'Next steps:' label per dx-product-lead A1, got: {msg}"
        );
        assert!(
            msg.contains("Remove --tui"),
            "error must tell user how to run headless, got: {msg}"
        );
        assert!(
            msg.contains("gadgetron.toml"),
            "error must point to config file for systemd/CI, got: {msg}"
        );
    }

    #[test]
    fn default_config_has_expected_bind() {
        let cfg = default_config();
        assert_eq!(cfg.server.bind, "0.0.0.0:8080");
        assert!(cfg.providers.is_empty());
    }

    #[test]
    fn build_providers_empty_config_returns_empty_map() {
        let cfg = default_config();
        let providers = build_providers(&cfg).unwrap();
        assert!(providers.is_empty());
    }

    #[test]
    fn build_providers_vllm_activates() {
        use gadgetron_core::config::{AppConfig, ProviderConfig, ServerConfig};
        use gadgetron_core::routing::RoutingConfig;
        use std::collections::HashMap;

        let mut providers = HashMap::new();
        providers.insert(
            "my-vllm".to_string(),
            ProviderConfig::Vllm {
                endpoint: "http://localhost:8001".to_string(),
                api_key: None,
            },
        );
        let cfg = AppConfig {
            server: ServerConfig {
                bind: "0.0.0.0:8080".to_string(),
                api_key: None,
                request_timeout_ms: 30_000,
            },
            router: RoutingConfig::default(),
            providers,
            nodes: vec![],
            models: vec![],
            web: gadgetron_core::config::WebConfig::default(),
            agent: gadgetron_core::agent::AgentConfig::default(),
        };
        let map = build_providers(&cfg).unwrap();
        assert!(
            map.contains_key("my-vllm"),
            "vLLM provider must be registered"
        );
        assert_eq!(map["my-vllm"].name(), "vllm");
    }

    #[test]
    fn build_providers_sglang_activates() {
        use gadgetron_core::config::{AppConfig, ProviderConfig, ServerConfig};
        use gadgetron_core::routing::RoutingConfig;
        use std::collections::HashMap;

        let mut providers = HashMap::new();
        providers.insert(
            "my-sglang".to_string(),
            ProviderConfig::Sglang {
                endpoint: "http://localhost:30000".to_string(),
                api_key: Some("sk-test".to_string()),
            },
        );
        let cfg = AppConfig {
            server: ServerConfig {
                bind: "0.0.0.0:8080".to_string(),
                api_key: None,
                request_timeout_ms: 30_000,
            },
            router: RoutingConfig::default(),
            providers,
            nodes: vec![],
            models: vec![],
            web: gadgetron_core::config::WebConfig::default(),
            agent: gadgetron_core::agent::AgentConfig::default(),
        };
        let map = build_providers(&cfg).unwrap();
        assert!(
            map.contains_key("my-sglang"),
            "SGLang provider must be registered"
        );
        assert_eq!(map["my-sglang"].name(), "sglang");
    }

    #[test]
    fn build_providers_gemini_activates() {
        use gadgetron_core::config::{AppConfig, ProviderConfig, ServerConfig};
        use gadgetron_core::routing::RoutingConfig;
        use std::collections::HashMap;

        let mut providers = HashMap::new();
        providers.insert(
            "my-gemini".to_string(),
            ProviderConfig::Gemini {
                api_key: "key".to_string(),
                models: vec!["gemini-pro".to_string()],
            },
        );
        let cfg = AppConfig {
            server: ServerConfig {
                bind: "0.0.0.0:8080".to_string(),
                api_key: None,
                request_timeout_ms: 30_000,
            },
            router: RoutingConfig::default(),
            providers,
            nodes: vec![],
            models: vec![],
            web: gadgetron_core::config::WebConfig::default(),
            agent: gadgetron_core::agent::AgentConfig::default(),
        };
        let map = build_providers(&cfg).expect("Gemini provider must now be implemented");
        assert!(
            map.contains_key("my-gemini"),
            "Gemini provider must be registered"
        );
        assert_eq!(map["my-gemini"].name(), "gemini");
    }

    // ------------------------------------------------------------------
    // S6-2 TDD: clap CLI parsing
    // ------------------------------------------------------------------

    /// S6-2-T2a: `serve --tui` flag is correctly parsed.
    ///
    /// `Cli::try_parse_from(["gadgetron", "serve", "--tui"])` must succeed and
    /// the parsed command must be `Commands::Serve { tui: true, ... }`.
    #[test]
    fn clap_parses_serve_with_tui() {
        let cli = Cli::try_parse_from(["gadgetron", "serve", "--tui"])
            .expect("parse must succeed for 'serve --tui'");
        match cli.command {
            Some(Commands::Serve {
                tui, config, bind, ..
            }) => {
                assert!(tui, "--tui flag must be true");
                assert!(config.is_none(), "config must be None (not provided)");
                assert!(bind.is_none(), "bind must be None (not provided)");
            }
            None => panic!("expected Some(Commands::Serve), got None"),
            _ => panic!("expected Commands::Serve"),
        }
    }

    /// S6-2-T2b: `serve --config /tmp/test.toml` is correctly parsed.
    ///
    /// The `config` field must be `Some(PathBuf::from("/tmp/test.toml"))`.
    #[test]
    fn clap_parses_serve_with_config() {
        let cli = Cli::try_parse_from(["gadgetron", "serve", "--config", "/tmp/test.toml"])
            .expect("parse must succeed for 'serve --config /tmp/test.toml'");
        match cli.command {
            Some(Commands::Serve {
                config, tui, bind, ..
            }) => {
                assert_eq!(
                    config,
                    Some(PathBuf::from("/tmp/test.toml")),
                    "config path must match"
                );
                assert!(!tui, "--tui must default to false");
                assert!(bind.is_none(), "bind must be None");
            }
            None => panic!("expected Some(Commands::Serve), got None"),
            _ => panic!("expected Commands::Serve"),
        }
    }

    /// S6-2-T2c: No subcommand → `cli.command` is `None`.
    ///
    /// The default behaviour (no subcommand given) must yield `None` so that
    /// `serve_from_matches(None)` can apply the `Serve` defaults.
    #[test]
    fn clap_defaults_to_serve() {
        let cli =
            Cli::try_parse_from(["gadgetron"]).expect("parse must succeed with no subcommand");
        assert!(
            cli.command.is_none(),
            "no subcommand must yield cli.command = None"
        );
    }

    /// S6-2-T2d: `serve --bind 0.0.0.0:9090` is correctly parsed.
    #[test]
    fn clap_parses_serve_with_bind() {
        let cli = Cli::try_parse_from(["gadgetron", "serve", "--bind", "0.0.0.0:9090"])
            .expect("parse must succeed for 'serve --bind 0.0.0.0:9090'");
        match cli.command {
            Some(Commands::Serve {
                bind, tui, config, ..
            }) => {
                assert_eq!(bind, Some("0.0.0.0:9090".to_string()));
                assert!(!tui);
                assert!(config.is_none());
            }
            None => panic!("expected Some(Commands::Serve), got None"),
            _ => panic!("expected Commands::Serve"),
        }
    }

    /// S6-2-T2e: Short flags `-c` and `-b` work as aliases.
    #[test]
    fn clap_parses_serve_with_short_flags() {
        let cli = Cli::try_parse_from([
            "gadgetron",
            "serve",
            "-c",
            "/etc/gdt.toml",
            "-b",
            "127.0.0.1:3000",
        ])
        .expect("parse must succeed for short flags");
        match cli.command {
            Some(Commands::Serve {
                config, bind, tui, ..
            }) => {
                assert_eq!(config, Some(PathBuf::from("/etc/gdt.toml")));
                assert_eq!(bind, Some("127.0.0.1:3000".to_string()));
                assert!(!tui);
            }
            None => panic!("expected Some(Commands::Serve), got None"),
            _ => panic!("expected Commands::Serve"),
        }
    }

    /// S6-2-T2f: All three flags combined parse correctly.
    #[test]
    fn clap_parses_serve_all_flags() {
        let cli = Cli::try_parse_from([
            "gadgetron",
            "serve",
            "--config",
            "/opt/gdt/gadgetron.toml",
            "--bind",
            "0.0.0.0:8080",
            "--tui",
        ])
        .expect("parse must succeed for all flags");
        match cli.command {
            Some(Commands::Serve {
                config, bind, tui, ..
            }) => {
                assert_eq!(config, Some(PathBuf::from("/opt/gdt/gadgetron.toml")));
                assert_eq!(bind, Some("0.0.0.0:8080".to_string()));
                assert!(tui);
            }
            None => panic!("expected Some(Commands::Serve), got None"),
            _ => panic!("expected Commands::Serve"),
        }
    }

    // ------------------------------------------------------------------
    // S7-1 TDD: clap parsing for tenant and key subcommands
    // ------------------------------------------------------------------

    /// S7-1-T5: `tenant create --name "acme"` parses correctly.
    #[test]
    fn clap_tenant_create() {
        let cli = Cli::try_parse_from(["gadgetron", "tenant", "create", "--name", "acme"])
            .expect("parse must succeed for 'tenant create --name acme'");
        match cli.command {
            Some(Commands::Tenant {
                command: TenantCmd::Create { name },
            }) => {
                assert_eq!(name, "acme", "tenant name must be 'acme'");
            }
            _ => panic!("expected Commands::Tenant {{ command: TenantCmd::Create }}"),
        }
    }

    /// S7-1-T6: `key create --tenant-id <uuid>` parses correctly with default scope.
    #[test]
    fn clap_key_create() {
        let uuid_str = "550e8400-e29b-41d4-a716-446655440000";
        let cli = Cli::try_parse_from(["gadgetron", "key", "create", "--tenant-id", uuid_str])
            .expect("parse must succeed for 'key create --tenant-id <uuid>'");
        match cli.command {
            Some(Commands::Key {
                command: KeyCmd::Create { tenant_id, scope },
            }) => {
                assert_eq!(
                    tenant_id.expect("tenant_id must be Some").to_string(),
                    uuid_str,
                    "tenant_id must match supplied UUID"
                );
                assert_eq!(
                    scope, "OpenAiCompat",
                    "default scope must be 'OpenAiCompat'"
                );
            }
            _ => panic!("expected Commands::Key {{ command: KeyCmd::Create }}"),
        }
    }

    /// S7-1-T6b: `key create` without `--tenant-id` is valid (no-db mode).
    #[test]
    fn clap_key_create_no_tenant() {
        let cli = Cli::try_parse_from(["gadgetron", "key", "create"])
            .expect("parse must succeed for 'key create' without --tenant-id");
        match cli.command {
            Some(Commands::Key {
                command: KeyCmd::Create { tenant_id, scope },
            }) => {
                assert!(
                    tenant_id.is_none(),
                    "tenant_id must be None when not provided"
                );
                assert_eq!(
                    scope, "OpenAiCompat",
                    "default scope must be 'OpenAiCompat'"
                );
            }
            _ => panic!("expected Commands::Key {{ command: KeyCmd::Create }}"),
        }
    }

    /// S7-1-T7: `key list --tenant-id <uuid>` parses correctly.
    #[test]
    fn clap_key_list() {
        let uuid_str = "550e8400-e29b-41d4-a716-446655440001";
        let cli = Cli::try_parse_from(["gadgetron", "key", "list", "--tenant-id", uuid_str])
            .expect("parse must succeed for 'key list --tenant-id <uuid>'");
        match cli.command {
            Some(Commands::Key {
                command: KeyCmd::List { tenant_id },
            }) => {
                assert_eq!(
                    tenant_id.to_string(),
                    uuid_str,
                    "tenant_id must match supplied UUID"
                );
            }
            _ => panic!("expected Commands::Key {{ command: KeyCmd::List }}"),
        }
    }

    /// S7-1-T8: `key revoke --key-id <uuid>` parses correctly.
    #[test]
    fn clap_key_revoke() {
        let uuid_str = "550e8400-e29b-41d4-a716-446655440002";
        let cli = Cli::try_parse_from(["gadgetron", "key", "revoke", "--key-id", uuid_str])
            .expect("parse must succeed for 'key revoke --key-id <uuid>'");
        match cli.command {
            Some(Commands::Key {
                command: KeyCmd::Revoke { key_id },
            }) => {
                assert_eq!(
                    key_id.to_string(),
                    uuid_str,
                    "key_id must match supplied UUID"
                );
            }
            _ => panic!("expected Commands::Key {{ command: KeyCmd::Revoke }}"),
        }
    }

    // ------------------------------------------------------------------
    // S7-2 TDD: init subcommand + provider flag + AppConfig::default
    // ------------------------------------------------------------------

    /// S7-2-T1: `gadgetron init` parses with default output path.
    ///
    /// `output` must default to `gadgetron.toml` and `yes` must default to `false`.
    #[test]
    fn clap_init_default() {
        let cli =
            Cli::try_parse_from(["gadgetron", "init"]).expect("parse must succeed for 'init'");
        match cli.command {
            Some(Commands::Init { output, yes }) => {
                assert_eq!(
                    output,
                    std::path::PathBuf::from("gadgetron.toml"),
                    "default output must be 'gadgetron.toml'"
                );
                assert!(!yes, "yes must default to false");
            }
            _ => panic!("expected Commands::Init"),
        }
    }

    /// S7-2-T2: `gadgetron init --output /tmp/test.toml` parses the custom path.
    #[test]
    fn clap_init_with_output() {
        let cli = Cli::try_parse_from(["gadgetron", "init", "--output", "/tmp/test.toml"])
            .expect("parse must succeed for 'init --output /tmp/test.toml'");
        match cli.command {
            Some(Commands::Init { output, yes }) => {
                assert_eq!(output, std::path::PathBuf::from("/tmp/test.toml"));
                assert!(!yes);
            }
            _ => panic!("expected Commands::Init"),
        }
    }

    /// S7-2-T3: `gadgetron serve --provider http://10.0.0.1:8100` parses correctly.
    ///
    /// The `provider` field must be `Some("http://10.0.0.1:8100")`.
    #[test]
    fn clap_serve_with_provider() {
        let cli = Cli::try_parse_from(["gadgetron", "serve", "--provider", "http://10.0.0.1:8100"])
            .expect("parse must succeed for 'serve --provider http://10.0.0.1:8100'");
        match cli.command {
            Some(Commands::Serve {
                provider,
                tui,
                config,
                bind,
                ..
            }) => {
                assert_eq!(
                    provider,
                    Some("http://10.0.0.1:8100".to_string()),
                    "provider endpoint must match"
                );
                assert!(!tui);
                assert!(config.is_none());
                assert!(bind.is_none());
            }
            _ => panic!("expected Commands::Serve"),
        }
    }

    /// S7-2-T4: `gadgetron init --yes` writes a TOML file to a temp directory.
    ///
    /// Verifies that `cmd_init` creates the file and that it contains required keys.
    #[test]
    fn init_creates_toml_file() {
        use std::fs;

        let dir = std::env::temp_dir().join(format!(
            "gadgetron_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        ));
        fs::create_dir_all(&dir).expect("failed to create temp dir");
        let output = dir.join("gadgetron.toml");

        cmd_init(&output, true).expect("cmd_init must succeed");

        assert!(output.exists(), "gadgetron.toml must exist after init");

        let content = fs::read_to_string(&output).expect("must read written file");
        assert!(
            content.contains("[server]"),
            "output must contain [server] section"
        );
        assert!(content.contains("bind"), "output must contain bind field");
        assert!(
            content.contains("[router.default_strategy]"),
            "output must contain router section"
        );
        assert!(
            content.contains("round_robin"),
            "output must contain round_robin strategy"
        );

        // Cleanup
        let _ = fs::remove_dir_all(&dir);
    }

    /// S7-2-T5: `gadgetron init` without `--yes` refuses to overwrite an existing file.
    ///
    /// When the file already exists and stdin is not a TTY (CI environment),
    /// `cmd_init` must exit without overwriting. The existing content must be preserved.
    #[test]
    fn init_refuses_overwrite_without_yes() {
        use std::fs;

        let dir = std::env::temp_dir().join(format!(
            "gadgetron_nowri_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        ));
        fs::create_dir_all(&dir).expect("failed to create temp dir");
        let output = dir.join("gadgetron.toml");

        // Write a sentinel to the file first.
        let sentinel = "# DO NOT OVERWRITE\nbind = \"sentinel\"\n";
        fs::write(&output, sentinel).expect("failed to write sentinel");

        // Call cmd_init without --yes. In CI stdin is not a TTY, so the overwrite
        // prompt receives no input → treated as "N" → file must remain unchanged.
        // We pipe in a no-op input by having no stdin hooked up (it will read "" → N).
        cmd_init(&output, false).expect("cmd_init must not error even when refusing overwrite");

        let after = fs::read_to_string(&output).expect("must still read file");
        assert_eq!(
            after, sentinel,
            "existing file content must not be modified when --yes is absent"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// S7-2-T6: `AppConfig::default()` produces bind address "0.0.0.0:8080".
    ///
    /// The built-in default must be safe-and-sensible: correct port, empty providers.
    #[test]
    fn app_config_default_has_bind_8080() {
        let cfg = AppConfig::default();
        assert_eq!(
            cfg.server.bind, "0.0.0.0:8080",
            "default bind must be 0.0.0.0:8080"
        );
        assert!(cfg.providers.is_empty(), "default providers must be empty");
    }

    /// Verify `redact_db_url` masks passwords correctly.
    #[test]
    fn redact_db_url_masks_password() {
        assert_eq!(
            redact_db_url("postgres://user:secret@localhost:5432/gadgetron"),
            "postgres://user:***@localhost:5432/gadgetron"
        );
        // URL with no password must pass through unchanged.
        assert_eq!(
            redact_db_url("postgres://localhost:5432/gadgetron"),
            "postgres://localhost:5432/gadgetron"
        );
    }

    // ------------------------------------------------------------------
    // S7-3 TDD: InMemoryKeyValidator, doctor, serve --no-db
    // ------------------------------------------------------------------

    /// S7-3-T1: `InMemoryKeyValidator` accepts any key hash.
    ///
    /// In no-db mode every incoming key hash must be accepted regardless of
    /// its value. The returned `ValidatedKey` must have all three scopes
    /// (OpenAiCompat, Management, XaasAdmin) so all routes are reachable.
    #[tokio::test]
    async fn in_memory_validator_accepts_any_hash() {
        use gadgetron_core::context::Scope;
        use gadgetron_xaas::auth::validator::{InMemoryKeyValidator, KeyValidator};

        let validator = InMemoryKeyValidator;

        // Validate a completely arbitrary string — must not return Err.
        let result = validator
            .validate("any_random_hash_value_123")
            .await
            .expect("InMemoryKeyValidator must accept any hash");

        // api_key_id and tenant_id should be nil UUIDs.
        assert_eq!(result.api_key_id, uuid::Uuid::nil());
        assert_eq!(result.tenant_id, uuid::Uuid::nil());

        // Must grant all three scopes.
        assert!(
            result.scopes.contains(&Scope::OpenAiCompat),
            "must include OpenAiCompat"
        );
        assert!(
            result.scopes.contains(&Scope::Management),
            "must include Management"
        );
        assert!(
            result.scopes.contains(&Scope::XaasAdmin),
            "must include XaasAdmin"
        );

        // invalidate must not panic.
        validator.invalidate("any_random_hash_value_123").await;
    }

    /// S7-3-T2: `gadgetron doctor` subcommand is parseable by clap.
    ///
    /// Verifies that the `Doctor` variant is wired into the `Commands` enum
    /// and that the optional `--config` flag defaults to `None`.
    #[test]
    fn clap_doctor() {
        // No --config flag → config must be None.
        let cli =
            Cli::try_parse_from(["gadgetron", "doctor"]).expect("parse must succeed for 'doctor'");
        match cli.command {
            Some(Commands::Doctor { config }) => {
                assert!(config.is_none(), "config must be None when not provided");
            }
            _ => panic!("expected Commands::Doctor"),
        }

        // With --config flag → config must be Some.
        let cli2 = Cli::try_parse_from(["gadgetron", "doctor", "--config", "/etc/gdt.toml"])
            .expect("parse must succeed for 'doctor --config'");
        match cli2.command {
            Some(Commands::Doctor { config }) => {
                assert_eq!(
                    config,
                    Some(PathBuf::from("/etc/gdt.toml")),
                    "config path must match"
                );
            }
            _ => panic!("expected Commands::Doctor with --config"),
        }
    }

    /// S7-3-T3: `gadgetron serve --no-db` is parseable by clap.
    ///
    /// Verifies that the `--no-db` flag is wired into the `Serve` variant and
    /// defaults to `false` when absent.
    #[test]
    fn clap_serve_no_db() {
        // Without --no-db: must default to false.
        let cli =
            Cli::try_parse_from(["gadgetron", "serve"]).expect("parse must succeed for 'serve'");
        match cli.command {
            Some(Commands::Serve { no_db, .. }) => {
                assert!(!no_db, "--no-db must default to false");
            }
            _ => panic!("expected Commands::Serve"),
        }

        // With --no-db: must be true.
        let cli2 = Cli::try_parse_from(["gadgetron", "serve", "--no-db"])
            .expect("parse must succeed for 'serve --no-db'");
        match cli2.command {
            Some(Commands::Serve { no_db, .. }) => {
                assert!(no_db, "--no-db flag must be true when provided");
            }
            _ => panic!("expected Commands::Serve with --no-db"),
        }
    }

    /// S8-1-T1: `gadgetron tenant list` is parseable by clap.
    ///
    /// Verifies that `TenantCmd::List` is wired into the enum and that the bare
    /// subcommand (no flags) parses without error.
    #[test]
    fn clap_tenant_list() {
        let cli = Cli::try_parse_from(["gadgetron", "tenant", "list"])
            .expect("parse must succeed for 'tenant list'");
        match cli.command {
            Some(Commands::Tenant {
                command: TenantCmd::List,
            }) => {}
            _ => panic!("expected Commands::Tenant {{ command: TenantCmd::List }}"),
        }
    }
}
