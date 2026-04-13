# Round 2 Cross-Review — dx-product-lead
**Date**: 2026-04-13
**Scope**: `docs/design/phase2/{00,01,02}.md` v2 + `docs/adr/ADR-P2A-{01,02,03}.md`
**Reviewer role**: Round 1.5 usability (§1.5-B checklist)

---

## Verdict

**APPROVE WITH MINOR**

All v1 dx blockers are addressed. Three new minor blockers and five nits identified below.
The docs are implementation-ready with the minor blockers resolved; none require a full
re-round.

---

## v1 Blocker Verification

The Appendix D in 00-overview states "dx A1-A9 all addressed in §3/§4/§6/§12." The 01
review provenance table lists exactly A1-A6 for dx-product-lead; the 02 provenance table
lists four dx items (§12, §5/spawn, §10 TOML, §9 log hint). The A7-A9 references in 00
Appendix D appear to count the 02 dx items as continuation of the same sequence. All items
from both 01 and 02 provenance rows are verified below.

### 01-knowledge-layer.md dx blockers

| ID | Topic | Status | Citation |
|----|-------|--------|----------|
| A1 | Rewrite `wiki_search` and `web_search` MCP tool descriptions | APPROVED | `01-knowledge-layer.md:966-1059` — descriptions include "when you don't know the exact page name," `max_results` guidance, "Google, Bing, DuckDuckGo, and Brave via a self-hosted SearXNG proxy." |
| A2 | `max_results` default of 5 present in `wiki_search` schema | APPROVED | `01-knowledge-layer.md:1010-1016` — `"default": 5` in JSON schema, `"maximum": 50` |
| A3 | `PageTooLarge` MCP tool error text includes `bytes` and `limit` values | APPROVED | `01-knowledge-layer.md:1121-1123` — `format!("Page too large: {bytes} bytes exceeds the {limit}-byte limit.")` |
| A4 | `gadgetron kairos init` stdout contract specified verbatim | APPROVED | `01-knowledge-layer.md:49-118` — §1.1 has exact literal output for success, 3 failure paths, `--docker` behavior |
| A5 | `eprintln!` used for operator-visible fallback warning (not `tracing::warn!` alone) | APPROVED | `01-knowledge-layer.md:106` — "printed via `eprintln!`, NOT `tracing::warn!` alone"; `config.rs` fallback at line 1207-1212 |
| A6 | `WikiErrorKind` moved to `gadgetron-core` (01 scope, not 02) | APPROVED | `01-knowledge-layer.md:32, 1222-1269` — `GadgetronError::Wiki` variant defined in this spec, re-exported from knowledge crate |

### 02-kairos-agent.md dx blockers

| ID | Topic | Status | Citation |
|----|-------|--------|----------|
| A7 | `kairos init` stdout authoritative cross-ref to 01 §1.1 | APPROVED | `02-kairos-agent.md:987-993` — dispatch calls `cmd_kairos_init`; comment says "Exact stdout contract authoritative in 01 §1.1. No divergence permitted." |
| A8 | `kill_on_drop(true)` in spawn + `max_concurrent_subprocesses` in TOML + `claude_model` empty-string validation | APPROVED | `02-kairos-agent.md:450, 894-903, 880-888` |
| A9 | `SpawnFailed` error message includes log hint for operator | APPROVED | `02-kairos-agent.md:814` — "Run `gadgetron serve` with `RUST_LOG=gadgetron_kairos=debug` for spawn diagnostics, or check `journalctl -u gadgetron`" |

---

## New Blockers

### BLOCKER-1: `gadgetron kairos init --scope` flag undocumented; `key create --scope open_ai_compat` inconsistency

**Location**: `00-overview.md:208` (Quick Start step 3)

**Issue**: The quickstart step 3 says:
```sh
./target/release/gadgetron key create --scope open_ai_compat
```
But the Phase 1 quickstart (`docs/manual/quickstart.md:122`) uses `key create` with `--tenant-id`, no `--scope` flag. The `--scope open_ai_compat` flag does not appear in any Phase 1 manual, CLI reference, or configuration doc. A new operator following the Phase 2 quickstart cannot know:

1. Whether `--scope` is a new Phase 2A flag or an existing one they missed.
2. What scopes are available besides `open_ai_compat`.
3. Whether `--scope` replaces `--tenant-id` or is additive.

The design doc references Phase 1 command output in parentheses "Phase 1 command" but gives no pointer to where `--scope` is documented or when it was added.

**Fix**: Either (a) replace with the exact Phase 1 `key create` invocation that already works (with `--tenant-id`), or (b) add a one-sentence note — "Phase 2A adds `--scope open_ai_compat` to `key create`; see `docs/manual/api-reference.md`" — and ensure the `key create` help text accepts the flag. If the flag doesn't exist yet, use the Phase 1 form in the quickstart and note that any valid key works.

