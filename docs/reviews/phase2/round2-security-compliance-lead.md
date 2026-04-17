# Round 2 Cross-Review — security-compliance-lead
**Date**: 2026-04-13
**Scope**: `docs/design/phase2/{00,01,02}.md` v2 + `docs/adr/ADR-P2A-{01,02,03}.md`
**Reviewer role**: Round 1.5 security + ADR re-review
**Review basis**: `docs/process/03-review-rubric.md §1.5-A`, OWASP LLM Top 10, OWASP ASVS Level 2

---

## Verdict

**REVISE**

v2 resolves all 10 SEC-1..SEC-10 blockers at the specification level. The ADRs are structurally sound. However, four new HIGH/CRITICAL-severity findings emerged from fresh STRIDE analysis on the v2 content that must be resolved before implementation begins.

---

## v1 Blocker Verification

| ID | Description | Status | Citation |
|----|-------------|--------|----------|
| SEC-1 | STRIDE threat model per component | **APPROVED** | `00-overview.md` §8 STRIDE table (5 components × 7 categories); `02-penny-agent.md` §15 (6 sub-components); trust boundary table B1-B6 present. Assets documented. Meets §1.5-A requirement. |
| SEC-2 | M4 — `--allowed-tools` enforcement verification (ADR-P2A-01 authored) | **APPROVED** | `ADR-P2A-01` specifies a concrete 5-step behavioral test, documented PASS/FAIL outcomes and fallback plan, and correctly blocks implementation until the test runs. The ADR is pending verification, which is the correct state — no code should precede it. |
| SEC-3 | M2 — stderr redaction, never echo to HTTP response | **APPROVED** | `02-penny-agent.md` §8 specifies `redact_stderr()` with 6 tight patterns (no catch-all). `§9` specifies `AgentError { stderr_redacted }` never appears in HTTP response body. Test `http_500_response_does_not_leak_stderr` enforces this end-to-end. `00-overview.md §12` error table confirms generic message only in HTTP 500. |
| SEC-4 | M1 — MCP config tempfile chmod 0600, lifetime bound | **APPROVED** | `02-penny-agent.md §7`: `NamedTempFile::with_prefix` uses `mkstemp(3)` which atomically creates at 0600. Redundant explicit `chmod` correctly removed (was misleading). `#[cfg(not(unix))] compile_error!` gate present. Drop removes file. Test `tmpfile_has_0600_permissions` verifies mode. Lifetime bound to subprocess: `NamedTempFile` owned by stream closure, dropped at end. |
| SEC-5 | `wiki_max_page_bytes` + M5 — size limits, generic git commit messages | **APPROVED** | `01-knowledge-layer.md §4.4` layer 1 size check; default 1 MiB in `config.rs`; `wiki_max_page_bytes` validated [1, 100 MiB]. Git commit message format hardcoded `"auto-commit: <page-name> <ISO8601>"` — never includes request_id or content. Test `commit_message_is_abstract` verifies format. |
| SEC-6 | M6 — audit log stores tool names only, not arguments | **APPROVED** | `02-penny-agent.md §6.2 stream.rs::event_to_chat_chunks` ToolUse arm logs `tool_name = %name` only via `tracing::info!`, discards `input` field. Test `tool_call_log_contains_name_not_args` enforces this. `00-overview.md §8` M6 defines `tools_called: Vec<String>`. |
| SEC-7 | Manual warning re --dangerously-skip-permissions (pre-merge gate) | **APPROVED** | `00-overview.md §10` specifies both Disclosure 1 (git permanence) and Disclosure 2 (SearXNG) as verbatim-locked text. ADR-P2A-03 designates `docs/manual/penny.md` as a blocking pre-merge gate. ADR-P2A-02 §Consequences requires explanation of `--dangerously-skip-permissions` in the manual. Gate mechanism documented. **Note: `docs/manual/penny.md` does not yet exist** — this is correct per the spec (written before first code PR, not before design review). |
| SEC-8 | §10 GDPR + SOC2 compliance mapping | **APPROVED WITH OBSERVATION** | `00-overview.md §10` covers: GDPR P2A (user = controller, no DPA), GDPR P2C DPIA requirement called out, SOC2 CC6.1 / CC6.6 / CC7.2 mapped. Observation: CC6.2 (security awareness) and CC6.3 (access control changes) are not mapped; CC9.2 (vendor risk) is not mentioned for `rmcp` / `git2` supply chain. These are gaps but not blockers for P2A single-user. Flagged as NIT-1. |
| SEC-9 | M3 — proptest corpus for wiki path escape | **APPROVED** | `01-knowledge-layer.md §10.3` provides a 7-category `traversal_strategy()` proptest covering raw `..`, URL-encoded variants, absolute paths, null bytes, Unicode NFC/NFD, Windows UNC, and mixed traversal with valid prefix. Positive property `resolve_path_accepts_valid_names` also present. The NFC/NFD comment in `resolve_path` correctly documents kernel-level handling (Linux treats `%2e%2e` as literal, not decoded). |
| SEC-10 | P2C reopen security threat model tag | **APPROVED** | `[P2C-SECURITY-REOPEN]` tags appear in: `02-penny-agent.md §10 config.rs` (ANTHROPIC_BASE_URL), `01-knowledge-layer.md §5.2 searxng.rs` (SSRF), `ADR-P2A-02 §Non-applicability`, `ADR-P2A-03 §For P2C`, `02-penny-agent.md §15.5`. The tag is present in all five critical locations identified in SEC-10. |

