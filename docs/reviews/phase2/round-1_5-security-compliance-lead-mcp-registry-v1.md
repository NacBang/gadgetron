# Round 1.5 Security Review — `security-compliance-lead`
## `docs/design/phase2/04-mcp-tool-registry.md` Draft v1

| Field | Value |
|---|---|
| **Reviewer** | security-compliance-lead |
| **Review round** | Round 1.5 — security (§1.5-A checklist) |
| **Doc under review** | `docs/design/phase2/04-mcp-tool-registry.md` Draft v1 (PM authored 2026-04-14) |
| **Sibling reviews (same round)** | `docs/reviews/phase2/round-1_5-dx-product-lead-mcp-registry-v1.md` (BLOCK — 10 blockers) |
| **Drives** | ADR-P2A-05 detailed design, D-20260414-04 |
| **Review basis** | `docs/process/03-review-rubric.md §1.5-A`, OWASP Top 10 (2021), OWASP LLM Top 10 (2025), OWASP ASVS L2, CWE-362/364/367/400/444/502/611/862/918, RFC 4949 terminology |
| **Date** | 2026-04-14 |
| **Verdict** | **BLOCK** |

---

## §1.5-A Checklist Summary

| Item | Result | Findings |
|---|---|---|
| Threat model (STRIDE table, assets, boundaries) | **FAIL** | Doc has no §(threat model) section at all. 4 new trust boundaries created (approval IPC, tool dispatch, SSE emit, audit digest) are never enumerated. Inherited §8 of 00-overview does not cover them. |
| Trust boundary input validation | **FAIL** | `POST /v1/approvals/{id}` body is not spec'd with a validation schema; `rationale`/`description` flow from provider to approval card without a declared sanitization boundary. |
| Authn/authz — default-deny | **FAIL** | `Scope::AgentApproval` semantics collide with existing `/v1/*` → `OpenAiCompat` prefix rule in the scope guard middleware. Keys holding only `AgentApproval` cannot reach the endpoint under the current middleware, and any key holding `OpenAiCompat` can approve (widening, not narrowing). |
| Secret management | **FAIL** | CSPRNG for the 32-byte startup token is unspecified; `ANTHROPIC_API_KEY` env disclosure boundary (`claude --debug`, process inspection, `/proc`) is not analyzed. |
| Supply chain | PASS | No new non-workspace dependencies introduced by this doc (dashmap, uuid, oneshot, thiserror, async-trait, serde, sha2 all already in workspace). |
| Crypto (no rolling your own) | **FAIL (conditional)** | `args_digest: [u8; 32]` is declared as "sha256(sanitized_args_json)" but no sanitization specification is provided — cannot audit the digest domain. |
| Audit log append-only / tamper | **FAIL** | `purge_audit_log` is referenced but does not exist anywhere in the codebase or prior specs. Retention claims (`30d`/`90d`/`365d`) have no backing mechanism. `ToolApprovalRequested` creates a PII exfiltration path if `rationale_digest` sources a plaintext rationale before hashing. |
| Error info disclosure | FAIL (minor) | `McpError::Denied { reason }` is propagated verbatim to the agent's `tool_result`, which reaches the SSE stream → browser. A reason string sourced from the MCP server process may carry internal state; no redaction layer specified. |
| LLM-specific threats (prompt injection) | **FAIL** | `agent.*` reserved namespace enforcement relies on the PROVIDER honestly declaring its category AND its tool names. A compromised provider (or a provider with a typo) can circumvent. The existing Rust code uses `Result::Err` correctly; the DESIGN DOC TEXT still shows `assert!` which would panic-propagate under malicious input. |
| Compliance mapping (SOC2/GDPR/HIPAA) | FAIL | No compliance row. `ToolApprovalRequested` creates a new PII-bearing audit category; GDPR Art 32 disclosure obligations are unaddressed. T2/T3 retention (90d/365d) lacks rationale citation to a SOC2 control (CC6.1/CC7.2). |

---

## BLOCKER Findings

### SEC-MCP-B1 — `gadgetron mcp serve` cannot enqueue into an in-process `ApprovalRegistry`: the approval architecture is absent

**Finding.** The design doc's approval flow (§7, §8, §10, §6 L4) describes a single-process flow:

> "`ask` mode — MCP server enqueues a `PendingApproval` in the `ApprovalRegistry`, emits `event: gadgetron.approval_required` on the active chat SSE stream, then awaits a `oneshot::Receiver<ApprovalDecision>`."

But `gadgetron mcp serve` is a **separate OS process** — it is a CLI subcommand (`gadgetron-cli::McpCommand::Serve`) that is spawned by Claude Code, which is itself spawned by `gadgetron serve`. This is an explicit architectural commitment in `00-overview.md §8 B3` ("Claude Code → `gadgetron mcp serve` subprocess | Process boundary (grandchild of Gadgetron)"), and in `01-knowledge-layer.md §6.1` (`serve_stdio` takes `KnowledgeConfig` and consumes stdin/stdout).

The in-memory `ApprovalRegistry: DashMap<ApprovalId, PendingApproval>` lives in the gateway process. `gadgetron mcp serve` has **no path** to insert into that map, no path to emit to the SSE stream owned by the gateway, and no path to await a oneshot receiver created in the gateway. The three bodies of text in the doc (§7 code, §8 SSE schema, §10 endpoint) presume all four participants (MCP server, ApprovalRegistry, SSE emitter, HTTP endpoint) are in-process — they cannot be.

**Why this blocks.** This is a CWE-710 (Improper Adherence to Coding Standards / Architecture) and a defense-in-depth regression: L3 (MCP server gate) and L4 (approval gate) both depend on a mechanism that the doc does not define. Without a defined bridge, one of the following is forced at implementation time:

| Option | Implications for security |
|---|---|
| A. MCP server makes an HTTP callback to gateway | New endpoint. Authenticates how? Loopback + startup token (same mechanism as `brain.shim`)? What scope? Rate-limited how? Audit writes happen in which process? CWE-918 SSRF surface if misconfigured. |
| B. Unix domain socket IPC (MCP server → gateway) | Socket path discovery, permissions (0600), process ownership check. Linux/macOS only. CWE-863 if peercred is skipped. |
| C. Reverse-direction MCP: gateway hosts MCP server in-process | Contradicts 00-overview §2 (Rust code stays narrow) and 01-knowledge-layer §6.1 `serve_stdio`. Would require `gadgetron mcp serve` to be a shim that forwards stdio to an in-process server, which still leaves the subprocess boundary question. |
| D. MCP server responds to Claude Code on stdio; Claude Code asks the user via a separate client mechanism | Impossible under `-p` non-interactive mode. |
| E. Approval is bridged through Claude Code's own stdio tool_result mechanism | Claude Code is a black box here — it does not forward MCP tool responses to the gateway via any documented channel. |

None of these are specified, and each has materially different security properties. The doc cannot pass Round 1.5 without picking one and documenting its auth, transport, and failure modes.

**Secondary impact.** If Option A is chosen (the most plausible), then a new HTTP endpoint like `POST /internal/approvals/enqueue` exists inside the gateway — this is a second loopback-auth endpoint on top of `/internal/agent-brain/v1/messages` (§12). It needs the same hardening as `brain.shim`: loopback bind, startup token, recursion guard, rate limit, audit redaction. None of this is in the doc.

**Remediation.**
1. Pick the option and document it in a new §7.0 "Cross-process approval bridge" subsection.
2. Specify the transport (HTTP + bearer token, UDS, or pipe).
3. Specify the auth token source (shared with `brain.shim`? separate? rotation?).
4. Specify the wire schema (what does the MCP server send? what does it receive?).
5. Specify what happens when the MCP server cannot reach the gateway (fail-closed — tool returns `McpError::Execution("approval unreachable")`; fail-open is a hard NO).
6. Add the new trust boundary (B3a: MCP subprocess → gateway approval IPC) to the threat model section this doc is still missing (SEC-MCP-B2 below).
7. A unit test `gateway_mcp_bridge_rejects_missing_token` must be listed in §16.

**References.**
- `00-overview.md §8 B3` — grandchild process boundary
- `01-knowledge-layer.md §6.1` — `serve_stdio` stdio contract
- `02-kairos-agent.md §13` — `claude -p` invocation (no callback channel documented)
- CWE-923 Improper Restriction of Communication Channel
- CWE-918 SSRF (if loopback HTTP is the answer)

