# Round 2 Cross-Review — chief-architect
**Date**: 2026-04-13
**Scope**: docs/design/phase2/{00,01,02}.md v2 + docs/adr/ADR-P2A-{01,02,03}.md
**Reviewer role**: Round 0 (scaffolding) + Round 3 (Rust idiom)

## Verdict
APPROVE WITH MINOR

---

## v1 Blocker Verification

| ID | Description | Status | Citation |
|----|-------------|--------|----------|
| A1 | kairos as LlmProvider (not new gateway dispatch) | APPROVED | 00-overview.md:64-66; 02-kairos-agent.md:193-244 — `KairosProvider` implements `LlmProvider`, registered in router provider map, gateway unchanged |
| A2 | Nested error variants `GadgetronError::Kairos { kind, message }` + `Wiki { kind, message }` | APPROVED | `gadgetron-core/src/error.rs:166-179` — both variants already landed in core with full `error_code`, `error_type`, `http_status_code`, `error_message` dispatch; `all_fourteen_variants_exist` test confirmed |
| A3 | Per-request MCP stdio (not long-lived daemon) | APPROVED | 00-overview.md:279-282 — explicit statement; 02-kairos-agent.md:595-627 — tempfile written per request, MCP server child exits with Claude Code |
| R3a | Owned `ClaudeCodeSession` — no Arc<Mutex> on stdin/stdout | APPROVED | 02-kairos-agent.md:270-273 — struct holds `config: Arc<KairosConfig>` (config sharing fine) and `request: ChatRequest` (owned); `run(mut self)` consumes self; stdin/stdout taken via `.take()` into local bindings, never shared |
| R3b | POSIX /bin/sh in shell snippets | APPROVED | 00-overview.md §4 quick-start uses `sh` not `bash`; spawn.rs uses `Command::new(&config.claude_binary)` (no shell) — no platform-specific shell shebang in code paths |
| R3c | toml config, not serde_yaml | APPROVED | 01-knowledge-layer.md:178 — `gray_matter` uses `features = ["toml"]`; 01:1178 `toml = { version = "0.8" }`; `KairosConfig` and `KnowledgeConfig` use `serde` + TOML example files; no `serde_yaml` anywhere |

All six v1 blockers: APPROVED.

---

## New Blockers (must fix before implementation)

### BLOCKER-1: Wrong type names imported in `event_to_chat_chunks`
- **Location**: `docs/design/phase2/02-kairos-agent.md:534`
- **Issue**: The code imports `gadgetron_core::provider::{Choice, Delta}` but the actual types in `gadgetron-core/src/provider.rs` are named `ChunkChoice` and `ChunkDelta`. `Choice` is the non-streaming `ChatResponse` choice (has `message: Message`, not `delta: ChunkDelta`). `Delta` does not exist at all. This will not compile.
- **Fix**: Replace `use gadgetron_core::provider::{Choice, Delta}` with `use gadgetron_core::provider::{ChunkChoice, ChunkDelta}` and update the construction sites: `choices: vec![ChunkChoice { index: 0, delta: ChunkDelta { role: None, content: Some(t), tool_calls: None, reasoning_content: None }, finish_reason: None }]`. The `ChunkDelta` struct has a fourth field `reasoning_content: Option<String>` (added in Phase 1 for SGLang) that must be initialized.

### BLOCKER-2: `uuid` dependency missing from `gadgetron-kairos/Cargo.toml`
- **Location**: `docs/design/phase2/02-kairos-agent.md:98-113` (Cargo.toml spec)
- **Issue**: `stream.rs` (02:538, 02:560) calls `uuid::Uuid::new_v4()` to generate chunk IDs. `uuid` is not listed in the `[dependencies]` block of the specified `gadgetron-kairos/Cargo.toml`. While `uuid` exists as a workspace dependency (`Cargo.toml:74`), it is not referenced in the crate's Cargo.toml. The crate will fail to compile.
- **Fix**: Add `uuid = { workspace = true }` to the `[dependencies]` section of `gadgetron-kairos/Cargo.toml` in the design doc. The workspace already has `uuid = { version = "1", features = ["v4"] }` so no workspace-level addition is needed.