---

### BLOCKER-2: `gadgetron kairos init --wiki-path` flag defined in 02 but not in 01 stdout contract; exit behavior inconsistent

**Location**: `02-kairos-agent.md:966-972` (CLI subcommand enum); `01-knowledge-layer.md:108-115` (failure path)

**Issue**: The `KairosCommand::Init` enum in 02 accepts `--wiki-path: Option<PathBuf>`. The 01 failure path text says:
```
Fix: choose a different path with `gadgetron kairos init --wiki-path <PATH>`,
```
However, the 01 §1.1 success-path output does not show how `--wiki-path` changes the printed output (does the `[OK] Wiki directory:` line show the custom path?). A developer implementing this must guess. Worse, there is no documented flag-conflict behavior: if the user has already set `wiki_path` in `gadgetron.toml`, does `--wiki-path` override it? Does `kairos init` read an existing config?

**Fix**: Add a one-paragraph note to 01 §1.1 (below the `--docker` description) specifying: (a) `--wiki-path` overrides `wiki_path` from any existing `gadgetron.toml`; (b) the `[OK] Wiki directory:` line always shows the resolved path; (c) the written `gadgetron.toml` uses the `--wiki-path` value.

---

### BLOCKER-3: `feed_stdin` is explicitly marked TBD — implementation determinism rule violation

**Location**: `02-kairos-agent.md:405-424` (`feed_stdin` function + comment)

**Issue**: The `feed_stdin` function contains an explicit "NOTE: Claude Code `-p` stdin contract verification is pending (ADR-P2A-01 behavioral test). v2 assumes JSON `{"messages":[...]}` on stdin." This is a documented ambiguity for a core data-flow function. Per the `feedback_implementation_determinism.md` rule: "TBD/모호함/추상적 표현 금지." A developer picking up this crate cannot implement `session.rs::feed_stdin` or the `stdin_echo` fake-claude scenario deterministically without resolving this first.

ADR-P2A-01 correctly identifies this as a blocking action item (A2), but the spec body itself carries the TBD inline. The review rubric requires "No TBD" for a spec to pass Round 1.5.

**Fix**: This blocker is already tracked as "Blocks P2A impl start" in ADR-P2A-01. The mitigation for the review is to move the entire `feed_stdin` implementation body out of the spec into a placeholder comment that says "to be filled after ADR-P2A-01 Part 2 is resolved" rather than showing a concrete `serde_json::json!` call that may be wrong. The current form gives false confidence that Option A is correct. Alternatively, show BOTH formats (Option A and Option B) as conditional branches and annotate which branch is activated by ADR-P2A-01 outcome. This makes the ambiguity explicit without being misleading.

---

## Recommendations (Non-Blocking)

### NIT-1: `kairos init` success output uses `Done.` but Phase 1 doctor uses `[ok]` style

**Location**: `01-knowledge-layer.md:80`

Phase 1 `gadgetron doctor` uses `[ok]` and `[FAIL]` prefixes (per `docs/manual/troubleshooting.md:19-35`). Phase 2 `kairos init` uses `[OK]` (uppercase). These differ by case. For consistency within the Gadgetron CLI family, pick one convention. Recommendation: use `[OK]` in kairos init (it scans better with surrounding `[WARN]`) and note in the troubleshooting doc that both forms appear in different subcommands. Minor, but someone will file a "why is it `[ok]` in one place and `[OK]` in another" ticket.

### NIT-2: 00-overview §4 Quick Start step 4 emits two commands on one invocation

**Location**: `00-overview.md:216-220`

```sh
./target/release/gadgetron kairos init --docker > docker-compose.yml
docker compose up -d
./target/release/gadgetron serve --config ~/.gadgetron/gadgetron.toml
```

Step 4 is labelled "Start Gadgetron, OpenWebUI, and SearXNG" but the first command runs `kairos init --docker` again (which was already run in step 2 without `--docker`). This is confusing: did the user need to run `kairos init` twice? The 01 §1.1 specifies that `--docker` emits compose YAML to stdout with no banner. The quickstart should instead say `./target/release/gadgetron kairos init --docker > docker-compose.yml` is an alternative to step 2, or it should clearly label this as generating the compose file separately from the init. Suggest splitting into "2a: init workspace" and "2b: optionally generate compose" rather than a deferred step 4 that re-runs `kairos init`.

### NIT-3: Error table in 00-overview §12 has `WikiErrorKind::Conflict` HTTP 409 mapped as `server_error` type — OpenAI convention mismatch

**Location**: `00-overview.md:706`