---

### SEC-MCP-B2 — Doc has no threat model section; §1.5-A checklist item 1 (threat model) cannot be marked N/A

**Finding.** `docs/process/03-review-rubric.md §1.5-A` requires: *"[ ] **위협 모델 (필수)** — STRIDE 6 카테고리... 자산·신뢰 경계·위협·완화가 표로 명시되어 있는가?"* The rubric explicitly states: *"단 위협 모델 항목은 N/A 불가"*.

This doc introduces FOUR new trust boundaries and SIX new assets that are not covered by `00-overview.md §8`:

**New trust boundaries:**
| ID | New boundary | Crosses | Not covered by 00-overview §8 |
|---|---|---|---|
| B7 | MCP subprocess → gateway ApprovalRegistry (SEC-MCP-B1) | Process or network | **New** |
| B8 | Browser → `POST /v1/approvals/{id}` | Network (bearer auth) | Partially (scope check) — new approval-specific bypass surface |
| B9 | Agent brain shim → router (`internal_call = true`) | In-process trust flag | **New** — tag spoofing not analyzed |
| B10 | Startup token in `ANTHROPIC_API_KEY` env → Claude Code process env → child process reads | Process env propagation | **New** — `claude --debug` disclosure not analyzed |

**New assets:**
- `PendingApproval.args` (may contain wiki content, search queries, user inputs)
- `PendingApproval.rationale` (agent output — UNTRUSTED per 00-overview §8 M8)
- `startup_token` (32-byte CSPRNG token for brain.shim)
- `ApprovalRegistry` state (oneshot channels — lost wakeup = DoS)
- `agent_tools` subcategory config (WriteToolsConfig fields — operator-only)
- `t3_rate_limit_counter` (counter state — must survive, or reset policy must be documented)

Without an explicit STRIDE table, reviewers cannot verify that Spoofing (T1/T2/T3 misclassification), Tampering (approval ID forgery), Repudiation (who approved what), Info disclosure (args_digest contents), DoS (approval channel flood), and Escalation (AgentApproval → OpenAiCompat via overlap) are all covered.

**Why this blocks.** The rubric forbids N/A on this item. The doc is structurally incomplete.

**Remediation.** Add a new §(N) "Threat Model (STRIDE)" section with:
1. Assets table (owner, sensitivity, lifetime, location)
2. Trust boundaries table (B7..B10 at minimum, plus inherited B1..B6)
3. STRIDE per new component (ApprovalRegistry, `POST /v1/approvals/{id}` handler, brain shim, cross-process bridge from SEC-MCP-B1)
4. Mitigations M9..M16 table bridging to code (test names, file paths)
5. A `[P2C-SECURITY-REOPEN]` tag for each assumption that breaks under multi-tenant (especially the single-global-ApprovalRegistry assumption — in multi-tenant it becomes a DashMap-per-tenant, with tenant isolation to verify)

Use the same format as `00-overview.md §8`.

---

### SEC-MCP-B3 — Reserved `agent.*` namespace enforcement uses `assert!` in the doc; panic is NOT the correct response to a potentially-compromised provider

**Finding.** Doc §13 specifies:

```rust
for tool in provider.tool_schemas() {
    let name = &tool.name;
    assert!(
        !name.starts_with("agent.")
            && !matches!(
                name.as_str(),
                "set_brain" | "list_brains" | "switch_model" | "read_config" | "write_config"
            ),
        "tool {name} is in the reserved 'agent.*' namespace (ADR-P2A-05 §14)"
    );
}
```

Three problems:

1. **`assert!` panics.** In Rust, `assert!` is a panic. In a long-running server process, this kills the `McpToolRegistry::register` call stack, and depending on whether the registration happens in `main()` or in a spawned task, this either kills the process or leaves it running with an inconsistent registry state. `ensure_tool_name_allowed` (which already exists in `crates/gadgetron-core/src/agent/tools.rs:173-191`) **correctly** returns `Result<(), McpError>`. The doc's `assert!` example contradicts the existing code. One of them is wrong.

2. **`category()` trust.** `provider.category()` returns `&'static str` and the doc text does not cross-check it against `ToolSchema.name`. A provider could declare `category() == "knowledge"` but register a tool named `agent.set_brain`. The current `ensure_tool_name_allowed(name, category)` does check both in isolation, but nothing cross-validates that `ToolSchema.name.split('.').next() == category`. A malicious provider can spoof its category.

3. **Case and Unicode bypass.** The match list is case-sensitive ASCII. A provider declaring `Agent.Set_Brain`, `ＡＧＥＮＴ.set_brain` (fullwidth), `agent.set_brain\u{200B}` (zero-width space), or `agent．set_brain` (fullwidth dot) may pass the check and be matched against a similar string elsewhere by a different normalization.

**Why this blocks.** CWE-183 (Permissive List of Allowed Inputs), CWE-602 (Client-Side Enforcement of Server-Side Security), and CWE-754 (Improper Check for Unusual or Exceptional Conditions). A panic on a prompt-injection-adjacent control is an availability regression, and the trust-the-category mistake is a classic control bypass.

**Remediation.**
1. Rewrite §13 enforcement code to use `Result::Err(McpError::Denied { .. })` — match the existing `ensure_tool_name_allowed` signature — not `assert!`. State explicitly: `McpToolRegistry::register` returns `Result<(), McpError>` and is fallible.
2. Add a new check: `tool.name.split_once('.')` → category must equal `provider.category()`. Specify the error variant.
3. Unicode normalize `tool.name` to NFKC and lowercase-compare against a fixed ASCII allowlist; reject anything containing non-ASCII or control characters in the name. Ban `.`, `_`, and alphanumerics only, matching the `--allowed-tools` format constraint from ADR-P2A-01.
4. Unit tests: `reserved_agent_namespace_nfkc_variants_rejected`, `provider_category_spoof_rejected`, `reserved_tool_zero_width_space_rejected`, `registry_register_returns_result_not_panic`.
5. Remove the `assert!` example entirely and replace with the fallible function.

**Cross-ref.** `crates/gadgetron-core/src/agent/tools.rs:173-191` already has the correct `Result` pattern. The doc text must match the code.

---

### SEC-MCP-B4 — `Scope::AgentApproval` vs existing `/v1/*` prefix-to-scope rule: collision, not addition

**Finding.** Doc §10 says: *"**Auth**: `Scope::OpenAiCompat` OR new `Scope::AgentApproval`"*.

The current scope guard middleware (`crates/gadgetron-gateway/src/middleware/scope.rs:31-42`) uses a **path-prefix → single-scope** mapping:

```rust
let required_scope: Option<Scope> = if path.starts_with("/v1/") {
    Some(Scope::OpenAiCompat)
} else if path.starts_with("/api/v1/xaas/") {
    Some(Scope::XaasAdmin)
} else if path.starts_with("/api/v1/") {
    Some(Scope::Management)
} else {
    None
};
```

Consequences under the current middleware:

| Key scopes | Can call `POST /v1/approvals/{id}`? | Can call `POST /v1/chat/completions`? |
|---|---|---|
| `[OpenAiCompat]` (default for `gad_live_*`) | **YES** (path is `/v1/*`) | YES |
| `[AgentApproval]` only | **NO** (middleware requires `OpenAiCompat` on `/v1/*`) | NO |
| `[OpenAiCompat, AgentApproval]` | YES | YES |

Three problems:

1. **`AgentApproval` alone is useless.** A dedicated approval-only client cannot reach the endpoint under the current middleware. This contradicts the doc's stated intent that `AgentApproval` is a separate capability.

2. **Bypass via `OpenAiCompat`.** Any client with `OpenAiCompat` (the default for all `gad_live_*` keys per `D-20260411-10`) can approve. A compromised downstream OAI-compat consumer (a CI job, an SDK with injected malicious code) can auto-approve pending tool calls. This is **widening**, not narrowing, the trust surface — exactly the opposite of what §14 ("agent cannot choose its own brain") is trying to achieve.

3. **Transitive call.** The doc says nothing prevents a client with `AgentApproval` only from being mistaken for `OpenAiCompat` if a future middleware change relaxes the prefix rule.

**Why this blocks.** CWE-287 (Improper Authentication), CWE-863 (Incorrect Authorization), CWE-266 (Incorrect Privilege Assignment). The doc creates a new scope without specifying how the middleware knows about it, and the "OR" logic is incompatible with the existing single-scope prefix dispatcher.