### BLOCKER-3: `AsyncBufReadExt` trait not imported for `read_line`
- **Location**: `docs/design/phase2/02-kairos-agent.md:260, 348`
- **Issue**: `session.rs` calls `reader.read_line(&mut line)` on a `BufReader<ChildStdout>`. The import at line 260 is `use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader}`. The `read_line` method lives on the `AsyncBufReadExt` trait, not `AsyncReadExt`. This trait is absent from the import list. The code will not compile.
- **Fix**: Add `AsyncBufReadExt` to the import: `use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader}`.

---

## Recommendations (optional, prefix NIT-)

### NIT-1: `ClaudeCodeSession::mcp_config_file` initialized to `None` then immediately set
- **Location**: `docs/design/phase2/02-kairos-agent.md:273, 295-300`
- **Issue**: `mcp_config_file: Option<NamedTempFile>` is set to `None` in `new()` and then assigned in `run()` via `self.mcp_config_file = Some(mcp_tmp)`. Since `run` consumes `self`, the field could be `Option<NamedTempFile>` initialized directly at the call site inside `run`, avoiding the struct field entirely. The current design is not wrong but the struct field that starts as `None` and is only set inside `run` is unusual for a consuming method.
- **Recommendation**: Move `mcp_tmp` to a local variable inside `run` (already computed there). Remove `mcp_config_file` from the struct. Tempfile lifetime is held by the `try_stream!` closure's capture, which is correct and simpler.

### NIT-2: `feed_stdin` error kind mismatch
- **Location**: `docs/design/phase2/02-kairos-agent.md:412-422`
- **Issue**: `feed_stdin` maps serialize and write errors to `KairosErrorKind::AgentError { exit_code: -1, stderr_redacted: String::new() }`. A serialization or stdin-write failure is not an "agent error" (subprocess exiting non-zero) — it is a `SpawnFailed` or an internal error. The `-1` exit code conflation with a genuine agent error could confuse diagnostics.
- **Recommendation**: Use `KairosErrorKind::SpawnFailed { reason: e.to_string() }` for stdin write failures and serialization failures, since they occur before the subprocess has had a chance to respond.

### NIT-3: `rmcp` feature gate is inverted — `#[cfg(not(feature = "use-rmcp"))]` is the default
- **Location**: `docs/design/phase2/01-knowledge-layer.md:872-884`
- **Issue**: The MCP server code gates the `rmcp` path on `#[cfg(feature = "use-rmcp")]` and the manual fallback on `#[cfg(not(feature = "use-rmcp"))]`. But the `Cargo.toml` for `gadgetron-knowledge` (01:180-183) does NOT define a `use-rmcp` feature — only `unix-fs`. This means the manual fallback is always compiled (feature is never enabled). The design is workable — the manual path is intentionally the default for P2A — but this needs to be made explicit: either (a) define `use-rmcp` as a feature in `Cargo.toml` with `rmcp` behind it, or (b) drop the `#[cfg]` guards and simply use the manual path unconditionally for P2A, with a comment explaining rmcp is P2B+.
- **Recommendation**: For determinism: if the manual path is the P2A default, remove the dead `#[cfg(feature = "use-rmcp")]` block from the design doc and document `rmcp` as deferred to P2B. The dual-path `#[cfg]` approach requires `rmcp` always in `[dependencies]`, which pulls in the crate even when unused.

### NIT-4: `WikiError::Io` and `WikiError::Frontmatter` map to `GitCorruption` in `From<WikiError>`
- **Location**: `docs/design/phase2/01-knowledge-layer.md:1314-1343`
- **Issue**: The `From<WikiError> for GadgetronError` impl maps `WikiError::Io` and `WikiError::Frontmatter` to `WikiErrorKind::GitCorruption`. An I/O error (e.g., disk full, permission denied on a read) is semantically distinct from git corruption. It will map to HTTP 503, which is appropriate, but the error code `wiki_git_corrupted` is misleading for a plain I/O error.
- **Recommendation**: Add a `WikiErrorKind::Io { reason: String }` variant with HTTP 503 to carry I/O failures, OR accept the current mapping (both yield 503) but rename the message to "wiki storage error" rather than "wiki git repository error" so operators do not check the git repo when the real cause is disk space.

---