---

## New Blockers

### BLOCKER-1: Subprocess environment inherits full parent env — PATH manipulation and secret leak vector
- **Location**: `02-penny-agent.md §5.1 spawn.rs::build_claude_command` (line ~434-465)
- **Issue**: `tokio::process::Command::new(&config.claude_binary)` inherits the parent process environment by default. The spec only sets `ANTHROPIC_BASE_URL` conditionally; it does not call `env_clear()` or restrict the inherited environment in any way. This means all environment variables from the `gadgetron serve` process — including `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `DATABASE_URL`, `AWS_ACCESS_KEY_ID`, `CARGO_REGISTRY_TOKEN`, `SSH_AUTH_SOCK`, and any other secrets present in the operator's shell — are silently inherited by Claude Code and visible to it. Claude Code may log these in its own diagnostics, expose them in stderr (which, while redacted by `redact_stderr`, is not fully sanitized for all secret shapes), or surface them to the model context if the model requests environment introspection. Additionally, `PATH` inheritance allows Claude Code to resolve tool names from the ambient system PATH rather than a controlled set. If an attacker has placed a malicious binary named `gadgetron` earlier on `PATH`, the `gadgetron mcp serve` invocation in the MCP config would resolve to it.
- **STRIDE category**: I (Information Disclosure) + E (Elevation of Privilege)
- **Fix required**: `build_claude_command` must call `cmd.env_clear()` and then re-add only the required variables:
  - `HOME` (required for `~/.claude/` session resolution)
  - `PATH` must be an **explicit allowlist** (e.g. `/usr/local/bin:/usr/bin:/bin`) or the absolute path of `claude` must be passed rather than relying on PATH lookup
  - `ANTHROPIC_BASE_URL` (conditional, already handled)
  - `TMPDIR` (needed for tempfile creation in the subprocess)
  - Any other variables required by the `claude` binary (e.g., `LANG`, `LC_ALL` for correct UTF-8 handling)
  - All other variables, including `*_API_KEY`, `*_TOKEN`, `DATABASE_URL`, `SSH_AUTH_SOCK`, must be explicitly excluded
- Add test: `build_claude_command_env_does_not_inherit_api_key` — sets `ANTHROPIC_API_KEY=sk-test-123` in the test environment, spawns `fake_claude` with scenario `stdin_echo`, asserts the env var is absent from fake_claude's environment (fake_claude prints its env vars on request).
- **Severity**: HIGH
- **Spec location to update**: `02-penny-agent.md §5.1 spawn.rs` + `00-overview.md §8 Appendix B` (subprocess environment section currently says "Everything else default" which must change to "env_clear + explicit allowlist")

---

### BLOCKER-2: Audit entry schema lacks `request_id` — forensic correlation impossible
- **Location**: `00-overview.md §8` audit logging section (line ~492-497); `02-penny-agent.md §6.2` (tracing events)
- **Issue**: The specified audit fields are `penny_dispatched: bool`, `tools_called: Vec<String>`, and `subprocess_duration_ms: i32`. The observability section (§11) mentions "Log Claude Code stderr at debug level with `request_id` correlation tag" in a tracing span, but there is no `request_id` field in the persisted `AuditEntry` struct extension. The `AuditEntry` extended for penny cannot be correlated with the gateway's HTTP request audit record or with the tracing span. If a `wiki_write_secret_suspected` audit event fires, there is no way to identify which HTTP request, which user session, or which timestamp triggered it without parsing unstructured log lines. This violates the SOC2 CC7.2 anomaly triage requirement the spec claims to satisfy. Furthermore, the `tools_called` log in `stream.rs` is a `tracing::info!` event, not a persisted audit entry — the spec never specifies how `tools_called` names flow from the tracing event into the `AuditEntry.tools_called: Vec<String>` field. This mechanism is missing.
- **Fix required**:
  1. Add `request_id: String` to the penny `AuditEntry` extension fields in `00-overview.md §8` and in whatever code spec accumulates them
  2. Specify the exact mechanism by which `tools_called` values are accumulated: the spec in `02-penny-agent.md §6.2` uses `tracing::info!` only; either an in-memory accumulator in `ClaudeCodeSession` collects tool names and writes them to `AuditEntry` at session end, OR a structured tracing subscriber extracts them. The current spec leaves this gap.
  3. Add a test: `audit_entry_contains_request_id_and_tool_names` that verifies a round-trip: send a request, assert the `AuditEntry` persisted to the audit writer includes both `request_id` and the tool names called.
- **Severity**: HIGH

---

### BLOCKER-3: `claude_binary` config field enables arbitrary binary execution — argv injection not fully mitigated
- **Location**: `02-penny-agent.md §10 config.rs::PennyConfig::validate()` (line ~863-891); `02-penny-agent.md §5.1 spawn.rs`
- **Issue**: `PennyConfig.claude_binary` is a `String` with validation `if self.claude_binary.is_empty() { return Err(...) }`. The spec validates `claude_model` for leading `-` (F1 mitigation) but does NOT validate `claude_binary` for: (a) shell metacharacters (`;`, `|`, `&&`, `$()`, backticks, `\n`) that could inject shell commands if the binary path is ever passed through a shell-level executor, (b) path traversal in the binary path itself (e.g. `claude_binary = "../../etc/cron.daily/malicious"`), (c) relative paths that resolve against CWD rather than an absolute path. While `tokio::process::Command::new()` does not invoke a shell and so metacharacter injection in the `argv[0]` position is not directly exploitable via shell, a relative path allows an attacker with filesystem write access to plant a malicious binary that `which` resolves ahead of the real `claude`. More critically: the MCP config JSON hardcodes `"command": "gadgetron"` as the MCP server binary, meaning the entire `claude_binary` validation bypass discussion is moot for the MCP server, but the `claude_binary` path itself goes into `argv[0]` unvalidated beyond the empty-string check.

  The spec does validate `claude_model` for leading `-` but analogous validation is absent for `claude_binary`. An operator who sets `claude_binary = "--version"` in `gadgetron.toml` would cause unintended behavior that `validate()` does not catch.
- **Fix required**:
  1. `claude_binary` must be validated to be either an absolute path OR a path-free basename (no `/` in the string other than as a prefix indicator). If it contains `/`, it must be an absolute path starting with `/`. If it is a basename, it is resolved via `which` at startup and the resolved absolute path stored.
  2. `claude_binary` must be validated to contain no shell metacharacters (`;`, `|`, `&`, `$`, `` ` ``, `(`, `)`, `<`, `>`, `\n`, `\r`).
  3. Test `validate_rejects_relative_path_with_traversal` and `validate_rejects_shell_metachar_in_binary` to be added to the config test list.