**Remediation.** Pick one:
- **Option 1 (split route):** Move `POST /v1/approvals/{id}` to `/api/v1/approvals/{id}` (under `Management`), and require scope `AgentApproval` as an ADDITIONAL check beyond `Management`. The middleware then gains a per-route override. Document the new rule in D-20260411-10 extension.
- **Option 2 (dual-scope middleware):** Change the scope guard to accept `Vec<Scope>` with OR semantics on specific routes. Explicitly list `/v1/approvals/{id}` as requiring `{OpenAiCompat, AgentApproval}` (either). Update `crates/gadgetron-gateway/src/middleware/scope.rs` and its tests.
- **Option 3 (reject `OpenAiCompat` on approval):** Require `AgentApproval` only, remove `OpenAiCompat` from the list. The doc's "OR" language must be retracted. Default `gad_live_*` keys do not get `AgentApproval`; only operator-issued `gadgetron-web` session keys do. Compromised OAI SDKs cannot approve. **This is the least-surprise secure default.**

Whichever is chosen, the doc must:
1. Specify the exact middleware change or route move.
2. Add test names: `approve_endpoint_requires_agent_approval_scope`, `approve_endpoint_rejects_openai_compat_only` (Option 3), `approve_endpoint_accepts_either_scope` (Option 2), and `approve_endpoint_at_api_path_not_v1` (Option 1).
3. Update the ADR-P2A-05 audit trail with the decision.

---

### SEC-MCP-B5 — Startup token CSPRNG source is unspecified; `rand::random()` (which defaults to `ThreadRng`) is **not** the correct choice for a security token

**Finding.** Doc §5 line 259-261:

> `# Token source. "startup_token" (default) = 32-byte random token generated at startup, passed to Claude Code via ANTHROPIC_API_KEY env, never persisted.`

and §12:

> "Auth: loopback-only bind + bearer token match (startup-generated 32-byte token). The token is memory-only, passed to Claude Code via subprocess env, rotated on every Gadgetron restart."

The doc **does not specify** the CSPRNG:

- `rand::random::<[u8; 32]>()` uses `ThreadRng`, which is derived from `OsRng` but keeps an in-process seed — this is acceptable for most cases but **not** documented as such.
- `rand::rngs::OsRng.fill_bytes(&mut buf)` (the pattern already used in `crates/gadgetron-xaas/src/auth/key_gen.rs:25`) is the correct, documented choice. It calls `getrandom(2)` on Linux, `SecRandomCopyBytes` on macOS, `BCryptGenRandom` on Windows.
- No explicit token format is specified — is it hex-encoded? base64? raw bytes? Claude Code's `ANTHROPIC_API_KEY` must be ASCII, so some encoding is mandatory.
- No unit test for token entropy is specified.

Related issue: **ANTHROPIC_API_KEY disclosure boundary**.

