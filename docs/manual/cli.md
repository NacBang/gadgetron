# CLI Reference
`gadgetron` is the operator-facing command-line entry point for starting the server, checking local state, and managing tenants, keys, users, teams, wiki indexing, and Gadget stdio flows. This page is the canonical reference for the shipped CLI surface and the current placeholder verbs.

Version and scope: this reference matches workspace version `0.5.14`. It covers command purpose, flags, stdout and stderr behavior, exit codes, database expectations, and the environment variable resolution order used by the CLI.

---

## Serve
### `gadgetron serve`
Start the HTTP gateway, and start the TUI when requested; this is the default action when no verb is supplied.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `-c`, `--config` | path | `GADGETRON_CONFIG`, else `./gadgetron.toml` | no | Path to the config file. |
| `-b`, `--bind` | string | `GADGETRON_BIND`, else config, else `0.0.0.0:8080` | no | Bind address for the HTTP listener. |
| `--tui` | bool | `false` | no | Start the interactive TUI dashboard. |
| `--no-db` | bool | `false` | no | Force no-db mode even when `GADGETRON_DATABASE_URL` is set. |
| `--provider` | string | none | no | Quick-start provider endpoint. |

- Output: stdout prints the startup banner. stderr prints tracing logs and status lines.
- Exit codes: `0` on clean shutdown, `1` on runtime error, `2` on `--tui` pre-check failure when stdin or stdout is not connected to a TTY (the TUI requires both).
- DB requirement: optional.
- Security notes: `GADGETRON_DATABASE_URL` is wrapped as `Secret<String>` and is not logged.
- Example: `gadgetron serve --config ./gadgetron.toml --bind 127.0.0.1:8080`

## Init
### `gadgetron init`
Write an annotated baseline config file to disk.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `-o`, `--output` | path | `gadgetron.toml` | no | Output path for the generated config file. |
| `-y`, `--yes` | bool | `false` | no | Overwrite without prompting. |

- Output: stdout prints `Config written to <path>`.
- Exit codes: `0` on success, `1` on error.
- DB requirement: N/A.
- Example: `gadgetron init --output ./gadgetron.toml --yes`

## Doctor
### `gadgetron doctor`
Run pre-flight checks for local config, bind address, database URL presence, provider reachability, and the `/health` endpoint.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `-c`, `--config` | path | `GADGETRON_CONFIG`, else `./gadgetron.toml` | no | Path to the config file used for the checks. |

- Output: stdout prints a table with `[PASS]`, `[WARN]`, or `[FAIL]` for the five checks.
- Exit codes: `0` when every check is `PASS` or `WARN` (WARN-only runs still exit 0), `2` when any check reports `FAIL`.
- DB requirement: optional. The database check only verifies environment presence, it does not connect.
- Example: `gadgetron doctor --config ./gadgetron.toml`

## Tenant
### `gadgetron tenant create`
Create a tenant row that can own API keys.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `--name` | string | none | yes | Tenant display name. |

- Output: stdout prints `Tenant Created`, the tenant ID and name, and a next-step hint for `gadgetron key create`.
- Exit codes: `0` on success, `1` on error.
- DB requirement: required.
- Example: `gadgetron tenant create --name "acme"`

### `gadgetron tenant list`
List tenants visible in the current database.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `(none)` | N/A | N/A | no | This subcommand takes no flags. |

- Output: stdout prints a table with `ID`, `Name`, `Status`, and `Created`.
- Exit codes: `0` on success, `1` on error.
- DB requirement: required.
- Example: `gadgetron tenant list`