- **Severity**: HIGH

---

### BLOCKER-4: `redact_stderr` regex for `generic_secret` has catastrophic backtracking risk
- **Location**: `02-penny-agent.md §8 redact.rs::REDACTION_PATTERNS` (line ~678-682)
- **Issue**: The `generic_secret` pattern is:
  ```
  (?i)(api[_-]?key|secret|token)\s*[:=]\s*[A-Za-z0-9+/]{20,}
  ```
  This contains `[A-Za-z0-9+/]{20,}` — an unbounded quantifier on a character class that also matches base64 `+` and `/`. On a pathological input like `"token = " + "A" * 10000`, the regex engine must explore O(n) states. Worse, when combined with the outer alternation `(api[_-]?key|secret|token)` and the `\s*[:=]\s*` separator, certain inputs can trigger worst-case backtracking in the `regex` crate. The `regex` crate uses a deterministic finite automaton (DFA) for most patterns, but the `(?i)` flag combined with the alternation can trigger NFA-mode for ambiguous matches.

  Claude Code stderr can be of arbitrary length (error dumps, OAuth diagnostic output, tool result logging). An adversarial wiki page or SearXNG result that injects content designed to appear in Claude Code's stderr could craft a string that causes `redact_stderr` to take seconds or minutes, effectively constituting a DoS on the Gadgetron server process (blocking the Tokio thread pool since regex is synchronous).

  Additionally, `[A-Za-z0-9+/]{20,}` will NOT match base64-encoded secrets that include `=` padding. A base64-encoded Anthropic key as `c2stYW50...` would slip through if the base64 does not contain `+` or `/` in the matched segment (though it likely does for long keys). The comment "can a base64-encoded secret slip by" in the review scope is validated: yes, a base64-wrapped Anthropic key that lacks `sk-ant-` prefix but encodes it would evade all patterns.