The `wiki_conflict` error maps `type = "server_error"` with HTTP 409. OpenAI uses `invalid_request_error` for 409 conflicts (e.g., model not found returns `model_not_found` type = `invalid_request_error`). HTTP 409 is a client-detectable condition (git conflict = the client or another process wrote; the client may resolve it). Mapping it to `server_error` implies the server is broken when actually the wiki state needs user action. Recommendation: change `type` to `invalid_request_error` for HTTP 409, consistent with the 01 spec's `error_type` table which correctly assigns `invalid_request_error` to 413 and 400.

This is a nit because the client (OpenWebUI) likely does not branch on `error.type` for wiki errors, but it matters for API callers who follow OpenAI convention.

### NIT-4: ADR-P2A-03 disclosure text uses "Google, Bing, DuckDuckGo, and Brave" — hardcoded engine list may be wrong

**Location**: `docs/adr/ADR-P2A-03-searxng-privacy-disclosure.md:95-98`

The disclosure text is "verbatim-locked by this ADR." But SearXNG's active engines depend on instance configuration. The bundled compose does not specify which engines are enabled; SearXNG's default engine list changes across versions. If a user disables Google in their SearXNG config, the disclosure is inaccurate. Recommend changing "Google, Bing, DuckDuckGo, and Brave" to "configured search engines (by default: Google, Bing, DuckDuckGo, Brave — depending on your SearXNG instance configuration)" to make the disclosure accurate across configs. This also protects against legal exposure if a user claims they were misinformed.

### NIT-5: `round_robin` routing danger is documented only in 02 §11 — should be in quickstart

**Location**: `02-kairos-agent.md:924-943`

The "AVOID: `default_strategy = { type = "round_robin" }` when kairos is registered" warning is correct and important, but it is buried in the implementation spec. Operators will follow the quickstart (00 §4), which shows `gadgetron serve --config ~/.gadgetron/gadgetron.toml`. The config written by `kairos init` should set `default_strategy = { type = "fallback", chain = ["kairos"] }` by default (the spec says it does, at 02:930-935), but the quickstart does not mention the routing strategy at all. A user who manually edits the config and adds `round_robin` will hit confusing failures. Add a one-line callout in 00 §4 step 3 or step 4: "The `kairos init`-generated config sets a kairos-only routing strategy. If you add other providers, see §11 of 02 for routing options."

---

## Determinism Findings

The following items are ambiguous or unresolved in the v2 specs. Blockers are already noted above; remaining items here are lower severity.

1. **`feed_stdin` format (BLOCKER-3 above)** — `02-kairos-agent.md:405-424`. Explicit TBD. Blocks `session.rs` and `fake_claude.rs::stdin_echo`.

2. **`rmcp` maturity verification** — `01-knowledge-layer.md:11 open items table`, `00-overview.md:745`. Listed as "validate maturity before P2A impl" and "PM action." Neither doc specifies what "mature enough" means — no minimum version, no minimum issue-tracker closure rate, no last-release-date threshold. If `rmcp` is immature, the fallback is "implement MCP stdio protocol manually" (01:214 `// with manual fallback outline`). The fallback outline is mentioned but not spec'd — no byte format, no JSON-RPC structure, no test names. This does not block Round 2 review (it is a known open item), but it must be resolved before 01 impl starts. Flagging for PM tracking.

3. **`[knowledge.search]` config field name drift** — `00-overview.md:316` shows `searxng_url = "http://127.0.0.1:8888"` and `search_timeout_secs = 5` under `[knowledge.search]`. But `ADR-P2A-03:131-138` shows the config as `searxng_url` + `max_results = 5` + `timeout_secs = 10` (no `search_` prefix, different defaults). The 01 Rust struct (`SearchConfig`) at `01:1174-1182` shows `searxng_url` + `search_timeout_secs: u64` + `default_search_timeout() = 5`. Three sources, two different defaults (5 vs 10), two different field names (`search_timeout_secs` vs `timeout_secs`), and ADR-03 adds `max_results` as a top-level config field that does not appear in the Rust struct. The authoritative source should be the 01 Rust struct. ADR-P2A-03 config example must be corrected to match. Minor risk of confusion at implementation time.

4. **`wiki::git::autocommit` on detached HEAD** — `01:1573-1584` test expects `GitCorruption { reason: .. "detached" }`. The actual `git2` error for a detached HEAD commit may not contain the string "detached" — it depends on `git2` error message wording. The test uses `matches!` with a `reason.contains("detached")` guard. If the `git2` error string is different, the test passes vacuously (the `matches!` macro in this usage without `assert!` does not fail). Implementation should assert, not match. Flagging as a determinism concern for the implementer.

