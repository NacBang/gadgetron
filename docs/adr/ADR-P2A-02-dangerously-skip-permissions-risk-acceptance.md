# ADR-P2A-02 — `--dangerously-skip-permissions` and P2A Single-User Risk Acceptance

| Field | Value |
|---|---|
| **Status** | ACCEPTED (P2A single-user scope only) (v3 — Round 2 review addressed) |
| **Date** | 2026-04-13 |
| **Author** | security-compliance-lead |
| **Parent docs** | `docs/design/phase2/00-overview.md` v2 §8 M8; `docs/design/phase2/02-penny-agent.md` v2 §15.3 |
| **Conditioned on** | ADR-P2A-01 PASS — this acceptance is invalidated if `--allowed-tools` enforcement fails |
| **P2C gate** | `[P2C-SECURITY-REOPEN]` — this acceptance explicitly does NOT extend to multi-user deployments |

---

## Context

### What `--dangerously-skip-permissions` does

Claude Code's interactive mode requires the user to confirm each tool call before
execution. This is a safety mechanism: the user reviews `Read /etc/passwd`, `Bash
rm -rf`, or `Edit ~/.bashrc` before it runs.

The `-p` (print/pipe) mode is designed for automated, non-interactive use. In `-p`
mode, interactive confirmation is structurally incompatible because there is no TTY.
`--dangerously-skip-permissions` is the explicit opt-in that tells Claude Code to
execute tool calls without confirmation in `-p` mode.

Penny requires `-p` mode because:
1. Claude Code is a subprocess spawned per HTTP request, with stdin/stdout piped to
   the Gadgetron process (no TTY)
2. The response must stream back to the client in real time (no human in the loop)
3. The user's intent is already expressed in the chat message — a second confirmation
   would break the UX model

The invocation (canonical in `docs/design/phase2/00-overview.md` v2 Appendix B):

```bash
claude -p \
  --output-format stream-json \
  --mcp-config <tempfile-path> \
  --allowed-tools mcp__knowledge__wiki_list,...,mcp__knowledge__web_search \
  --dangerously-skip-permissions
```

### The security risk

Without interactive confirmation, any tool call the model outputs is executed
immediately. The risks are:

**R1 — Prompt injection via wiki content**
A malicious wiki page (written by the user earlier, or injected via `wiki_write` as
part of a prior attack) can instruct the model to call `wiki_write` with attacker-
controlled content, corrupting the user's knowledge base. Worst case: wiki content
integrity loss.

Source: `docs/design/phase2/00-overview.md` v2 §8 STRIDE table, Claude Code
subprocess row, T (tamper) = High.

**R2 — Prompt injection via SearXNG results**
A malicious web page returned in a search result can include prompt injection
instructions. The model may act on these before filtering or context-switching.
Same worst-case as R1.

Source: `docs/design/phase2/00-overview.md` v2 §8 STRIDE table, SearXNG row,
I (disclose) = High.

**R3 — Unconstrained tool scope (conditional)**
If `--allowed-tools` enforcement fails (ADR-P2A-01 outcome = FAIL), the model could
call `Read`, `Bash`, or `Write` in addition to the knowledge MCP tools, escalating
from wiki-corruption to credential exfiltration.

Source: `docs/design/phase2/00-overview.md` v2 §8 M4 and M8.

**R4 — Audit log persistence**
Audit records include `tools_called: Vec<String>` and `stderr_redacted`. These
persist on the local filesystem. If the local machine is compromised, these records
may reveal usage patterns or (despite redaction) partial information.

Source: `docs/design/phase2/00-overview.md` v2 §8 audit logging section.

### Why this risk is acceptable for P2A

P2A is explicitly scoped to **single-user local deployment** on the user's own
machine. The threat model for this deployment context differs fundamentally from
a multi-user or cloud deployment:

| Factor | P2A single-user | P2C multi-user |
|---|---|---|
| Who can trigger a Penny request | The user themselves | Any authenticated tenant |
| Who owns the wiki | The user | A shared or per-tenant store |
| Worst case of wiki corruption | User corrupts their own data | Tenant A corrupts Tenant B's data |
| Credential exposure surface | User's own credentials (already accessible on their machine) | Cross-tenant credential leakage |
| Interactive confirmation alternative | User is the operator; consent is expressed at config time | Per-request confirmation or RBAC required |
| Audit log custody | User's own machine | Shared or operator-controlled |