- **Fix required**:
  1. Add a maximum-length bound to the `generic_secret` pattern: replace `{20,}` with `{20,512}` to cap the scan length.
  2. Add a ReDoS proptest that generates strings of the form `"token = " + "A".repeat(n)` for n up to 50,000 and asserts `redact_stderr` completes in under 100ms.
  3. Add a comment documenting that base64-encoded secrets without a recognizable prefix are NOT caught by any pattern, which is a known limitation and accepted for P2A (the audit-only patterns in `wiki/secrets.rs` cover the write path separately).
- **Severity**: MED (DoS risk on Tokio thread pool; base64 evasion is LOW for P2A given single-user threat model)

---

## ADR Re-Review

| ADR | Still sound? | Adjustments needed |
|-----|--------------|-------------------|
| **P2A-01** | Structurally sound. The verification procedure is concrete and correct. The two-outcome decision tree (PASS/FAIL + fallback plan) is well-specified. The stdin contract question is properly flagged as an open item. | One gap: the ADR does not specify what happens if Claude Code version drift causes a previously-PASSing behavioral test to FAIL in a future release (regression). Add a "Claude Code version pinning" note: the ADR should specify that the penny implementation MUST record the minimum `claude` CLI version that passed verification (e.g. `>= 2.0.0`), and that CI should run `claude --version` and fail if the installed version is below this floor. This prevents a silent regression where `--allowed-tools` enforcement is removed in a future Claude Code release. Without this, the security posture degrades invisibly. |
| **P2A-02** | Sound for P2A single-user. The conditionality on ADR-P2A-01 PASS is correctly specified. The P2C non-applicability section is thorough. | The risk acceptance statement does not explicitly address what happens if the Gadgetron server process is running as a privileged user (e.g., root or a service account with broad filesystem access). In that deployment scenario, even with `--allowed-tools` enforcement, `wiki_write` can corrupt files beyond the wiki path if M3 fails (symlink race). Add a note: "This risk acceptance assumes `gadgetron serve` runs as a non-privileged OS user with no access to system files beyond `wiki_path` and `~/.claude/`. Running as root is explicitly unsupported and invalidates this risk acceptance." |
| **P2A-03** | Sound. The disclosure text is accurate, the opt-out mechanism is correctly specified (`searxng_url` unset removes tool from MCP list), and the P2C data segregation concern is properly flagged. | The ADR correctly identifies that SearXNG forwards query text to upstream engines (correcting the v1 inaccuracy). However, it does not disclose that SearXNG results include `url` and `snippet` fields from upstream engines, which may themselves contain tracking parameters or content designed to manipulate the model (prompt injection via search result). The ADR's scope is privacy disclosure, so this is outside its remit, but it should cross-reference the prompt-injection concern documented in `00-overview.md §8 STRIDE table` (SearXNG row, I = High) so the two documents compose cleanly. Add a sentence: "The adversarial content risk (prompt injection via search results) is documented separately in `docs/design/phase2/00-overview.md §8` and is out of scope for this privacy disclosure ADR." |