## Determinism Findings
Per "구현 결정론적" rule — items where implementation behavior is not uniquely specified:

- `docs/design/phase2/02-kairos-agent.md:208-209` — `chat()` non-streaming accumulator has `todo!("assemble ChatResponse with assembled content")`. The `ChatResponse` struct requires `id`, `object`, `created`, `model`, `choices`, and `usage`. None of these are specified. If `chat()` is called (non-streaming), the implementation will panic. The spec must either (a) specify the exact `ChatResponse` construction from accumulated chunks, including `usage: Usage::default()`, or (b) mark `chat()` as `unimplemented!` and note that kairos only supports `stream=true`.

- `docs/design/phase2/02-kairos-agent.md:409-410` — `feed_stdin` comment says "v2 assumes JSON `{"messages":[...]}` on stdin. If the behavioral test finds raw text is required instead, this function is rewritten." This is documented as an open item, but the ADR-P2A-01 "Verification result" table is fully PENDING. The `stdin_echo` fake_claude scenario in the test harness (02:1128-1136) asserts exact byte counts, which depend on the JSON format. Any deviation requires rewriting both the session code and the test. This open item is correctly tracked but blocks `session.rs` coding — the implementation cannot start until ADR-P2A-01 Part 2 is resolved.

- `docs/design/phase2/01-knowledge-layer.md:1192-1198` — `KnowledgeConfig::validate()` and `to_wiki_config()` have `/* ... */` bodies. The validation rules are stated in the doc comment (wiki_path parent writable, max_page_bytes [1, 100MiB], searxng_url http(s), search_timeout [1,60]) but the exact behavior on `wiki_path` that does not exist yet (which `kairos init` would create) is ambiguous: should `validate()` fail because the parent doesn't exist, or pass because `kairos init` will create it? This needs a concrete decision: validate that the *parent directory* of `wiki_path` is writable and exists, not `wiki_path` itself.

- `docs/design/phase2/01-knowledge-layer.md:1201-1215` — `autodetect_git_author` calls `git_config_get("user.name")` — the `git_config_get` helper is not defined anywhere in the spec. Its exact implementation (does it shell out to `git config --global user.name`? does it read `~/.gitconfig` via `git2`?) is unspecified. Both approaches have different failure modes. Specify: use `git2::Config::open_default()` and `.get_string("user.name")`, not a subprocess call, to avoid depending on `git` being on PATH for just this config lookup.

---

## Summary

**Overall**: The v2 design docs represent a substantial and disciplined response to Round 1 blockers. The core architecture is sound: kairos as `LlmProvider`, per-request MCP stdio, nested error variants in `gadgetron-core` — all three architectural pillars are now implemented in actual code (the `error.rs` is already in the codebase and passes `all_fourteen_variants_exist`). The security threat model (§15 in 02) is thorough, ADR-P2A-02 and ADR-P2A-03 are ACCEPTED, and the `kill_on_drop` + stderr sink pattern correctly solves the deadlock problem from v1.

**Three blockers prevent implementation start.** All three are Rust compilation errors in the design-doc code snippets: (1) wrong type names `Choice`/`Delta` instead of `ChunkChoice`/`ChunkDelta` (plus missing `reasoning_content` field initialization), (2) `uuid` dependency missing from the crate's Cargo.toml despite being used in `stream.rs`, and (3) `AsyncBufReadExt` missing from the import list while `read_line` is called. These are mechanical errors that a code review would catch on first compile but must be corrected in the spec before implementation is handed off — per the "implementation determinism" rule, anyone implementing from this spec will hit the same compile errors.

**ADR-P2A-01 remains PENDING** (both the `--allowed-tools` enforcement test and the stdin contract verification). The spec correctly blocks kairos coding on this ADR but the verification must be performed before the next sprint starts. The `feed_stdin` format and the `stdin_echo` test scenario are both downstream of that result.

**Four nits and four determinism items** (two of which are spec gaps, two are open items already tracked). None of the nits are blockers. The determinism gap on `chat()` assembly and `git_config_get` should be resolved before coding to avoid implementation divergence across engineers.

**Readiness**: Fix the three blockers, resolve ADR-P2A-01, and this spec is ready for implementation. The document quality is high — the test plans are unusually concrete and complete.
