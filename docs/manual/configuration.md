# Configuration

Gadgetron is configured through three mechanisms: CLI flags, environment variables, and `gadgetron.toml`. When the same setting is supplied by more than one mechanism, the order of precedence from highest to lowest is:

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

`gadgetron init` writes a fully-annotated `gadgetron.toml` to the current directory. Every field is present with its default value and a comment explaining what it does and which environment variable overrides it. This is the recommended starting point for any new deployment.

```sh
./target/release/gadgetron init
./target/release/gadgetron init --output /etc/gadgetron/gadgetron.toml
./target/release/gadgetron init --yes
```

If the target file already exists and `--yes` is not passed, the command prompts before overwriting it. In non-interactive mode without `--yes`, it leaves the existing file unchanged and exits successfully.

After running `gadgetron init`, open the generated file and follow the inline comments. If you want a zero-config single-provider test instead, use `gadgetron serve --provider <URL>`.

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

# Maximum time to wait for a response from a provider, in milliseconds.
# Default: 30000 (30 seconds)
request_timeout_ms = 30000
```

`server.api_key` is parsed by the config loader but is not used by the server in Sprint 1-3. Leave it absent.

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

**Supported provider types as of Sprint 4:** `openai`, `anthropic`, `ollama`, `vllm`, `sglang`

**Not yet supported (will fail at startup):** `gemini`

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

#### vLLM

Available as of Sprint 4.

```toml
[providers.gemma4]
type = "vllm"

# (required) Full URL to your vLLM instance, including port.
# Override with an environment variable if needed: endpoint = "${VLLM_ENDPOINT}"
endpoint = "http://10.100.1.5:8100"

# (required) List of model IDs this provider can serve.
# Must match the model name as vLLM knows it (--served-model-name or default).
models = ["gemma-4-27b-it"]
```

vLLM does not require an API key when running in its default open mode. If your vLLM instance has `--api-key` configured, add `api_key = "${VLLM_API_KEY}"`.

#### SGLang

Available as of Sprint 4.

```toml
[providers.glm]
type = "sglang"

# (required) Full URL to your SGLang instance, including port.
endpoint = "http://10.100.1.110:30000"

# (required) List of model IDs this provider can serve.
models = ["glm-4-9b-chat"]
```

SGLang does not require an API key by default. For reasoning models such as GLM-5.1, Gadgetron forwards the `reasoning_content` field in the response if the model returns it. See [api-reference.md](api-reference.md) for the field description.

---

### Minimal working `gadgetron.toml`

The following file is the minimum configuration to serve requests through a single OpenAI provider. Copy it verbatim and substitute your API key.

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