Claude Code CLI has a `claude --debug` mode that (per observed behavior on `claude 2.1.104` in ADR-P2A-01's test) logs extensive diagnostic information. The doc does not analyze:

1. Does `claude --debug` print `ANTHROPIC_API_KEY` to stderr? If yes, this goes straight to `KairosProvider::spawn` stderr capture, which goes to `redact_stderr()` (`02-kairos-agent.md` M2). But `redact_stderr` only strips `sk-ant-*` and `gad_*` patterns — a 32-byte hex-encoded random token matches NEITHER pattern and will leak into audit logs.
2. `/proc/$pid/environ` on Linux is readable by the same user (and root). For a single-user desktop this is in scope (same user = operator), but for Docker deployments (`00-overview.md` Deployment modes table, "Docker" row) the token is visible via `docker inspect` if env is not explicitly scrubbed.
3. Claude Code may pass env vars through to its own subprocesses (`gadgetron mcp serve` in particular). Does the MCP server process inherit `ANTHROPIC_API_KEY` from its grandparent gadgetron-serve → claude → gadgetron-mcp-serve? If yes, the token is now in 3 process environments and visible in 3 places in `/proc`.

**Why this blocks.** CWE-532 (Insertion of Sensitive Information into Log File), CWE-200 (Exposure of Sensitive Information to an Unauthorized Actor), CWE-338 (Use of Cryptographically Weak PRNG) if `ThreadRng` is not explicitly blessed with a rationale citation.

**Remediation.**
1. §5 explicit field spec: *"The token is generated by `rand::rngs::OsRng.fill_bytes(&mut [0u8; 32])` then hex-encoded (64 chars). Reference: same pattern as `gadgetron-xaas::auth::key_gen`."*
2. §12 add "**Disclosure boundary analysis**" subsection:
   - `redact_stderr` MUST extend its pattern list to include `gadgetron_brain_token_[0-9a-f]{64}` or similar (specify the exact pattern and add to `redact_stderr` test fixtures).
   - The token format should have a recognizable prefix (e.g. `gad_brain_<hex>`) so it matches `gad_*` existing redaction; specify this and update M2 pattern list in 00-overview §8.
   - `KairosProvider::spawn` MUST NOT propagate `ANTHROPIC_API_KEY` to `gadgetron mcp serve` subprocess. Use explicit `Command::env_clear().envs(allowlist)`. Spec the allowlist.
   - Docker deployment mode MUST document that `ANTHROPIC_API_KEY` is rotated on every container restart and recommends `docker inspect` audit in `deployment-operations.md`.
3. Unit tests: `startup_token_is_32_bytes_os_rng`, `mcp_serve_does_not_inherit_anthropic_api_key`, `redact_stderr_strips_startup_token`.
4. A risk acceptance entry in M8 (`00-overview.md §8`) for the `/proc/$pid/environ` single-user-local case.

**Cross-ref.** `crates/gadgetron-xaas/src/auth/key_gen.rs:25` (canonical OsRng pattern), `crates/gadgetron-kairos` (future — `redact_stderr` extension).

---

### SEC-MCP-B6 — `args_digest` sanitization domain is undefined; secret leakage via audit log or SSE payload is possible

**Finding.** Doc §10 audit schema:

```rust
ToolApprovalRequested {
    ...
    args_digest: [u8; 32],       // sha256(sanitized_args_json)
    rationale_digest: [u8; 32],  // sha256(rationale || "")
},
```

and §7:

```rust
pub struct PendingApproval {
    ...
    pub args: serde_json::Value,     // already sanitized for display
    pub rationale: Option<String>,   // agent's self-explanation
}
```

Six unresolved questions:

1. **Who sanitizes?** The MCP server process, the gateway, the frontend?
2. **What is the sanitization spec?** What constitutes "sanitized for display" for a JSON value whose schema is determined by the tool author? For `wiki.write`, args include `path` + `content` — neither can be "sanitized" without losing user intent.
3. **What if the user accidentally types their API key into a wiki page** and a `wiki.write` approval card renders `{ "content": "... sk-ant-api03-AAAA... ..." }`? The card renders it; the user approves (maybe auto-approves if `wiki_write = "auto"`); the sha256 goes to audit; the original plaintext reaches the browser over SSE.
4. **`rationale_digest`** is `sha256(rationale || "")`. The plaintext `rationale` is in `PendingApproval`, rendered on the approval card (§8), emitted over SSE (§8), and passed through `<MarkdownRenderer>` → DOMPurify (§8). The digest is only useful if it's the ONLY copy. It isn't.
5. **What goes into the actual SSE `data`?** The full `args` plaintext. §8 shows `"args": { "provider": "vllm/llama3" }` as plaintext, with a note "Args rendered via `<pre>` after sanitization (`JSON.stringify` with truncation at 2 KB)". 2 KB still leaks a 32-char API key.
6. **Audit field privacy.** `[u8; 32]` digests in audit logs satisfy SOC2 CC7.2 (forensic value: "was approval X for tool Y called?") but do NOT satisfy GDPR Art 32 (PII in `args` transiently exists in the gateway's memory, in the browser's DOM, in the SSE proxy logs, and in any frontend error reporter).

**Why this blocks.** CWE-532 (Logging sensitive info), CWE-312 (Cleartext storage of sensitive info), CWE-359 (Exposure of private personal info to an unauthorized actor). The design doc claims "already sanitized for display" without ever defining what that means, which is a classic hand-wave.

**Remediation.**
1. Add §7.1 "Sanitization contract" with:
   - Definition: "sanitized" means (a) the credential patterns in M5 (`00-overview.md §8`) are replaced with `<redacted>`, AND (b) string values longer than 256 chars are truncated to `<first-128>...<last-64>`, AND (c) nested objects/arrays deeper than 4 levels are replaced with `"<...>"`.
   - Enforcement location: `gadgetron-kairos::approval::sanitize_args(&mut serde_json::Value)` — called by the MCP server BEFORE `enqueue`.
   - Applies to `args` field in `PendingApproval`, the SSE payload, the `<ApprovalCard>` render path, AND the input to `args_digest`.
2. Clarify digest semantics: `args_digest = sha256(serde_json::to_vec(&sanitized_args))`. Document that the audit row alone is forensically insufficient to recover the plaintext — it is a fingerprint only.
3. `rationale` MUST go through the same sanitization (credential patterns + length cap), and the `rationale_digest` should be computed AFTER sanitization, same as `args_digest`.
4. Add a "do not store `args` plaintext beyond the PendingApproval lifetime" policy statement. PendingApproval is removed from the DashMap on decision → GC. No second copy anywhere.
5. Security test: `wiki_write_approval_redacts_anthropic_key`, `approval_sanitizer_truncates_large_string`, `rationale_digest_matches_sanitized_rationale`.
6. GDPR Art 32 note: "`ToolApprovalRequested.args_digest` is pseudonymous forensic data; the plaintext `args` is NEVER written to persistent storage. Single-user P2A. Multi-tenant P2C MUST re-evaluate — add a retention row for `args_full_payload` if operators require replay capability, subject to data subject access requests under Art 15."

---

### SEC-MCP-B7 — `ApprovalRegistry` race conditions: lost wakeups, TOCTOU timeout, out-of-order decide/await, and rate-limit counter is unbacked

**Finding.** Doc §7 code:

```rust
pub fn enqueue(&self, ...) -> (ApprovalId, oneshot::Receiver<ApprovalDecision>) {
    let (tx, rx) = oneshot::channel();
    let id = Uuid::new_v4();
    let pending = PendingApproval { id, ..., tx };
    self.pending.insert(id, pending);
    (id, rx)
}

pub fn decide(&self, id: ApprovalId, decision: ApprovalDecision) -> Result<(), ApprovalError> {
    let (_, pending) = self.pending.remove(&id).ok_or(ApprovalError::NotFound)?;
    pending.tx.send(decision).map_err(|_| ApprovalError::ChannelClosed)?;
    Ok(())
}

pub async fn await_decision(&self, id: ApprovalId, rx: oneshot::Receiver<ApprovalDecision>) -> ApprovalDecision {
    match tokio::time::timeout(self.timeout, rx).await {
        Ok(Ok(decision)) => decision,
        Ok(Err(_)) => ApprovalDecision::Timeout,
        Err(_) => {
            let _ = self.pending.remove(&id);
            ApprovalDecision::Timeout
        }
    }
}
```

Seven race conditions:

1. **Lost-wakeup TOCTOU on timeout.** The sequence (a) `await_decision` hits `tokio::time::timeout(...)` elapsed → returns `Err(Elapsed)` → the code enters `let _ = self.pending.remove(&id)` — but BEFORE `remove` acquires the dashmap shard lock, `decide()` on another thread has already taken the entry, sent the decision on `tx`, and returned `Ok(())`. The sent decision is lost because `rx` is already dropped by the timeout branch. Result: user clicked Allow, tool returns Timeout, user sees "Timeout" despite approving. CWE-362.

2. **`enqueue`/SSE ordering.** `enqueue()` inserts into the DashMap. The caller then must (a) emit the SSE event, and (b) call `await_decision`. If the frontend is fast (localStorage auto-approve for T2), the client POSTs `/v1/approvals/{id}` in the time window between insert and `await_decision` being called. `decide()` succeeds (entry found), `tx.send()` puts the decision in the channel, but if `rx` is dropped (e.g., the caller panics between `enqueue` and `await`), the decision is dropped with it. The user sees "approved" in the frontend, the tool sees "channel closed" → Timeout. CWE-820 (Missing Synchronization).

3. **Order of operations for `enqueue` is not specified.** §7 shows `enqueue()` inserting into the map, but does not specify that the SSE emission and `await_decision` MUST happen in a specific order. If SSE is emitted AFTER `await_decision` starts, and the client POSTs before the SSE event fires (fast localStorage path), the client has no `approval_id` to POST — because the SSE event carries it. But if SSE is emitted BEFORE `enqueue` inserts, the client POSTs and gets `404 Not Found`. There is a real, non-trivial ordering constraint that the doc does not describe.

4. **The dashmap shard lock in `remove` can block an arbitrary long time** if another thread holds the same shard for a write. `tokio::time::timeout` uses monotonic clock, but the ACTUAL resolution of the timeout branch into a removed-from-map state is not atomic.

5. **`decide()` idempotency.** A client double-POSTs `/v1/approvals/{id}` (retry logic, double-click on a slow connection). First POST succeeds → `remove` succeeds → `send` succeeds. Second POST → `remove` returns None → `NotFound` → 404. The frontend sees a 404 and may interpret this as "my first POST failed, retry". CWE-358.

6. **`rate_limit_remaining` storage is never specified.** `PendingApproval.rate_limit_remaining: Option<u32>` is passed in by the caller but the doc does not say WHERE the rate-limit counter lives, HOW it decrements on `enqueue` vs `decide`, whether it increments back on `Deny`/`Timeout`, or whether it resets. §5 V5 says `max_per_hour > 0` at validation — but the counter implementation is absent. In-process only? Per-tenant? Per-request? Survives restart? Clock drift — what if `SystemTime::now()` jumps backward between two counter checks?

7. **Heartbeat interference.** §7 "Heartbeat: While waiting for a decision, the Kairos provider emits `: keepalive\n\n` SSE comment frames every 15 seconds". If the heartbeat task and the `decide()` task race on the same SSE channel sender, tokio `mpsc` ordering is preserved, but if the channel is `broadcast` the order is not guaranteed. The doc does not say which one.

**Why this blocks.** These are not theoretical — an in-process approval channel that loses wakeups is a silent DoS vector (every T2 approval occasionally times out, giving the agent a false "user denied" signal, which teaches the LLM to avoid legitimate tools, degrading service). Under adversarial conditions (SEC-MCP-B8 flood), the race frequency increases and the DoS becomes reliable.

**Remediation.**
1. §7 rewrite the `await_decision` pattern to use a single removal path via a sentinel:
   - Replace the pending entry with one holding `Option<oneshot::Sender<_>>` under a `Mutex`, so that `decide()` and `await_decision` can atomically "claim" the sender before sending.
   - Or switch from `DashMap` + `oneshot` to a pattern like `DashMap<ApprovalId, watch::Sender<State>>` where state transitions are total.
   - Or use `DashMap<ApprovalId, Arc<Notify>>` + a `State` enum (Pending/Allowed/Denied/Timeout) so that `decide()` atomically CAS-writes the state and `await_decision` observes the state, eliminating the "message was sent but rx was dropped" class entirely.
2. Spec the exact operation order: `enqueue` MUST insert into the map BEFORE returning the `approval_id`; the caller then emits the SSE event in a tokio task WHOSE LIFETIME IS JOINED with `await_decision`; SSE emission failure is a hard error (ApprovalError::NotifyFailed). Document this as a §7.2 "Operation ordering" subsection.
3. Idempotency: `decide()` returning `NotFound` on a decided approval MUST be represented as a distinct error variant (`ApprovalError::AlreadyDecided`) so the frontend does not retry. 404 is still the HTTP code but the JSON body explains.
4. Rate limit: specify the storage (§12 new "Rate limiter" subsection):
   - In-process `DashMap<TenantId, RateWindow>` with `moka::sync::Cache` TTL eviction.
   - Window: fixed 1-hour, rolling or tumbling — specify tumbling with `UTC hour boundary` (avoids clock drift on DST).
   - Decrement on `enqueue` of T3 ONLY, increment back on `Deny`/`Timeout` (doc already says "rate limit counter 증가 안 함" on timeout — spec this as an invariant with a test).
   - Does NOT survive restart. Spec this: "restart resets T3 rate limit counter — this is a trade-off for P2A single-user". Add to M8 risk acceptance.
5. Tests (add to §16):
   - `approval_decide_after_timeout_is_lost_wakeup_free`
   - `approval_enqueue_then_immediate_decide_preserves_decision`
   - `approval_decide_idempotent_returns_already_decided`
   - `t3_rate_limit_counter_tumbles_on_hour_boundary`
   - `t3_rate_limit_restart_resets_counter_with_audit_warn`
   - `heartbeat_does_not_race_with_decide_via_mpsc`

**References.** CWE-362 TOCTOU, CWE-364 Signal Handler Race Condition, CWE-367 TOCTOU File Access, CWE-820 Missing Synchronization, CWE-833 Deadlock, tokio docs on `oneshot` cancel safety, `watch` vs `Notify` idioms.

---

### SEC-MCP-B8 — `POST /v1/approvals/{id}` rate limit (60/min/tenant) is insufficient against approval-flood DoS and audit-log flood

**Finding.** Doc §9: *"Rate limit: per tenant, 60 approvals/minute (DOS defense)"*.

Consider an attacker with a single valid `gad_live_*` API key (compromised SDK, stolen from a dev machine, leaked in a public repo):

1. **Audit-log flood.** Each `POST /v1/approvals/{id}` call either resolves an existing approval (64-byte audit row: `ToolApprovalGranted` or `ToolApprovalDenied`) OR returns 404 (still creates at least one middleware metric row + a `warn!` trace event). At 60/min sustained, that's 86,400 audit rows/day from ONE tenant. Multiply by concurrent attackers, multiply by the `ToolApprovalRequested` row that MAY exist for each (a malicious user can trigger a wiki_write approval via their own chat stream, generating `ToolApprovalRequested`, then flood the `POST` endpoint with random UUIDs to generate 60/min of 404 rows). SOC2 CC7.2 anomaly detection gets buried in noise. Storage billing spikes.

2. **Channel exhaustion of tokio mpsc audit writer.** `AuditWriter::send` (`crates/gadgetron-xaas/src/audit/writer.rs:51-56`) drops on full and increments `dropped: AtomicU64`. At 60/min * 10 attackers the channel depth may or may not saturate depending on configured capacity; this is not specified for approvals.

3. **OneshotSender drain of legitimate approvals.** If an attacker discovers or guesses a valid `approval_id` (UUIDv4, 122 bits of entropy — hard but not impossible via timing side-channels or a race against the SSE emit), a `decide()` with an attacker-chosen `Deny` silently nixes a legitimate user's in-flight approval. CWE-940 Improper Verification of Source of a Communication Channel.

4. **60/min is chosen without rationale.** What's the expected legitimate approval rate? If one user session generates at most ~1 approval/min (even burst mode), 60/min is 60x headroom. But if a `gadgetron-web` client with an active LLM conversation generates a burst of 20 approvals in 10 seconds (valid scenario: multi-step agent writing several wiki pages), the limit is too tight. The number is uncalibrated.

5. **Per-tenant, not per-key.** If a tenant has 10 API keys and one is compromised, the attacker gets 60/min — but a legitimate CI pipeline on another key in the same tenant is denied 60/min. CWE-770 Allocation of Resources Without Limits or Throttling (applied in the wrong direction).

**Why this blocks.** CWE-770 (DoS via resource allocation), CWE-400 (Resource exhaustion), CWE-837 (Improper Enforcement of a Single, Unique Action). The doc's "60/minute per tenant" is a magic number without a threat analysis.

**Remediation.**
1. §9 add a "Rate limit rationale" subsection:
   - Legitimate upper bound: 1 approval per 2 seconds for a single session = 30/min. Two concurrent sessions = 60/min. Document this bound.
   - Per-tenant-per-key: 30/min (not per-tenant, to prevent one compromised key from denying a tenant's other keys).
   - Per-tenant-aggregate: 120/min (4x legitimate headroom).
   - 404s count against the rate limit (attack surface: guessing IDs).
2. Anti-flood-by-404: after 10 consecutive 404s from one key in 1 minute, return `429 Too Many Requests` + audit `ToolApprovalBruteForce { api_key_id, count }` and `tracing::warn!`.
3. Approval ID entropy: specify UUIDv4 (122 bits) explicitly and require `rand::rngs::OsRng` (existing convention) — NOT `Uuid::new_v4()` which uses its internal RNG (usually fine but unverified for worst-case). Actually `Uuid::new_v4()` uses `getrandom` under the hood; document that explicitly with a crate version pin.
4. Audit-log rate limit: `ToolApprovalRequested` and `ToolApprovalGranted/Denied` audit emissions MUST be gated by the middleware's rate limit, not emitted for every 404 POST.
5. Tests: `approval_post_after_10_consecutive_404s_returns_429`, `approval_post_rate_limit_per_key_not_per_tenant`, `approval_id_entropy_is_122_bits`.
6. Specify the rate limiter backing store (same as SEC-MCP-B7 #4): in-process moka, tumbling 1-min window, does not survive restart.

---

### SEC-MCP-B9 — `purge_audit_log` is referenced but does not exist; T3 retention claim is unbacked

**Finding.** Doc §10 Retention:

> - `Read` (T1) — 30 days default
> - `Write` (T2) — 90 days minimum (SOC2 CC6.1)
> - `Destructive` (T3) — 365 days minimum, excluded from any `purge_audit_log` operation

A grep across the entire codebase and docs (`crates/`, `docs/`) finds **zero** other references to `purge_audit_log`. The existing `AuditEntry` struct (`crates/gadgetron-xaas/src/audit/writer.rs:6-18`) has no `tier`, `event_type`, or `retention_class` field. There is no DB migration, no cron job, no admin endpoint, and no ADR defining `purge_audit_log`.

**Why this blocks.** CWE-778 (Insufficient Logging), CWE-1263 (Improper Physical Access Control to Stored Data). The doc makes a compliance-relevant claim ("SOC2 CC6.1") that has no implementation path. If a reviewer (internal or external audit) asks "show me the T3 retention enforcement mechanism", the answer is "it doesn't exist, it's a future feature".

**Remediation.**
1. Either:
   - **Option A:** Remove the retention section from §10 and replace with: "**Retention is out of scope for this doc**. Audit retention is tracked in D-TBD (to be added after `xaas-platform-lead` specs `purge_audit_log` in a dedicated ADR). T3 audit rows are tagged with `retention_class = Critical` so the future purger can honor it."
   - **Option B:** Full spec in this doc (probably too much for a P2A design doc): define `purge_audit_log` as a new admin endpoint, define a DB migration adding `audit_entries.retention_class` column, define the purge algorithm, define the `admin-lead` runbook. This is ~2 weeks of work; recommend Option A.
2. Add `retention_class: RetentionClass` (enum: `Standard`, `Elevated`, `Critical`) to the `ToolAuditEvent` variants. Default `Standard` = 30d, `Elevated` = 90d, `Critical` = 365d. This is a FORWARD-COMPATIBLE annotation — it doesn't enforce retention itself but tags the rows for a future purger.
3. Specify that `AuditEntry` gains a `retention_class` field in a follow-on ticket, and this doc's only commitment is to populate it correctly on emission.
4. Compliance citation: only claim SOC2 CC6.1 if an audit control exists; otherwise remove the citation (honest > aspirational).

---

### SEC-MCP-B10 — `[agent.brain.shim]` recursion guard (`max_recursion_depth`) relies on a client-settable header; header spoofing analysis is absent

**Finding.** Doc §5 V12: *"`brain.shim.max_recursion_depth < 1` → error"* and §12: *"Recursion guard: `X-Gadgetron-Recursion-Depth` header; shim rejects requests with depth >= `agent.brain.shim.max_recursion_depth`"*.

The header is **set by the client** — Claude Code is the client. Claude Code receives a request from the gateway, calls back via `ANTHROPIC_BASE_URL`, and SOMETHING sets the header. Two questions:

1. **Who sets the header on the outgoing Claude Code → shim request?** Claude Code does not know about the Gadgetron recursion guard. The header must be SET by the gateway's shim handler on the TRANSLATED request (`router.chat_stream(translated_request, internal_call: true)`). But the shim handler is the thing RECEIVING the request from Claude Code, not the thing issuing it. The depth is per-call, not per-session, and the shim handler only sees the INBOUND header from the client — it cannot know what depth the outbound call will be unless it's explicitly tracked in request-context.

2. **Can a compromised in-process component spoof the header?** The `router.chat_stream(..., internal_call: true)` flag is passed by the shim to the router — it's Rust-to-Rust, in-process, no header crossing the trust boundary. But the header is ONLY meaningful on the INBOUND request from Claude Code. Claude Code is an external subprocess; it sends whatever headers the library puts on. If Claude Code's Anthropic client library ever adds a custom-header feature (or if the prompt injection attack makes Claude Code do `curl -H "X-Gadgetron-Recursion-Depth: 0" ...` via `Bash` tool — BLOCKED by `--allowed-tools` but still worth documenting), the header can be spoofed to 0 on every call.

3. **Default 2 is insufficient.** §5: "Default 2 (i.e., one brain call is allowed; a brain call that somehow re-enters Gadgetron and tries to call the brain again is not)". But with default 2, a single level of indirection IS allowed — so a prompt injection + a tool that forwards the injection to another brain call succeeds once. It takes a SECOND brain call to fail. The default should be 1 (no re-entry at all).

**Why this blocks.** CWE-441 (Unintended Proxy or Intermediary), CWE-345 (Insufficient Verification of Data Authenticity), CWE-290 (Authentication Bypass by Spoofing). The recursion guard is security-critical but the trust model of the header is unstated.

**Remediation.**
1. §12 add a subsection "Recursion depth header trust model":
   - The header is **loopback-only**; the shim rejects requests arriving on any non-loopback address (already in V13).
   - The header value on INBOUND is COMPUTED by the shim handler: `let depth = request.header("X-Gadgetron-Recursion-Depth").unwrap_or(0);`. The handler then INCREMENTS depth and passes `depth + 1` to the downstream router call.
   - If the downstream call triggers another brain call, that call's shim handler sees `depth + 1` on its inbound. If `depth + 1 >= max_recursion_depth`, reject.
   - This is the standard recursion-depth pattern and MUST be explicit in the doc.
2. Change default `max_recursion_depth` from 2 to **1**: no re-entry at all is the secure default. Operators who genuinely want brain→brain can opt in to 2+.
3. V12 must be `>= 1` (current) AND a new V12b: *"default is 1; higher values are permitted but emit a warning at startup"*.
4. Test: `brain_shim_rejects_recursion_depth_1_with_default_max`, `brain_shim_rejects_spoofed_depth_0_on_second_call` (simulate Claude Code resetting the header).
5. Add the header computation to the loopback-only bearer auth check — combine both into a single middleware so the auth and recursion check are inseparable.

---

### SEC-MCP-B11 — T3 tier classification is author-declared and unauthenticated; a misclassified tool can reach `auto` mode via `[agent.tools.write]` subcategory

**Finding.** The tool tier is declared by the tool author in `ToolSchema.tier: Tier` (doc §2). The config's per-subcategory override (`write.scheduler_write = "auto"`) assumes the tool is classified correctly at declaration time. There is NO cross-check at `McpToolRegistry::register` time that a tool in `scheduler.*` category is declared with the correct tier for its actual destructiveness.

Scenario: a P3 `SchedulerToolProvider` declares `scheduler.cancel_job` (which aborts a running compute job, destructive) as `Tier::Write` (not `Tier::Destructive`) — either by mistake, or through a compromised crate. An operator sets `[agent.tools.write] scheduler_write = "auto"` on a single-user desktop for speed. The agent receives a prompt injection, calls `scheduler.cancel_job` with the running training job's ID, and the job dies.

Alternative scenario: a provider uses Tier enum from an older version of `gadgetron-core` that only has `Read/Write` (before `Destructive` was added). Serde default would make all tools `Write`.

**Why this blocks.** CWE-345 (Insufficient Verification of Data Authenticity), CWE-20 (Improper Input Validation). The cardinal rule ("T3 cannot be auto") only holds if tier classification is trustworthy, and the doc does not make it so.

**Remediation.**
1. §2 add a "Tier integrity contract" subsection:
   - Tier is declared by the tool author but cross-validated at `McpToolRegistry::register` time against a **reference classifier** (`gadgetron-core::agent::tier_classifier::classify(tool_name) -> Option<Tier>`).
   - The classifier is a hardcoded table in `gadgetron-core` listing well-known tool names with their expected tier. Unknown tool names are accepted with a `warn!` trace.
   - Known tools: `wiki.write: Write`, `wiki.delete: Destructive`, `infra.deploy_model: Write`, `infra.undeploy_model: Destructive`, `scheduler.cancel_job: Destructive`, etc.
   - Mismatch = `McpError::Denied { reason: "tier mismatch" }` at registration.
2. Second defense: tool names matching a fixed regex `.*(delete|cancel|kill|drop|purge|wipe|destroy|remove).*` are forced to `Tier::Destructive` at registration time, overriding the declared tier. Document this heuristic as defense-in-depth.
3. Test: `scheduler_cancel_job_misclassified_as_write_rejected_by_registry`, `tier_classifier_forces_destroy_keyword_to_destructive`.
4. Add a new validation rule V15: *"`[agent.tools.write]` subcategories listed in `TIER_PROMOTED_TO_DESTRUCTIVE` set cannot be `"auto"`"*.

---

## MAJOR Findings

### SEC-MCP-M1 — Frontend `localStorage.gadgetron_web_auto_approve` bypasses Round 1.5-B localStorage origin analysis (SEC-W-B5)

**Finding.** §8 "Allow always" semantics:

> "Frontend writes `toolName` to `localStorage.gadgetron_web_auto_approve` set. Next time the SSE stream emits `approval_required` for a tool in the set, the frontend silently auto-POSTs `allow` WITHOUT rendering the card."

`docs/reviews/phase2/round2-security-compliance-lead-web-v2.md` explicitly retracted the "localStorage == same origin" claim and required operator guidance. This new localStorage key is a new attack surface:

1. An XSS in `gadgetron-web` (if DOMPurify bypass ever happens, see SEC-W-B1/B2 which is in scope for that review) can add arbitrary tool names to `gadgetron_web_auto_approve`, making the agent auto-approve any T2 write tool the attacker chooses.
2. The guard `if (tier === "destructive")` is client-side only. A compromised frontend bundle bypasses the guard entirely.
3. Multiple tabs / multi-session: the same origin shares localStorage. A logged-in operator with `gadgetron_web` open AND a malicious tab exfiltrating tokens via XSS can have their auto-approve set mutated.

**Remediation.**
1. §8 add: *"`localStorage.gadgetron_web_auto_approve` is defense-in-depth ONLY; the server MUST ignore a client-side 'remember' flag. Every POST still includes `{remember_for_tool: true}` for audit, but the server re-validates against the tier (T3 = always deny remember) and rate limit."*
2. The server's `POST /v1/approvals/{id}` handler MUST reject `remember_for_tool: true` for any tool whose tier is Destructive, returning 400. Test: `post_approvals_id_remember_for_t3_returns_400`.
3. Cross-ref this finding to `03-gadgetron-web.md` (SEC-W review) — the web doc's XSS hardening (DOMPurify, CSP Trusted Types) is load-bearing for this guarantee.

---

### SEC-MCP-M2 — `McpError::Denied { reason }` reaches `tool_result` unredacted; reason strings from MCP server can leak internal state

**Finding.** §3 Mode definitions: *"`never` — MCP server immediately returns `McpError::Denied { reason }`"*. The `McpError` is translated to `ToolResult { is_error: true, content: ... }` at the MCP dispatch boundary, which flows through the stream-json pipeline (`02-kairos-agent.md §6`), ultimately reaching the SSE stream → browser and the audit log.

If `reason` is sourced from a code path that includes environmental state (e.g., `"tool X requires feature flag Y which is disabled because config path Z was set to false"`), the attacker-observable tool result reveals internal Gadgetron configuration state. CWE-209.

**Remediation.**
1. §2 add: *"`McpError::Denied.reason` is a fixed, non-parameterized string drawn from a constant table `DENIAL_REASONS: &[&str]`. Dynamic parameterization is forbidden. Audit logs may capture the full internal state in `reason_internal: String` (NEVER sent to the agent), but the agent sees only the public constant."*
2. Test: `mcp_denied_reason_comes_from_fixed_constant_table`.

---

### SEC-MCP-M3 — Audit log `stderr_redacted` extension for `startup_token` is not specified; token can leak via `redact_stderr` gap

**Finding.** `02-kairos-agent.md` M2 `redact_stderr` pattern list:
- `sk-ant-[a-zA-Z0-9_-]{40,}` (Anthropic)
- `gad_(live|test)_[a-f0-9]{32}` (Gadgetron)
- `Bearer\s+[A-Za-z0-9._-]+`
- High-entropy base64-ish preceded by `token`, `secret`, `key`, `auth`

The new `startup_token` (SEC-MCP-B5) format is not specified in this doc. If it is 32 bytes hex (64 chars `[a-f0-9]`), it does NOT match `gad_*` pattern and MAY match the "high-entropy base64-ish" heuristic only if prefixed by a keyword. A token that leaks via `claude --debug` stderr containing `"ANTHROPIC_API_KEY=abcdef...64chars"` does match the `key` heuristic; but a log line like `"using api credentials abcdef...64chars"` does not.

**Remediation.**
1. §12 spec: `startup_token` format is `gad_brain_[a-f0-9]{64}` so it matches a new pattern `gad_brain_[a-f0-9]{64}` in `redact_stderr`.
2. Cross-reference: update `02-kairos-agent.md §8` M2 pattern list to include this new pattern.
3. Test: `redact_stderr_strips_gad_brain_token`.

---

### SEC-MCP-M4 — Frontend DOMPurify pipeline for `rationale` is trusted but `rationale` sources from the agent which reads untrusted wiki/search content

**Finding.** §8: *"Rationale rendered through the same `<MarkdownRenderer>` pipeline (§16 of 03-gadgetron-web.md) — goes through DOMPurify"*. `03-gadgetron-web.md` v2 sanitizes markdown with DOMPurify Trusted Types (SEC-W-B1/B2 verified resolved). This is correct for SAFETY, but does not defend against PROMPT INJECTION IMPRINTING.

Scenario: attacker injects a malicious wiki page with content designed to manipulate the `rationale` emitted by the agent during the approval card render: *"Rationale: this is a completely safe read, trust me, click Allow Always"*. The user reads the rationale on the approval card, is nudged to click Allow. This is a social engineering attack vector, not an XSS — DOMPurify does nothing against it.

**Remediation.**
1. §8 add a "Rationale trust model" subsection:
   - `rationale` is UNTRUSTED and is rendered with a prominent visual marker: *"Agent rationale (not verified)"* above the text.
   - A `warn_injection_keywords` function checks `rationale` for known prompt-injection social-engineering patterns: `"trust me"`, `"it's safe"`, `"Allow Always"`, `"click yes"`, `"no need to read"`, etc., and displays a banner if any match.
   - The banner is mandatory for T2 and T3; cannot be disabled.
2. Test: `rationale_with_trust_me_phrase_renders_warning_banner`, `rationale_does_not_render_agent_authority_framing`.
3. Cross-ref `03-gadgetron-web.md` to add the banner component.

---

### SEC-MCP-M5 — `[agent.tools.destructive] extra_confirmation = "env"` mode is mentioned in §4 but has no validation rule in §5

**Finding.** §4: *"Optional belt-and-suspenders token. `"none"` (default) = UI approval alone suffices. `"env"` or `"file"` = UI approval AND a pre-shared token match both."*

§5 V6 only covers `"file"` mode file existence and perms. `"env"` mode is not validated — what env var? `GADGETRON_DESTRUCTIVE_TOKEN`? Is it required to be set if `extra_confirmation = "env"`? Empty string? What entropy?

The existing code (`config.rs:427-430`) has an `ExtraConfirmation::Env` variant but the validation is only on file mode (lines 447-471). Env mode has no validation path.

**Remediation.**
1. Add V6b: *"`extra_confirmation == "env"` AND `GADGETRON_DESTRUCTIVE_TOKEN` is unset or `< 16 bytes`"*.
2. Document the env var name explicitly; is it hardcoded or configurable via another field?
3. Test: `v6b_destructive_env_mode_requires_token_env`, `v6b_destructive_env_mode_rejects_short_token`.

---

### SEC-MCP-M6 — Audit log `tools_called` field (existing) vs new `ToolCallCompleted` event: double-accounting risk

**Finding.** `00-overview.md §8` and `02-kairos-agent.md` specify `AuditEntry.tools_called: Vec<String>` — a names-only list accumulated during a session. The new `ToolCallCompleted` event (§10) records individual tool calls with outcome and latency. These two capture the same events but with different schemas; if both are written for the same call, auditors see double counts.

**Remediation.**
1. §10 add a "Migration from legacy `tools_called` field" subsection: *"The `AuditEntry.tools_called` field is superseded by `ToolAuditEvent::ToolCallCompleted`. Phase 2A writes both during a transition window; Phase 2B removes `tools_called`. Auditors should prefer `ToolCallCompleted` for forensics."*
2. Test: `audit_tool_call_completed_matches_legacy_tools_called_during_transition`.

---

## MINOR Findings

### SEC-MCP-N1 — `extra_confirmation_token_file` perms check is Unix-only; Windows deferred without mention

§5 V6: file mode 0400/0600. `config.rs:456-470` only runs under `#[cfg(unix)]`. Windows deployments get no check. §1 scope says Linux/macOS but `00-overview.md` Deployment modes table mentions Docker (which may run on Windows Docker Desktop). Add a V6c: *"`#[cfg(windows)]` path rejects `extra_confirmation = "file"` with a "Windows is not supported for file-backed confirmation tokens" error."*

### SEC-MCP-N2 — `approval_timeout_secs` default of 60s lacks justification against social engineering bypass

A user distracted by a convincing rationale may instinctively click before reading. 60s is the human attention span. Document the trade-off in §4 config comment and in M8: *"Reducing timeout below 30s discourages click-fatigue approval but risks legitimate timeouts. 60s is the median industry default."*

### SEC-MCP-N3 — `PendingApproval.category: &'static str` prevents runtime categorization

`&'static str` means the category must be a compile-time constant. If a future provider wants dynamic category (e.g., per-plugin category loaded from config), this type forbids it. The existing `McpToolProvider::category(&self) -> &'static str` enforces the same constraint — consistent. But document the trade-off: runtime categories require a type change, which is a SemVer-major API change.

### SEC-MCP-N4 — SSE `heartbeat: keepalive` is not specified as a reserved SSE event type

§7: *"Kairos provider emits `: keepalive\n\n` SSE comment frames"*. The `:` prefix is an SSE comment, not an event. The doc calls it "comment frames" — correct. But some frontend clients discard comments entirely; verify `03-gadgetron-web.md §?` SSE parser handles them without raising errors.

### SEC-MCP-N5 — `[agent.brain].external_proxy` mode has no URL validation

`external_base_url` is accepted as any string. No `https://` enforcement, no allowlist, no host regex. A typo like `htp://` or `example.com` (missing scheme) silently passes. Add V15: *"`external_base_url` must start with `https://` OR match `http://127.0.0.1|http://localhost` for local proxies only"*.

### SEC-MCP-N6 — `Scope::AgentApproval` + `[agent.brain.shim]` endpoint interaction not analyzed

If the brain shim is routed via gateway (P2C scope), then `POST /internal/agent-brain/v1/messages` is INSIDE the gateway and subject to scope middleware. The doc does not say whether `/internal/*` is bypassed by the scope guard or whether it's gated by a new `Scope::InternalAgentBrain` variant. Defer resolution to P2C but add an OPEN ITEM: *"P2C: scope enforcement for `/internal/*` routes."*

### SEC-MCP-N7 — §12 missing "Scope of testing for shim" — P2A has no E2E coverage

§16 lists E2E tests for P2C only: `brain_shim_loopback_only`, `brain_shim_recursion_guard`. P2A has config validation tests (V12, V13) but no runtime assertion that the shim binds loopback-only. A user could define the config correctly and the shim code could regress — nothing catches it in P2A because the shim is not implemented. Acceptable trade-off but explicit: add a statement *"P2A: shim is unimplemented, config is accepted but no runtime endpoint exists. Any HTTP call to `/internal/agent-brain/v1/messages` returns 501 Not Implemented."*

---

## Threat Model Diff from `00-overview.md §8`

This doc creates new assets, new trust boundaries, and new attack surfaces that are not in the parent threat model. An explicit diff table is required (see SEC-MCP-B2).

### New assets (not in §8)

| Asset | Sensitivity | Owner | Lifetime | Location |
|---|---|---|---|---|
| `PendingApproval.args` (sanitized JSON) | **High** — may contain wiki content | User | Per-request, dropped on decide/timeout | In-process (gateway memory) |
| `PendingApproval.rationale` (agent string, untrusted) | **Medium** — social engineering vector | Agent (untrusted) | Per-request | In-process + SSE payload + DOM |
| `startup_token` (32-byte brain shim auth) | **Critical** — grants brain shim access | Gadgetron | Process lifetime | In-process + subprocess env (`ANTHROPIC_API_KEY`) |
| `ApprovalRegistry` state | **High** — approval loss = silent DoS | Gateway | Per-request oneshot | In-process DashMap |
| `t3_rate_limit_counter` | Medium — counter state | Gateway | In-process, per-hour tumbling | In-process (unbacked) |
| `audit.ToolApprovalRequested.rationale_digest` | **Medium** — forensic fingerprint | Auditor | Retention = 365d (unbacked) | DB (future) |
| `localStorage.gadgetron_web_auto_approve` | **Medium** — XSS target | User (browser) | Until localStorage cleared | Browser |

### New trust boundaries (not in §8)

| ID | Boundary | Crosses | Auth mechanism | Spec'd? |
|---|---|---|---|---|
| B7 | MCP subprocess → gateway ApprovalRegistry | Process or network (TBD per SEC-MCP-B1) | **UNSPECIFIED** | NO |
| B8 | Browser → `POST /v1/approvals/{id}` | Network (bearer) | `Scope::OpenAiCompat` OR `Scope::AgentApproval` (broken per SEC-MCP-B4) | PARTIALLY |
| B9 | `internal_call = true` flag → router dispatch | In-process boolean | Operator-provided, shim-set | NO |
| B10 | `startup_token` → Claude Code subprocess env → grandchild env inheritance | Process env propagation | None (env var) | NO |
| B11 | `X-Gadgetron-Recursion-Depth` header trust | HTTP header | Loopback-only assumption | NO |

### STRIDE additions (new rows)

| Component | S | T | R | I | D | E | Highest unmitigated risk |
|---|---|---|---|---|---|---|---|
| `ApprovalRegistry` | Medium — approval_id guess (B8) | Medium — decide race (B7/B8) | Low | Medium — args in memory | **High** — race/lost wakeup (SEC-MCP-B7) | Low | Lost wakeup under load |
| `POST /v1/approvals/{id}` | Medium — scope confusion (B4) | Low | **High** — wrong scope allows approval | Medium — 404 timing | **High** — flood DoS (B8) | Medium — AgentApproval→OpenAiCompat (B4) | Scope collision (SEC-MCP-B4) |
| Cross-proc approval bridge (B7) | **UNSPECIFIED** | **UNSPECIFIED** | **UNSPECIFIED** | **UNSPECIFIED** | **UNSPECIFIED** | **UNSPECIFIED** | Architectural hole (SEC-MCP-B1) |
| Brain shim (P2C stub in P2A) | Medium — header spoof (B11) | Medium | Low | High — token in env (B10) | Medium — no rate limit | **High** — recursion (default 2, SEC-MCP-B10) | Header trust model (SEC-MCP-B10) |
| `<ApprovalCard>` | Low | Medium — XSS in rationale (cross-ref 03 doc) | Medium | High — args plaintext in SSE (SEC-MCP-B6) | Low | Medium — localStorage XSS (SEC-MCP-M1) | Rationale social engineering (SEC-MCP-M4) |

### Mitigations (M9..M16) — required to be defined in the doc's new threat model section

- **M9** — Cross-proc approval bridge: loopback HTTP + startup token, SEC-MCP-B1 remediation
- **M10** — `args` sanitization contract: credential patterns + length cap, SEC-MCP-B6 remediation
- **M11** — ApprovalRegistry race-free state machine: atomic state transitions, SEC-MCP-B7 remediation
- **M12** — Approval flood defense: per-key rate limit + 404 brute-force detection, SEC-MCP-B8 remediation
- **M13** — Startup token disclosure discipline: OsRng + `redact_stderr` pattern + env isolation, SEC-MCP-B5 remediation
- **M14** — Tier integrity cross-check: registration-time classifier, SEC-MCP-B11 remediation
- **M15** — Reserved namespace NFKC normalization: Unicode-safe enforcement, SEC-MCP-B3 remediation
- **M16** — Rationale trust UI: agent-authorship banner + injection keyword warning, SEC-MCP-M4 remediation

---

## Cross-document dependencies (MUST resolve before this doc passes Round 1.5)

| Dep | Target doc | Reason |
|---|---|---|
| D1 | `02-kairos-agent.md` | `feed_stdin` text mode (ADR-P2A-01 VERIFIED TEXT) has no `tool_use → tool_result` round trip analysis. Approval flow assumes `-p` mode pauses on MCP tool call — ADR-P2A-01 verified enforcement but NOT pause semantics. A paused tool call on stdio, with NO way for the MCP server to delay its response beyond (what?) seconds, may be killed by Claude Code's own internal timeout. This is SEC-MCP-B12 (promoted from N to B because it is load-bearing) — VERIFY that Claude Code `-p` mode accepts a slow MCP tool response (say, 60s) without killing the subprocess or the MCP stream. Add as a new behavioral test to ADR-P2A-01 verification record: "Part 3 — slow MCP tool response tolerance". |
| D2 | `03-gadgetron-web.md` | `<ApprovalCard>` rationale DOMPurify pipeline (SEC-MCP-M4) + localStorage XSS guard (SEC-MCP-M1) + SSE `gadgetron.approval_required` event schema (implies SEC-W review re-verification). |
| D3 | `00-overview.md §8` | M5 `redact_stderr` pattern list extension to include `gad_brain_*` (SEC-MCP-B5). M8 risk acceptance additions for approval bridge (B7), rate limit restart volatility (SEC-MCP-B7/B8), `/proc/$pid/environ` on Docker (SEC-MCP-B5). |
| D4 | `ADR-P2A-01` | Part 3 verification for slow MCP tool response (see D1 above). |
| D5 | `D-20260411-10` | Scope enum update: `AgentApproval` variant and its interaction with existing prefix-dispatcher (SEC-MCP-B4). |

---

## Verdict

**BLOCK** — 11 blockers (SEC-MCP-B1..B11), 6 majors (SEC-MCP-M1..M6), 7 minors (SEC-MCP-N1..N7), and a mandatory threat-model section.

The doc cannot enter Round 2 (qa-test-architect) until:

1. **SEC-MCP-B1** — cross-process approval bridge specified (transport, auth, wire schema, fail-closed path)
2. **SEC-MCP-B2** — full §(threat model) section added with STRIDE table and mitigations M9..M16
3. **SEC-MCP-B3** — reserved namespace enforcement uses `Result::Err`, NFKC normalization, and provider category cross-check
4. **SEC-MCP-B4** — `Scope::AgentApproval` semantics pick one of three options; middleware change documented
5. **SEC-MCP-B5** — CSPRNG = OsRng documented, token format = `gad_brain_*`, env inheritance guard specified
6. **SEC-MCP-B6** — `sanitize_args` contract specified with enforcement location + tests
7. **SEC-MCP-B7** — ApprovalRegistry race-free state machine; rate limit store specified
8. **SEC-MCP-B8** — per-key rate limit, 404 brute-force detection, 429 on repeat misses
9. **SEC-MCP-B9** — `purge_audit_log` reference removed or bridged to future ADR; `retention_class` tag added
10. **SEC-MCP-B10** — recursion depth header trust model; default changed to 1
11. **SEC-MCP-B11** — tier integrity cross-check at registration time
12. **Cross-doc D1/D4** — ADR-P2A-01 Part 3 verification (slow MCP tool response tolerance on `claude -p`)

None of these are prose fixes — each requires a concrete code/config spec with test names, file paths, and the same level of implementation determinism as the rest of the doc.

I will re-review upon v2 submission. My estimate: 2-3 iteration cycles, ~1-2 days of PM drafting per cycle.

---

## Sibling review summary

`dx-product-lead` reviewed this same doc for Round 1.5-B (usability) and returned **BLOCK** with 10 blockers of its own. The security and usability findings are complementary, not overlapping — the doc needs BOTH reviews addressed, plus Round 2 (qa) and Round 3 (chief-architect), before `#10` code lands. Current estimate: 5-7 days of doc work before implementation start.
