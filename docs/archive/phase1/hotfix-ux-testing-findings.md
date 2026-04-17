# Hotfix: UX issues found during manual testing

> **Status**: Design note
> **Scope**: 2 small UX fixes (no API changes, no new types)
> **Source**: Manual testing by user on 2026-04-13

## Context
Manual QA testing of Sprint 1-9 release found 2 minor UX issues. This is a hotfix, not a full sprint.

## Fix 1: `doctor` Provider check output misleading

### Current
```
[PASS] Provider gemma4:   http://10.100.1.5:8100 reachable (404 in 26ms)
```

User sees "404" and assumes failure, even though `[PASS]` indicates success. The 404 comes from `GET /` on a vLLM endpoint — vLLM only serves `/v1/*` routes, so `/` returns 404 but the server is reachable. Our reachability test is "TCP connection works and server responds to any HTTP request," not "endpoint returns 200."

### Fix
Change `cmd_doctor::check_provider_reachable()` in `crates/gadgetron-cli/src/main.rs`:
- Replace "(404 in 26ms)" format with "(reachable in 26ms)" regardless of HTTP status
- Only report actual failures (connection refused, timeout, DNS) as FAIL
- HTTP 2xx/3xx/4xx/5xx responses are all "reachable" for doctor's purposes

### Acceptance
```
[PASS] Provider gemma4:   http://10.100.1.5:8100 reachable in 26ms
```
Endpoint URL is preserved (operators need it).

## Fix 2: no-db warning duplicate "WARNING:" prefix

### Current
```
WARNING: Running without database — keys not validated, quota disabled
2026-04-13T02:38:53.752682Z  WARN gadgetron: WARNING: Running without database — keys not validated, quota disabled mode="no-db"
```

The "WARNING:" prefix appears twice:
1. `eprintln!` includes "WARNING:" (redundant — stderr channel already implies warning)
2. `tracing::warn!` includes "WARNING:" (redundant — WARN level already set)

### Fix
Keep **both** outputs (eprintln! ensures visibility when `RUST_LOG=off` silences tracing). Strip "WARNING:" prefix from both:
- `eprintln!("Running without database — keys not validated, quota disabled")`
- `tracing::warn!(mode = "no-db", "running without database — keys not validated, quota disabled")`

### Acceptance
```
Running without database — keys not validated, quota disabled
2026-04-13T... WARN gadgetron: running without database — keys not validated, quota disabled mode="no-db"
```
Two lines (intentional for RUST_LOG=off fallback), no redundant "WARNING:" prefix.

## Test plan
1. `cargo build --release -p gadgetron-cli`
2. `./target/release/gadgetron doctor` → verify no "404" in reachable line
3. `./target/release/gadgetron serve --no-db --provider http://10.100.1.5:8100` → verify single WARN line

## Non-goals
- `/v1/models` auth requirement stays (OpenAI compat)
- `x-request-id` header already works (verified)
- No new tests (changes are in CLI output format only)