The user has already consented to this risk by setting `[penny]` section in
`gadgetron.toml` and running `gadgetron serve`. The act of configuration is the
consent mechanism.

This reasoning matches M8 in `docs/design/phase2/00-overview.md` v2 §8:

> `--dangerously-skip-permissions` removes interactive confirmation; acceptable
> because the user is the operator and has consented via config.

---

## Decision

### Explicit risk acceptance statement for P2A

The following risks are **explicitly accepted** for Gadgetron Phase 2A,
bounded to single-user local deployment:

**Accepted risk A — wiki data integrity**
Prompt injection from SearXNG results or malicious wiki pages may cause `wiki_write`
tool calls that corrupt or pollute the user's wiki. Worst case: wiki content integrity
loss. Accepted because:
- The wiki is the user's own data; they are the only party harmed
- M3 (path traversal prevention) limits writes to within `wiki_path`
- M5 (`wiki_max_page_bytes`, credential BLOCK patterns) limits payload size and
  prevents credential storage
- Git history (`wiki_write` auto-commits) provides recovery capability
- The user can audit and revert via standard `git` commands

Source: `docs/design/phase2/02-penny-agent.md` v2 §15.3 STRIDE table,
`ClaudeCodeSession` row.

**Accepted risk B — no interactive confirmation**
Claude Code will execute tool calls without prompting the user. Accepted because:
- The user has consented at configuration time by enabling the `[penny]` section
- P2A is single-user; the user is simultaneously operator and data subject
- `--allowed-tools` restricts the callable surface to five knowledge tools
  (conditioned on ADR-P2A-01 PASS)
- The audit log records `tools_called: Vec<String>` so the user can review what
  happened after the fact

**Accepted risk C — local-only audit logs**
Audit logs are written to the local filesystem with no remote aggregation in P2A.
Accepted because:
- P2A is single-user; the user is the log custodian
- Local-only storage reduces the attack surface compared to remote log shipping
- This is a weaker guarantee than P2C will require (CC7.2 SOC2 anomaly detection)

**Accepted risk D — prompt injection data integrity (not exfiltration)**
This acceptance is explicitly conditioned on ADR-P2A-01 PASS. If `--allowed-tools`
enforcement holds:
- Prompt injection can cause wiki corruption (data integrity risk)
- Prompt injection CANNOT cause credential exfiltration via `Read`/`Bash`
  (these tools are not on the whitelist and the binary blocks them)

If ADR-P2A-01 returns FAIL, this acceptance is INVALID and must be reopened with
the sandbox fallback plan in place before any penny implementation proceeds.

### Conditionality

This ADR ACCEPTED status is conditioned on:

1. **ADR-P2A-01 PASS** — `--allowed-tools` must be verified as binary-level
   enforcement before this acceptance holds. If ADR-P2A-01 returns FAIL, this ADR
   must be revised to reference the sandbox as the enforcement mechanism.

2. **Single-user deployment only** — this acceptance is invalid for any deployment
   where more than one OS user account, or more than one Gadgetron tenant, can send
   requests to the same `gadgetron serve` instance.

### Non-applicability to P2C

`[P2C-SECURITY-REOPEN]`

This risk acceptance is explicitly bounded to P2A single-user local deployment.
When P2C (multi-user, on-premise, or cloud) is designed, the following assumptions
from this ADR break and MUST be re-evaluated:

| Assumption | Why it breaks in P2C |
|---|---|
| User is operator and data subject | Different users send requests; operator ≠ user |
| Worst case is self-harm (own wiki) | Cross-tenant wiki corruption or credential leakage becomes possible |
| No interactive confirmation needed | Per-request confirmation or RBAC per tool must be designed |
| Local audit log custody | Audit logs are operator-controlled; access control and tamper resistance required |
| Single OS user | Multi-user processes require credential isolation (per-user subprocess, container, or token delegation) |
| Consent via config | Consent mechanism must be per-user, not per-deployment |

P2C MUST NOT inherit this ADR. A new ADR is required before P2C multi-user
implementation begins.

This tag also appears at the relevant locations in:
- `docs/design/phase2/02-penny-agent.md` v2 §15.3 (security lead review F2)
- `docs/design/phase2/00-overview.md` v2 §8 M8