---

## Recommendations (NIT-)

**NIT-1** — SOC2 mapping is incomplete for supply chain.
`00-overview.md §10` maps CC6.1, CC6.6, CC7.2 but omits CC9.2 (vendor risk management), which applies to `rmcp`, `git2` (C library), and `reqwest`. Add a one-line entry: "CC9.2: new dependencies (`rmcp`, `git2`, `reqwest`) assessed via `cargo audit` / `cargo deny` gate (Phase 1 CI). `git2` pulls `libgit2` C library — CVE feed monitored quarterly per security policy."

**NIT-2** — `redact_stderr` is idempotent by design, but the property test checks only "output contains [REDACTED:" marker" for secret inputs, not idempotency directly.
`02-penny-agent.md §8` has `fn is_idempotent()` unit test listed but no proptest for idempotency. Add a proptest: `prop_redact_is_idempotent(s in "\\PC*") { assert_eq!(redact_stderr(&redact_stderr(&s)), redact_stderr(&s)); }`.

**NIT-3** — `SearxngClient::new` takes `config: &SearchConfig` (the full struct) but the nullability is controlled by `KnowledgeConfig.search: Option<SearchConfig>`. ADR-P2A-03 specifies `SearxngClient::new()` returns `Option<SearxngClient>` but `01-knowledge-layer.md §5.2` shows it returns `Result<Self, SearchError>`. These signatures are inconsistent. Reconcile: either `SearxngClient::new` takes `&SearchConfig` and returns `Result<Self>` (and the caller handles the `Option<SearchConfig>` before calling it — which is what `serve_stdio` in §6.1 does), or the ADR's signature is updated. The `serve_stdio` code in §6.1 is correct; the ADR description is misleading. Update ADR-P2A-03 §Implementation to say "the `Option<SearxngClient>` is handled at the `KnowledgeConfig` layer — `SearxngClient::new` itself takes a non-optional `SearchConfig`."

**NIT-4** — `Obsidian link parser` and prompt injection via link injection.
`01-knowledge-layer.md §4.6` specifies `parse_links` returns `Vec<WikiLink>`. The doc notes "Links inside code blocks are NOT parsed." This is good — but there is no specification for what happens when a wiki page `A.md` contains `[[B]]` and `B.md` contains prompt-injection content. The `wiki_get` tool fetches `B.md` contents verbatim. The `parse_links` function is not used in the `wiki_get` response — it is only used for `backlinks()`. So the link-injection-to-path-traversal concern from the review scope does not apply: `wiki_get` does not re-resolve links, it returns raw content. Documenting this non-issue explicitly in `§4.6` would help future reviewers. Add a comment: "`parse_links` is used only by `Wiki::backlinks()`. `wiki_get` returns the raw page body; it does not follow or resolve `[[links]]`. Prompt injection via link content is a model-layer concern (M8 risk acceptance), not a path-traversal concern."

**NIT-5** — `canonicalize_with_missing_tail` has a potential loop issue if `wiki_root` itself does not exist (e.g., a race condition between config validation and the first write). If `root` does not exist, `canonicalize(root)` fails at line 439, returning `WikiError::Io`, before `canonicalize_with_missing_tail` is called. This is fine. But if `wiki_root` is removed after initialization and before a write, the loop in `canonicalize_with_missing_tail` will walk up to `/` without finding an existing ancestor, then `existing.parent()` returns `None` when `existing == /`, and the function returns `Err(WikiError::path_escape(""))`. This is technically correct (the write fails safely) but the error kind (`PathEscape`) is semantically wrong for "root directory was deleted." Not a security issue, but a determinism / testability one. Flag as low-priority in open items.