5. **`CredentialBlocked` variant in `WikiErrorKind` vs `GadgetronError::Wiki` HTTP mapping** — `01-knowledge-layer.md:1238-1244` defines `WikiErrorKind::CredentialBlocked` with HTTP 422. The `00-overview.md` §12 error table does not include `CredentialBlocked` — only the 5 variants from the earlier draft. The 01 spec adds this 6th kind, but the HTTP status and `error_code` / `error_type` mapping for `CredentialBlocked` is specified only inline in the 01 error.rs code comment (`HTTP 422`). The 00 error table (which operators consult for troubleshooting) is out of date. Recommend adding a `CredentialBlocked` row to `00-overview.md §12`.

---

## §1.5-B Usability Checklist (this round)

- [x] **사용자 touchpoint 워크스루** — §4 quickstart covers CLI → OpenWebUI → chat. Error states covered in 01 §1.1. MCP tool errors are internal (Claude Code handles them). Touchpoints mapped.
- [PARTIAL] **에러 메시지 3요소** — All Kairos error messages pass (what/why/what-to-do). `WikiErrorKind::GitCorruption` user message ("Run `git status` in the wiki directory and resolve manually.") lacks actionable specifics for a non-git-expert user. It tells them what tool to run but not what output to look for.
- [x] **CLI flag** — GNU/POSIX: `--docker`, `--wiki-path`, `--config`. `--docker` flag behavior (stdout-only, no file write) is documented. `--help` text not shown but contract is defined. Short aliases (`-p` etc.) not needed for init.
- [x] **API 응답 shape** — OpenAI-compat envelope preserved. `kairos` is just another provider in `/v1/models`. SSE framing reuses existing adapter. `error.code`/`error.type` present in all variants.
- [PARTIAL] **config 필드** — All fields have doc comments, defaults, and env overrides in 00 §6 and 02 §10. `wiki_git_author` is commented out by default (correct). Minor drift between 00 §6 and ADR-03 config example (see Determinism item 3). `kairos` section has no doc comment for `request_timeout_secs` default reasoning (why 300?).
- [x] **defaults 안전성** — `wiki_autocommit = true` (safe), `claude_binary = "claude"` (PATH resolution, safe), `max_concurrent_subprocesses = 4` (conservative for P2A desktop), `request_timeout_secs = 300` (generous but gated by gateway timeout above it). `searxng_url` absent by default (web_search disabled — safe default for privacy). All safe.
- [x] **문서 5분 경로** — 00 §4 gives a 5-step path from zero to chat. All commands are copy-pasteable except the `--scope` flag issue (BLOCKER-1). Steps are sequential and don't require reading 897+1667+1402 lines.
- [PARTIAL] **runbook playbook** — Error table in 00 §12 gives HTTP codes and messages. `troubleshooting.md` Phase 2 section is planned pre-merge (per §15 step 7 and manual rule) but does not yet exist. The plan is sound; execution is a pre-merge gate.
- [x] **하위 호환** — Existing Phase 1 providers, API keys, and endpoints unchanged. `kairos` is additive-only. Gateway untouched. Confirmed in 00 §5 "Explicit non-change" paragraph.
- [N/A] **i18n 준비** — User-visible messages are string literals in Rust. No i18n layer exists in Phase 1 either. Not a regression.

---

## Summary

**V2 is substantially improved.** All six dx-product-lead v1 blockers in 01-knowledge-layer.md and all four dx items in 02-kairos-agent.md are addressed and verifiable at cited locations. The `kairos init` stdout contract is now literal-exact (01 §1.1), the error messages all answer what/why/what-to-do, config fields have defaults and env overrides, and the `--docker` flag UX is clearly specified. The ADRs are production-quality: ADR-P2A-03 in particular is well-written and the disclosure text is clear, accurate (post-v1 correction), and non-legalese.

Three minor blockers remain. BLOCKER-1 (`--scope` flag in quickstart) will cause operator confusion on the first run. BLOCKER-2 (`--wiki-path` behavior underspecified) will cause implementation ambiguity. BLOCKER-3 (`feed_stdin` TBD) is a determinism violation that the implementer will hit immediately. All three can be resolved with targeted doc edits; none require architecture changes.

The most likely source of support tickets will be the router strategy footgun (NIT-5: round_robin + kairos = confusing failures) and the `WikiErrorKind::GitCorruption` error message that tells non-git users to run `git status` without explaining what to look for. The `feed_stdin` TBD is the highest-priority item since it blocks the `session.rs` implementation and the `stdin_echo` test scenario.

The manual plan (00 §15 step 7 + ADRs' pre-merge gate requirement) is structurally sound. The `docs/manual/kairos.md` does not yet exist — this is expected and intentional (pre-merge gate). The plan covers install, login, first chat, both privacy disclosures, and the security warning for `--dangerously-skip-permissions`. Sufficient for implementation to proceed once the three blockers are closed.

**Recommended action**: fix BLOCKER-1, 2, 3 with doc-only edits (no re-review round needed if fixes are straightforward), then proceed to implementation. No full re-round required.