---

## Consequences

### For P2A implementation

- `spawn.rs::build_claude_command()` MUST include `--dangerously-skip-permissions`
  as a non-configurable argument (not exposed in `gadgetron.toml`)
- The flag is not user-configurable because removing it breaks the non-interactive
  pipeline. If a user wants interactive confirmation, they run `claude` directly
- `config.rs::PennyConfig` does NOT have a `dangerously_skip_permissions: bool`
  field; the flag is hard-coded in spawn logic
- The test `build_claude_command_has_expected_args` in `spawn.rs` verifies
  `--dangerously-skip-permissions` is present in every subprocess invocation

### For documentation

- `docs/manual/penny.md` (P2A pre-merge requirement) MUST include a plain-language
  note explaining what `--dangerously-skip-permissions` means and why it is used
- The note must NOT be buried — it should appear in the security/privacy section
  alongside the Disclosure 2 (SearXNG) text required by ADR-P2A-03
- The note must not alarm users unnecessarily but must be factually accurate:
  Penny runs in automated mode and executes tool calls without asking per-call;
  this is why the wiki and web search tools are the only ones available

### For future phases

- P2B (stream resumption, session history) must not expand `--allowed-tools` scope
  without a new security review
- P2C (multi-user) requires a full threat model re-evaluation and a new ADR before
  any multi-user penny feature is implemented
- If Claude Code removes or changes `--dangerously-skip-permissions` in a future
  CLI version, the penny spawn logic must be updated and this ADR reviewed

---

## Deployment preconditions (non-root user assumption)

This risk acceptance EXPLICITLY ASSUMES that `gadgetron serve` runs as a
NON-PRIVILEGED OS user (typically a dedicated `gadgetron` service account)
with filesystem access limited to:
- `~/.gadgetron/` (wiki path + config + audit log)
- `~/.claude/` (credential session — READ-ONLY if possible)
- `$TMPDIR` for subprocess-owned tempfiles

Running `gadgetron serve` as root (or any user with broad filesystem access)
is EXPLICITLY UNSUPPORTED for P2A. In a root context, a `wiki_write` with a
successful M3 bypass (symlink race) could corrupt arbitrary system files; the
`--dangerously-skip-permissions` flag removes the Claude Code interactive
confirmation safety net that would otherwise catch this.

**Operator action required**: install and run `gadgetron` as a non-root
user. If your deployment platform (systemd, docker-compose, Kubernetes) runs
as root by default, configure `User=gadgetron` (systemd) or `user: gadgetron`
(compose) or `securityContext.runAsUser` (K8s).

**This precondition does NOT apply to P2A single-user local desktop** — the
desktop user running `cargo run` or `./target/release/gadgetron serve` is
already the owner of `~/.gadgetron/` and `~/.claude/`, so their privilege
level is de facto limited by their filesystem ACLs.

---

## References

| Document | Section | Relevance |
|---|---|---|
| `docs/design/phase2/00-overview.md` v2 | §8 M8 | Primary source for P2A risk acceptance |
| `docs/design/phase2/00-overview.md` v2 | §8 STRIDE table, Claude Code row | E (escalate) = High baseline threat |
| `docs/design/phase2/00-overview.md` v2 | §8 deployment modes | P2A vs P2C deployment context |
| `docs/design/phase2/02-penny-agent.md` v2 | §15.3 STRIDE table | Per-component threat analysis |
| `docs/design/phase2/02-penny-agent.md` v2 | §15.4 mitigations table | M8 maps to this ADR |
| `docs/design/phase2/02-penny-agent.md` v2 | §16 ADR table | Listed as impl blocker |
| `docs/design/phase2/02-penny-agent.md` v2 | `spawn.rs` build function | Hard-coded flag location |
| `docs/adr/ADR-P2A-01-allowed-tools-enforcement.md` | Part 1 | Conditioned PASS required for this acceptance |
| `docs/design/phase2/00-overview.md` v2 | §10 Compliance (GDPR/SOC2) | P2C GDPR obligations listed |
| OWASP LLM Top 10 | LLM02 — Insecure Output Handling | Secondary category |

---

## Changelog

- **2026-04-13 — Round 2**: added non-root user precondition section (`runAsUser`, `User=gadgetron`, filesystem ACL constraint). Status bumped to v3.