## Key
### `gadgetron key create`
Generate a new API key for a tenant, or generate a local no-db key when no tenant ID is supplied.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `--tenant-id` | UUID | none | no | Tenant ID for persisted key creation. Required in full database mode. |
| `--scope` | string | `OpenAiCompat` | no | Comma-separated scope list. See [Scope system](auth.md#scope-system). |

- Output: in full mode, stderr prints `API Key Created`, the raw key, tenant, and scopes. In no-db mode, stdout prints the raw key and a `curl` example.
- Exit codes: `0` on success, `1` on error.
- DB requirement: optional. Database-backed key creation needs a tenant ID and a live database. No-db mode does not.
- Security notes: the raw key is printed once and never stored. In full mode it is sent to stderr per SEC-M7.
- Example: `gadgetron key create --tenant-id 9f1c5a2e-8d4b-4f0d-b3a2-7c0e1f5b6d4e --scope OpenAiCompat,Management`

### `gadgetron key list`
List active keys for one tenant.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `--tenant-id` | UUID | none | yes | Tenant ID whose keys should be listed. |

- Output: stdout prints a table with `ID`, `Prefix`, `Kind`, `Scopes`, and `Created`.
- Exit codes: `0` on success, `1` on error.
- DB requirement: required.
- Security notes: only the key prefix, kind, and scopes are shown. The raw key and hash are never shown.
- Example: `gadgetron key list --tenant-id 9f1c5a2e-8d4b-4f0d-b3a2-7c0e1f5b6d4e`

### `gadgetron key revoke`
Revoke one API key by key ID.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `--key-id` | UUID | none | yes | Key ID to revoke. |

- Output: stdout prints `Key revoked: {id}`.
- Exit codes: `0` on success, `1` on error.
- DB requirement: required.
- Example: `gadgetron key revoke --key-id 2ddf6a51-3e4a-4870-9d55-8857d1f51f77`

## User
### `gadgetron user create`
Create a user in the default tenant for local auth and admin flows.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `--email` | string | none | yes | Login email address. |
| `--name` | string | none | yes | Display name. |
| `--role` | string | `member` | no | User role, one of `member`, `admin`, or `service`. |
| `--password-env` | string | none | no | Name of the environment variable that holds the password. Omit it for `service`. |

- Output: stdout prints `User created: {id}` with the email and role.
- Exit codes: `0` on success, `1` on error.
- DB requirement: required.
- Security notes: the password is read from the named environment variable, never from a CLI argument.
- Example: `USER_PASS='change-me-now' gadgetron user create --email alice@example.com --name "Alice Admin" --role admin --password-env USER_PASS`

### `gadgetron user list`
List users in the default tenant.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `(none)` | N/A | N/A | no | This subcommand takes no flags. |

- Output: stdout prints `Users in default tenant (N rows)` followed by rows in `status | id | role | email | name` order.
- Exit codes: `0` on success, `1` on error.
- DB requirement: required.
- Example: `gadgetron user list`

### `gadgetron user delete`
Delete one user by user ID.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `--user-id` | UUID | none | yes | User ID to delete. |

- Output: stdout prints `User deleted: {id}` on success.
- Exit codes: `0` on success, `1` on error. Deleting the last active admin returns a 409 conflict and exits non-zero.
- DB requirement: required.
- Example: `gadgetron user delete --user-id 1a4d1f65-4ef8-4dc4-ae51-7a44f522d1f9`

## Team
### `gadgetron team create`
Create a team in the default tenant.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `--id` | string | none | yes | Team ID in kebab-case, max 32 characters, `admins` is reserved. |
| `--display-name` | string | none | yes | Human-readable team name. |
| `--description` | string | none | no | Optional team description. |

- Output: stdout prints the team ID, display name, and description.
- Exit codes: `0` on success, `1` on error.
- DB requirement: required.
- Example: `gadgetron team create --id platform-ops --display-name "Platform Ops" --description "On-call operators"`

### `gadgetron team list`
List teams in the default tenant.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `(none)` | N/A | N/A | no | This subcommand takes no flags. |

- Output: stdout prints `Teams in default tenant (N rows)` followed by the team list.
- Exit codes: `0` on success, `1` on error.
- DB requirement: required.
- Example: `gadgetron team list`

### `gadgetron team delete`
Delete one team by team ID.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `--id` | string | none | yes | Team ID to delete. |

- Output: stdout prints `Team deleted: {id}`.
- Exit codes: `0` on success, `1` on error.
- DB requirement: required.
- Example: `gadgetron team delete --id platform-ops`

## Reindex
### `gadgetron reindex`
Rebuild or inspect the wiki semantic index, with incremental mode selected by default.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `-c`, `--config` | path | `GADGETRON_CONFIG`, else `./gadgetron.toml` | no | Path to the config file. |
| `--full` | bool | `false` | no | Scan all pages and rebuild all embeddings. |
| `--incremental` | bool | implicit | no | Force incremental mode explicitly. This mode is selected when no mode flag is passed. |
| `--dry-run` | bool | `false` | no | Report planned changes without writing embeddings. |
| `--verbose` | bool | `false` | no | Print per-page detail after the summary. |

- Output: stdout prints a summary table with `Mode`, `Dry run`, `Scanned`, `Re-embedded`, `Deleted`, and `Skipped`. With `--verbose`, it also prints per-page detail.
- Exit codes: `0` on success, `1` on error.
- DB requirement: required.
- Example: `gadgetron reindex --config ./gadgetron.toml --dry-run --verbose`

## Wiki
### `gadgetron wiki audit`
Audit wiki content for stale pages and missing frontmatter.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `-c`, `--config` | path | `GADGETRON_CONFIG`, else `./gadgetron.toml` | no | Path to the config file. |

- Output: stdout prints an audit report covering pages older than 90 days and pages missing frontmatter.
- Exit codes: `0` on success, `1` on error.
- DB requirement: optional. The command validates embedding config without opening a database connection.
- Example: `gadgetron wiki audit --config ./gadgetron.toml`

## Gadget
### `gadgetron gadget serve`
Run the canonical JSON-RPC 2.0 stdio Gadget server defined by ADR-P2A-10.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `-c`, `--config` | path | `GADGETRON_CONFIG`, else `./gadgetron.toml` | no | Path to the config file. |

- Output: the process reads JSON-RPC 2.0 requests from stdin, writes JSON-RPC 2.0 responses to stdout, and exits on EOF.
- Exit codes: `0` on EOF or clean shutdown, `1` on error.
- DB requirement: optional.
- Example: `gadgetron gadget serve --config ./gadgetron.toml`

### `gadgetron gadget list`
Print the current placeholder message for Gadget listing.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `(none)` | N/A | N/A | no | This subcommand takes no flags. |

- Output: stdout prints `gadget list: not yet implemented -- tracked in P2B per ADR-P2A-10 section CLI.`
- Exit codes: `0`.
- DB requirement: N/A.
- Example: `gadgetron gadget list`
> **Status**: P2B, not yet implemented.

## Bundle/Plug
### `gadgetron bundle install`
Print the current placeholder message for bundle installation.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `name` | string | none | yes | Bundle name to install. |

- Output: stdout prints `bundle install {name}: not yet implemented -- tracked in P2B per ADR-P2A-10 section CLI.`
- Exit codes: `0`.
- DB requirement: N/A.
- Example: `gadgetron bundle install example-bundle`
> **Status**: P2B, not yet implemented.

### `gadgetron bundle list`
Print the current placeholder message for bundle listing.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `(none)` | N/A | N/A | no | This subcommand takes no flags. |

- Output: stdout prints `bundle list: not yet implemented -- tracked in P2B per ADR-P2A-10 section CLI.`
- Exit codes: `0`.
- DB requirement: N/A.
- Example: `gadgetron bundle list`
> **Status**: P2B, not yet implemented.

### `gadgetron plug list`
Print the current placeholder message for plug listing.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `(none)` | N/A | N/A | no | This subcommand takes no flags. |

- Output: stdout prints `plug list: not yet implemented -- tracked in P2B per ADR-P2A-10 section CLI.`
- Exit codes: `0`.
- DB requirement: N/A.
- Example: `gadgetron plug list`
> **Status**: P2B, not yet implemented.

### `gadgetron install`
Forward to the `bundle install` placeholder alias.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `name` | string | none | yes | Bundle name to install through the alias. |

- Output: stdout prints the same placeholder text as `gadgetron bundle install {name}: not yet implemented -- tracked in P2B per ADR-P2A-10 section CLI.`
- Exit codes: `0`.
- DB requirement: N/A.
- Example: `gadgetron install example-bundle`
> **Status**: P2B, not yet implemented.

## Legacy Shim
### `gadgetron mcp serve`
Run the deprecated compatibility shim that warns and then forwards to `gadgetron gadget serve`.

| Name | Type | Default | Required | Meaning |
|---|---|---|---|---|
| `-c`, `--config` | path | `GADGETRON_CONFIG`, else `./gadgetron.toml` | no | Path to the config file. |

- Output: stderr emits a deprecation warning, then the process behaves like `gadgetron gadget serve`. stdout carries JSON-RPC 2.0 responses.
- Exit codes: `0` on EOF or clean shutdown, `1` on error.
- DB requirement: optional.
- Example: `gadgetron mcp serve --config ./gadgetron.toml`

## Environment and Precedence
The CLI resolves settings in one order for every command surface: CLI flags first, then environment variables, then `gadgetron.toml`, then built-in defaults.

| Priority | Source | Meaning |
|---|---|---|
| `1` | CLI flags | Highest priority. |
| `2` | Environment variables | Override config file values. |
| `3` | `gadgetron.toml` | Baseline operator config. |
| `4` | Built-in defaults | Used when no higher source provides a value. |

| Variable | Used by | Overridden by | Meaning |
|---|---|---|---|
| `GADGETRON_CONFIG` | `serve`, `doctor`, `reindex`, `wiki audit`, `gadget serve`, `mcp serve` | matching `-c`, `--config` flag | Config file path. |
| `GADGETRON_BIND` | `serve` | `-b`, `--bind` | HTTP bind address. |
| `GADGETRON_DATABASE_URL` | `serve` and all database-backed management commands | `--no-db` on `serve` only | PostgreSQL connection URL. Treated as a secret. |
| `GADGETRON_PROVIDER` | `serve` | `--provider` | Quick-start provider endpoint. |
| `RUST_LOG` | long-running commands such as `serve`, `gadget serve`, and `mcp serve` | no CLI flag | Tracing filter. Default: `gadgetron=info,tower_http=info`. |
| `<PASSWORD_ENV_VAR>` | `user create` | no CLI flag beyond the variable name in `--password-env` | Environment variable that holds the password value. |

For the first-run operator loop, including tenant and key creation, see [Quickstart step 6](quickstart.md#step-6--create-a-tenant-and-api-key). For scope strings used by `gadgetron key create --scope`, see [Scope system](auth.md#scope-system). For the recovery path when a database-backed command fails with a missing database URL, see [GADGETRON_DATABASE_URL is not set](troubleshooting.md#gadgetron_database_url-is-not-set).