---

## Determinism Findings

The following items are security-critical sections that contain residual ambiguity, violating the "implementation determinism" rule:

1. **`00-overview.md §8` Appendix B — subprocess environment is underspecified**: "Everything else default" at line ~818 is directly contradicted by BLOCKER-1. This is not merely an ambiguity — it creates a security-critical implementation choice that implementers will resolve incorrectly by default (inheriting all env vars). The spec must be explicit about what env vars are passed and what is excluded.

2. **`00-overview.md §8` audit logging — `tools_called` accumulation mechanism unspecified**: The spec lists `tools_called: Vec<String>` as an `AuditEntry` field but does not specify how values flow from the `tracing::info!(tool_name = %name)` event in `stream.rs` into that field. Two implementers would produce different code: one might use a Tokio channel, another a shared `Mutex<Vec<String>>` on the session struct. BLOCKER-2 covers this.

3. **`02-penny-agent.md §10 config.rs` — `claude_binary` validation underspecified**: Validation is `!is_empty()`. An implementer cannot determine from this spec whether relative paths, paths with `.`, or shell metacharacters are valid. BLOCKER-3 covers this.

4. **`01-knowledge-layer.md §7` `SearchConfig.searxng_url` — SSRF validation at config load time vs. runtime**: `KnowledgeConfig::validate()` specifies "searxng_url if present must be http(s):// with non-empty hostname" but does not specify whether IP address resolution happens at validation time or at request time. For SSRF protection, resolution at validation time is insufficient (DNS rebinding attack: URL resolves to public IP at validation, then rebinds to `169.254.169.254` at request time). For P2A with a user-configured local SearXNG, this is acceptable (noted by `[P2C-SECURITY-REOPEN]`), but the spec should state explicitly: "DNS resolution is NOT performed at validation time. SSRF protection at P2A relies on the operator configuring only a trusted local URL. DNS-rebinding SSRF mitigation is a P2C requirement."

---

## Summary

**v2 successfully resolves all 10 v1 blockers at the specification level.** The STRIDE models (per `00-overview.md §8` and `02-penny-agent.md §15`) meet the Round 1.5 requirements: assets, trust boundaries, and per-component threat tables are present and coherent. The three ADRs are structurally sound and appropriately scope the P2A risk acceptance.

**Four new blockers were identified through fresh STRIDE analysis on v2 content:**

BLOCKER-1 (HIGH) is the most consequential: the subprocess environment inherits all parent env vars, creating a silent credential exfiltration vector that bypasses all the carefully designed `--allowed-tools` / `redact_stderr` mitigations. Any Anthropic or cloud provider API key present in the operator's environment when they run `gadgetron serve` is silently visible to Claude Code. This must be fixed with `env_clear()` and an explicit allowlist before a single line of `spawn.rs` is written.

BLOCKER-2 (HIGH) is a forensic integrity gap: without `request_id` in the persisted `AuditEntry` and without a specified accumulation mechanism for `tools_called`, the audit log cannot be used for incident response. The spec's SOC2 CC7.2 claim cannot hold.

BLOCKER-3 (HIGH) addresses the `claude_binary` validation gap, which could allow an operator misconfiguration to execute an unintended binary or cause confusing failures.

BLOCKER-4 (MED) addresses a ReDoS risk in `redact_stderr`, which is on the hot path for every subprocess exit and could be triggered by adversarial SearXNG content appearing in Claude Code's stderr.

**Readiness for implementation**: NOT READY. The four blockers above must be addressed in v2.1 of the design docs before any `gadgetron-penny` or `gadgetron-knowledge` code is written. The ADR-P2A-01 behavioral verification (M4) also remains pending and must be completed before penny code starts. Once BLOCKER-1..4 are resolved and M4 verified, the security posture for P2A single-user local deployment is coherent and implementation may proceed.
